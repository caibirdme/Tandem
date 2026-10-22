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
            if query.get("after_id") == ["second"]:
                self.respond(200, page([("second", "Second"), ("third", "Third")], False, "third"))
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
        "data": [{"type": "model", "id": model_id, "display_name": name,
                  "created_at": "2026-10-21T00:00:00Z"} for model_id, name in models],
        "has_more": has_more,
        "first_id": models[0][0] if models else None,
        "last_id": last_id,
    }


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
    expected = [{"id": model_id, "displayName": name}
                for model_id, name in [("first", "First"), ("second", "Second"), ("third", "Third")]]
    for prefix, path, expected_path in (
        ("/gateway/", "/v1/models", "/gateway/v1/models"),
        ("/gateway", "catalog/models?catalog=visible", "/gateway/catalog/models"),
    ):
        requests.clear()
        require(fetch(app, base + prefix, path) == expected,
                "Discovery lost display names, ordering, or duplicate filtering")
        check_requests(requests, expected_path)
        if "catalog=" in path:
            require(all(parse_qs(urlsplit(item[0]).query).get("catalog") == ["visible"]
                        for item in requests), "Configured path query was lost during pagination")
    require(app.command("snapshot")["settings"]["providers"] == [],
            "Fetching models unexpectedly persisted a provider")
    require(fetch(app, base, "/empty") == [], "An empty catalog should succeed")
    for path in ("/error", "/malformed", "/loop", "/redirect"):
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
    provider = app.command("save_provider", provider={
        "id": "", "name": "Discovery fixture", "protocol": "anthropic",
        "baseUrl": base, "listModelsPath": "/catalog/models?catalog=visible",
        "models": [{"id": "second", "name": "Second", "contextWindow": 128000,
                    "maxOutputTokens": None, "reasoningEffort": None}],
    }, apiKey=FIXTURE_KEY)
    require([model["id"] for model in provider["models"]] == ["second"],
            "Saving selected models included unselected catalog entries")
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
        print("PASS: paginated discovery, auth, safe paths/errors, explicit model persistence, restart, credential isolation")
    finally:
        endpoint.shutdown()
        endpoint.server_close()
        worker.join(timeout=5)


if __name__ == "__main__":
    main()
