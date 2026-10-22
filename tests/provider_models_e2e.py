#!/usr/bin/env python3
"""Verify model discovery through the real HTTP development host and local fixtures.

No inference provider, paid API, or user credential is used. Build the development
host first, then run python3 tests/provider_models_e2e.py [--server PATH].
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import tempfile
import threading
from contextlib import closing
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

from acceptance_support import Application, debug_binary
from credential_checks import require_provider_key, scan_for_key
from inference_fixture import require

FIXTURE_KEY = "discovery-only-test-key-not-a-real-credential"
ERROR_MARKER = "upstream-private-response-marker"


def handler_for(requests):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, _format, *_args):
            pass

        def do_GET(self):
            requests.append((self.path, dict(self.headers)))
            parsed = urlsplit(self.path)
            query = parse_qs(parsed.query)
            if parsed.path.endswith("/redirect"):
                self.send_response(302)
                self.send_header("Location", f"http://localhost:{self.server.server_port}/leak")
                self.end_headers()
                return
            if parsed.path.endswith("/error"):
                self.respond(401, {"error": ERROR_MARKER + FIXTURE_KEY})
                return
            if parsed.path.endswith("/malformed"):
                self.respond(200, {"data": ERROR_MARKER + FIXTURE_KEY})
                return
            if parsed.path.endswith("/loop"):
                self.respond(200, page([("loop", "Loop")], True, "loop"))
                return
            if parsed.path.endswith("/empty"):
                self.respond(200, page([], False, None))
                return
            if parsed.path.endswith("/unsupported"):
                self.respond(200, page([("unsupported", "Unsupported")], False, "unsupported"))
                return
            if parsed.path.endswith("/invalid-limits"):
                value = page([("second", "Second")], False, "second")
                value["data"][0]["max_tokens"] = 0
                self.respond(200, value)
                return
            if query.get("after_id") == ["second"]:
                self.respond(200, page([("second", "Second"), ("third", "Third"), ("k3", "K3")], False, "k3"))
            else:
                self.respond(200, page([("first", "First"), ("second", "Second")], True, "second"))

        def respond(self, status, value):
            body = json.dumps(value).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    return Handler


def page(models, has_more, last_id):
    return {
        "data": [wire_model(model_id, name) for model_id, name in models],
        "has_more": has_more,
        "first_id": models[0][0] if models else None,
        "last_id": last_id,
    }


def wire_model(model_id, name):
    value = {"type": "model", "id": model_id, "display_name": name,
             "created_at": "2026-10-21T00:00:00Z"}
    if model_id == "second":
        value.update({
            "max_input_tokens": 1000000, "max_tokens": 128000,
            "capabilities": {"effort": {"supported": True, **{
                level: {"supported": True} for level in ("low", "medium", "high", "xhigh", "max")
            }}},
        })
    elif model_id == "third":
        value["capabilities"] = {"effort": {
            "supported": True, "low": {"supported": False},
            "high": {"supported": True}, "xhigh": {"supported": True},
        }}
    elif model_id == "k3":
        value.update({"max_input_tokens": 1000000, "max_tokens": 1000000})
    elif model_id == "unsupported":
        value["capabilities"] = {"effort": {"supported": False}}
    return value


def fetch(app, base, path="/v1/models"):
    return app.command("list_provider_models", baseUrl=base,
                       listModelsPath=path, apiKey=FIXTURE_KEY)


def require_failure(callback):
    try:
        callback()
    except RuntimeError as error:
        message = str(error)
        require(FIXTURE_KEY not in message, "Discovery error exposed its API key")
        require(ERROR_MARKER not in message, "Discovery error exposed upstream content")
        return
    raise AssertionError("Invalid discovery request unexpectedly succeeded")


def check_requests(requests, expected_path):
    require(len(requests) == 2, "Pagination did not make exactly two requests")
    for path, headers in requests:
        require(urlsplit(path).path == expected_path, "Base URL prefix or configured path was lost")
        normalized = {key.lower(): value for key, value in headers.items()}
        require(normalized.get("x-api-key") == FIXTURE_KEY, "Models API authentication is missing")
        require(normalized.get("anthropic-version") == "2023-06-01", "Models API version differs")
    require(parse_qs(urlsplit(requests[1][0]).query).get("after_id") == ["second"],
            "Next page did not use the last_id cursor")


def check_discovery(app, base, requests):
    expected = [
        {"id": "first", "displayName": "First"},
        {"id": "second", "displayName": "Second", "contextWindow": 1000000,
         "maxOutputTokens": 128000,
         "supportedReasoningEfforts": ["low", "medium", "high", "xhigh", "max"]},
        {"id": "third", "displayName": "Third", "supportedReasoningEfforts": ["high", "xhigh"]},
        {"id": "k3", "displayName": "K3", "contextWindow": 1000000, "maxOutputTokens": 1000000},
    ]
    for prefix, path, expected_path in (
        ("/gateway/", "/v1/models", "/gateway/v1/models"),
        ("/gateway", "catalog/models?catalog=visible", "/gateway/catalog/models"),
    ):
        requests.clear()
        require(fetch(app, base + prefix, path) == expected,
                "Discovery lost token limits, reasoning support, display names, order, or deduplication")
        check_requests(requests, expected_path)
        if "catalog=" in path:
            require(all(parse_qs(urlsplit(item[0]).query).get("catalog") == ["visible"]
                        for item in requests), "Configured path query was lost during pagination")
    require(app.command("snapshot")["settings"]["providers"] == [],
            "Fetching models unexpectedly persisted a provider")
    require(fetch(app, base, "/empty") == [], "An empty catalog should succeed")
    require(fetch(app, base, "/unsupported") == [
        {"id": "unsupported", "displayName": "Unsupported", "supportedReasoningEfforts": []},
    ], "Explicitly unsupported reasoning was treated as unknown support")
    for path in ("/error", "/malformed", "/loop", "/redirect", "/invalid-limits"):
        requests.clear()
        require_failure(lambda: fetch(app, base, path))
        require(not any(urlsplit(item[0]).path == "/leak" for item in requests),
                "Discovery followed a cross-origin redirect")
    require_failure(lambda: fetch(app, base + "/gateway/?tenant=fixture"))
    for path in ("https://example.invalid/models", "//example.invalid/models"):
        requests.clear()
        require_failure(lambda: fetch(app, base, path))
        require(not requests, "Invalid discovery path reached the fixture")


def save_selected(app, base):
    selected = next(model for model in fetch(app, base) if model["id"] == "second")
    provider = app.command("save_provider", provider={
        "id": "", "name": "Discovery fixture", "protocol": "anthropic",
        "baseUrl": base, "listModelsPath": "/catalog/models?catalog=visible",
        "models": [{"id": selected["id"], "name": selected["displayName"],
                    "contextWindow": selected["contextWindow"],
                    "maxOutputTokens": selected["maxOutputTokens"], "reasoningEffort": "xhigh",
                    "supportedReasoningEfforts": selected["supportedReasoningEfforts"]}],
    }, apiKey=FIXTURE_KEY)
    require([model["id"] for model in provider["models"]] == ["second"],
            "Saving selected models included unselected catalog entries")
    require(provider["models"][0] == {
        "id": "second", "name": "Second", "contextWindow": 1000000,
        "maxOutputTokens": 128000, "reasoningEffort": "xhigh",
        "supportedReasoningEfforts": ["low", "medium", "high", "xhigh", "max"],
    }, "Saving a discovered model dropped token limits, reasoning support, or xhigh")
    require_provider_key(app, provider["id"], FIXTURE_KEY)
    return provider


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=debug_binary("tandem-dev-server"))
    arguments = parser.parse_args()
    require(arguments.server.is_file(), "Build tandem-dev-server first")
    requests = []
    endpoint = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(requests))
    worker = threading.Thread(target=endpoint.serve_forever, daemon=True)
    worker.start()
    base = f"http://127.0.0.1:{endpoint.server_port}"
    try:
        with tempfile.TemporaryDirectory(prefix="tandem-discovery-e2e-") as temporary:
            root = Path(temporary)
            app = Application(arguments.server.resolve(), root)
            try:
                check_discovery(app, base, requests)
                provider = save_selected(app, base)
            finally:
                app.close()
            app = Application(arguments.server.resolve(), root)
            try:
                restored = app.command("snapshot")["settings"]["providers"]
                require(restored == [provider], "Configured discovery path or models did not survive restart")
                require_provider_key(app, provider["id"], FIXTURE_KEY)
            finally:
                app.close()
            # Exercise real persisted JSON lacking the new field, not merely an old request.
            database = root / "data" / "tandem.sqlite3"
            with closing(sqlite3.connect(database)) as connection:
                provider.pop("listModelsPath")
                connection.execute("UPDATE providers SET data=? WHERE id=?",
                                   (json.dumps(provider), provider["id"]))
                connection.commit()
            app = Application(arguments.server.resolve(), root)
            try:
                restored = app.command("snapshot")["settings"]["providers"][0]
                require(restored["listModelsPath"] == "/v1/models",
                        "Persisted provider without a discovery path did not receive the default")
                require(restored["models"] == provider["models"], "Defaulting path changed configured models")
            finally:
                app.close()
            require(not scan_for_key(root, FIXTURE_KEY, allow_local_vault=True),
                    "Fixture key leaked outside the local credential table")
        print("PASS: paginated discovery, token limits, reasoning support, xhigh, auth, safe errors, model persistence, restart, credential isolation")
    finally:
        endpoint.shutdown()
        endpoint.server_close()
        worker.join(timeout=5)


if __name__ == "__main__":
    main()
