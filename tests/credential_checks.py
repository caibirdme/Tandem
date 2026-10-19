"""Credential assertions for isolated acceptance data; never print secret values."""

import json
import sqlite3
from contextlib import closing

from inference_fixture import require


def require_provider_key(app, provider_id, expected):
    """Only the explicit lookup returns a key; ordinary snapshots remain metadata."""
    actual = app.command("get_provider_api_key", id=provider_id)
    require(actual == expected, "On-demand API key lookup returned the wrong value")
    snapshot = app.command("snapshot")
    provider = next(
        item for item in snapshot["settings"]["providers"] if item["id"] == provider_id
    )
    require(
        provider["hasApiKey"] == (expected is not None),
        "Provider key-presence metadata is incorrect",
    )
    require("apiKey" not in provider, "Provider snapshot exposed an API key field")
    if expected:
        require(
            expected not in json.dumps(snapshot),
            "Provider API key leaked into snapshot",
        )


def require_local_secret(directory, provider_id, expected):
    database = directory / "data" / "relay.sqlite3"
    with closing(
        sqlite3.connect(database.as_uri() + "?mode=ro", uri=True)
    ) as connection:
        row = connection.execute(
            "SELECT secret FROM provider_secrets WHERE id=?", (provider_id,)
        ).fetchone()
    require(
        row is not None and row[0] == expected,
        "Local credential row was not saved as expected",
    )


def contains_key(path, marker):
    # Scan incrementally, including matches spanning a chunk boundary.
    tail = b""
    with path.open("rb") as stream:
        while block := stream.read(64 * 1024):
            content = tail + block
            if marker in content:
                return True
            tail = content[-len(marker) :]
    return False


def database_contains_unexpected_key(path, marker):
    """Read committed SQLite data, including WAL, exempting exactly one column."""

    def contains(value):
        data = value if isinstance(value, bytes) else str(value).encode()
        return marker in data

    with closing(sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)) as connection:
        connection.execute("BEGIN")
        schema = connection.execute(
            "SELECT type, name, sql FROM sqlite_schema"
        ).fetchall()
        if any(contains(value) for row in schema for value in row):
            return True
        for kind, table, _sql in schema:
            if kind != "table":
                continue
            quoted = '"' + table.replace('"', '""') + '"'
            cursor = connection.execute(f"SELECT * FROM {quoted}")
            checked = [
                index
                for index, column in enumerate(cursor.description)
                if (table, column[0]) != ("provider_secrets", "secret")
            ]
            if any(contains(row[index]) for row in cursor for index in checked):
                return True
    return False


def scan_for_key(directory, key, *, allow_local_vault=False):
    """Only this application's named credential column may contain the key.

    Its WAL/SHM are physical parts of the same database, checked through SQLite's
    committed view above. Every other table, column, schema entry, database file,
    runtime file and log is checked. Reports contain paths, never matching data.
    """
    require(bool(key), "Credential scan requires a non-empty marker")
    directory = directory.resolve()
    marker = key.encode()
    database = directory / "data" / "relay.sqlite3"
    checked_database_files = set()
    leaks = []
    if allow_local_vault and database.is_file() and not database.is_symlink():
        try:
            unexpected = database_contains_unexpected_key(database, marker)
        except sqlite3.Error:
            # An unreadable schema cannot establish the narrow storage exemption.
            unexpected = True
        if unexpected:
            leaks.append(str(database.relative_to(directory)))
        checked_database_files = {
            database,
            database.with_name(database.name + "-wal"),
            database.with_name(database.name + "-shm"),
        }
    for path in sorted(directory.rglob("*")):
        if not path.is_file() or path.is_symlink() or path in checked_database_files:
            continue
        if contains_key(path, marker):
            leaks.append(str(path.relative_to(directory)))
    return leaks
