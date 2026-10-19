#!/usr/bin/env python3
"""Check production dependency boundaries using Cargo's resolved graph."""

import json
import subprocess
import sys
from pathlib import Path

LAYERS = {
    "tandem-protocol": set(),
    "copilot-runtime": set(),
    "tandem-remote": {"tandem-protocol"},
    "tandem-core": {"copilot-runtime", "tandem-remote"},
    "tandem-server": {"copilot-runtime", "tandem-protocol"},
    "tandem-dev-server": {"tandem-core"},
    "tandem-desktop": {"tandem-core"},
}
SERVER_FORBIDDEN = {
    "tandem-core",
    "tandem-remote",
    "tandem-dev-server",
    "tandem-desktop",
    "axum",
    "rusqlite",
    "libsqlite3-sys",
    "keyring",
    "keyring-core",
    "apple-native-keyring-store",
    "windows-native-keyring-store",
}


def violations(metadata):
    """Ignore test-only edges; normal and build dependencies ship or compile together."""
    names = {package["id"]: package["name"] for package in metadata["packages"]}
    members = set(metadata["workspace_members"])
    member_names = {names[package] for package in members}
    graph = {
        node["id"]: {
            dependency["pkg"]
            for dependency in node["deps"]
            if any(kind["kind"] != "dev" for kind in dependency["dep_kinds"])
        }
        for node in metadata["resolve"]["nodes"]
    }
    errors = []
    for name in sorted(member_names - LAYERS.keys()):
        errors.append(f"Define the dependency boundary for new workspace crate {name}.")
    for name in sorted(LAYERS.keys() - member_names):
        errors.append(f"Required workspace crate is missing: {name}.")
    for package in sorted(members):
        name = names[package]
        if name not in LAYERS:
            continue
        internal = {names[child] for child in graph[package] & members}
        for dependency in sorted(internal - LAYERS[name]):
            errors.append(f"Invalid workspace dependency: {name} -> {dependency}.")

    for package in sorted(members):
        name = names[package]
        if name == "tandem-desktop":
            continue
        for dependency in reachable(graph, package):
            target = names[dependency]
            if target.startswith("tauri"):
                errors.append(
                    f"Only the desktop shell may depend on {target}: found in {name}."
                )
            if name == "tandem-server" and target in SERVER_FORBIDDEN:
                errors.append(
                    f"Remote Agent server must not depend on {target} (directly or transitively)."
                )
    return sorted(errors)


def reachable(graph, root):
    pending = [root]
    visited = set()
    while pending:
        package = pending.pop()
        if package in visited:
            continue
        visited.add(package)
        pending.extend(graph[package] - visited)
    return visited


def main():
    try:
        result = subprocess.run(
            ["cargo", "metadata", "--locked", "--format-version", "1"],
            cwd=Path(__file__).resolve().parents[1],
            capture_output=True,
            text=True,
            check=True,
        )
    except subprocess.CalledProcessError as error:
        print(error.stderr, file=sys.stderr, end="")
        return 1
    errors = violations(json.loads(result.stdout))
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(
        "Workspace boundaries passed; remote Agent server has no desktop, development HTTP, SQLite, or credential-store dependencies."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
