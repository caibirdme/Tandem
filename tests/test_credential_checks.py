"""Credential storage exemptions stay narrow; only disposable markers are used."""

import shutil
import sqlite3
import tempfile
import unittest
from contextlib import closing
from pathlib import Path
from unittest.mock import Mock, patch

from credential_checks import require_provider_key, scan_for_key

MARKER = "fixture-only-test-key"


class CredentialScanTests(unittest.TestCase):
    def test_only_the_named_credential_column_is_exempt(self):
        for table, column in [
            ("provider_secrets", "secret"),
            ("provider_secrets", "id"),
            ("provider_secrets", "notes"),
            ("providers", "data"),
            ("messages", "data"),
        ]:
            with (
                self.subTest(table=table, column=column),
                tempfile.TemporaryDirectory() as temporary,
            ):
                root = Path(temporary)
                (root / "data").mkdir()
                database = root / "data" / "tandem.sqlite3"
                with closing(sqlite3.connect(database)) as connection:
                    connection.executescript("""
                        CREATE TABLE provider_secrets(id TEXT, secret TEXT, notes TEXT);
                        CREATE TABLE providers(data TEXT);
                        CREATE TABLE messages(data TEXT);
                    """)
                    connection.execute(
                        f'INSERT INTO "{table}"("{column}") VALUES(?)', (MARKER,)
                    )
                    connection.commit()
                allowed = (table, column) == ("provider_secrets", "secret")
                self.assertEqual(
                    scan_for_key(root, MARKER, allow_local_vault=True),
                    [] if allowed else ["data/tandem.sqlite3"],
                )
                self.assertEqual(scan_for_key(root, MARKER), ["data/tandem.sqlite3"])

    def test_committed_wal_is_checked_through_its_logical_tables(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "data").mkdir()
            database = root / "data" / "tandem.sqlite3"
            with closing(sqlite3.connect(database)) as connection:
                connection.executescript("""
                    PRAGMA journal_mode=WAL;
                    PRAGMA wal_autocheckpoint=0;
                    CREATE TABLE provider_secrets(id TEXT, secret TEXT);
                    CREATE TABLE messages(data BLOB);
                """)
                connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")
                connection.execute(
                    "INSERT INTO provider_secrets VALUES('provider', ?)", (MARKER,)
                )
                connection.commit()
                self.assertNotIn(MARKER.encode(), database.read_bytes())
                self.assertEqual(scan_for_key(root, MARKER, allow_local_vault=True), [])
                connection.execute("INSERT INTO messages VALUES(?)", (MARKER.encode(),))
                connection.commit()
                self.assertNotIn(MARKER.encode(), database.read_bytes())
                self.assertEqual(
                    scan_for_key(root, MARKER, allow_local_vault=True),
                    ["data/tandem.sqlite3"],
                )

    def test_checkpointed_wal_without_sidecars_keeps_the_same_narrow_exemption(self):
        for leak in (False, True):
            with (
                self.subTest(leak=leak),
                tempfile.TemporaryDirectory() as temporary,
                tempfile.TemporaryDirectory() as source_directory,
            ):
                root = Path(temporary)
                (root / "data").mkdir()
                source = Path(source_directory) / "checkpoint.sqlite3"
                with closing(sqlite3.connect(source)) as connection:
                    connection.executescript("""
                        PRAGMA journal_mode=WAL;
                        CREATE TABLE provider_secrets(id TEXT, secret TEXT);
                        CREATE TABLE messages(data TEXT);
                    """)
                    connection.execute(
                        "INSERT INTO provider_secrets VALUES('provider', ?)", (MARKER,)
                    )
                    if leak:
                        connection.execute("INSERT INTO messages VALUES(?)", (MARKER,))
                    connection.commit()
                    connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")
                database = root / "data" / "tandem.sqlite3"
                shutil.copyfile(source, database)
                self.assertEqual(database.read_bytes()[18:20], b"\x02\x02")
                self.assertFalse(database.with_name(database.name + "-wal").exists())
                self.assertEqual(
                    scan_for_key(root, MARKER, allow_local_vault=True),
                    ["data/tandem.sqlite3"] if leak else [],
                )

    def test_unreadable_or_new_wal_cannot_use_the_checkpointed_database_exemption(self):
        for wal_exists in (False, True):
            with (
                self.subTest(wal_exists=wal_exists),
                tempfile.TemporaryDirectory() as temporary,
            ):
                root = Path(temporary)
                (root / "data").mkdir()
                database = root / "data" / "tandem.sqlite3"
                database.write_bytes(b"SQLite format 3\x00" + b"\x00" * 2 + b"\x02\x02")
                wal = database.with_name(database.name + "-wal")
                if wal_exists:
                    wal.write_bytes(b"")

                def scan(_path, _marker, *, immutable=False):
                    if not immutable:
                        raise sqlite3.OperationalError("Fixture readonly failure")
                    wal.write_bytes(b"")
                    return False

                with patch("credential_checks._database_contains_unexpected_key", side_effect=scan):
                    self.assertEqual(
                        scan_for_key(root, MARKER, allow_local_vault=True),
                        ["data/tandem.sqlite3"],
                    )

    def test_schema_and_other_database_files_are_not_exempt(self):
        for filename, statement in [
            ("tandem.sqlite3", f"CREATE TABLE example(data TEXT DEFAULT '{MARKER}')"),
            ("other.sqlite3", "CREATE TABLE provider_secrets(secret TEXT)"),
        ]:
            with (
                self.subTest(filename=filename),
                tempfile.TemporaryDirectory() as temporary,
            ):
                root = Path(temporary)
                (root / "data").mkdir()
                with closing(sqlite3.connect(root / "data" / filename)) as connection:
                    connection.execute(statement)
                    if filename != "tandem.sqlite3":
                        connection.execute(
                            "INSERT INTO provider_secrets VALUES(?)", (MARKER,)
                        )
                    connection.commit()
                self.assertEqual(
                    scan_for_key(root, MARKER, allow_local_vault=True),
                    ["data/" + filename],
                )

    def test_logs_and_sdk_state_are_scanned_across_block_boundaries(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "data" / "runtime").mkdir(parents=True)
            for relative in ["server.log", "data/runtime/session.json"]:
                (root / relative).write_bytes(b"x" * 65533 + MARKER.encode())
            self.assertEqual(
                scan_for_key(root, MARKER, allow_local_vault=True),
                ["data/runtime/session.json", "server.log"],
            )

    def test_missing_or_unreadable_database_cannot_enable_an_exemption(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "data").mkdir()
            (root / "data" / "tandem.sqlite3-wal").write_text(MARKER)
            self.assertEqual(
                scan_for_key(root, MARKER, allow_local_vault=True),
                ["data/tandem.sqlite3-wal"],
            )
            (root / "data" / "tandem.sqlite3").write_text("invalid SQLite")
            self.assertIn(
                "data/tandem.sqlite3", scan_for_key(root, MARKER, allow_local_vault=True)
            )


class CredentialApiTests(unittest.TestCase):
    def test_explicit_lookup_matches_metadata_without_echoing_secret_values(self):
        for expected in [None, MARKER]:
            with self.subTest(configured=expected is not None):
                app = Mock()
                app.command.side_effect = [
                    expected,
                    {
                        "settings": {
                            "providers": [
                                {"id": "provider", "hasApiKey": expected is not None}
                            ]
                        }
                    },
                ]
                require_provider_key(app, "provider", expected)
                self.assertEqual(
                    app.command.call_args_list[0].args, ("get_provider_api_key",)
                )
                self.assertEqual(
                    app.command.call_args_list[0].kwargs, {"id": "provider"}
                )
        app = Mock()
        app.command.side_effect = [
            MARKER,
            {
                "settings": {
                    "providers": [
                        {"id": "provider", "hasApiKey": True, "leaked": MARKER}
                    ]
                }
            },
        ]
        with self.assertRaisesRegex(AssertionError, "snapshot") as raised:
            require_provider_key(app, "provider", MARKER)
        self.assertNotIn(MARKER, str(raised.exception))


if __name__ == "__main__":
    unittest.main()
