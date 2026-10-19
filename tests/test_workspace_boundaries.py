"""Guard the split against dependency leaks, including indirect and build edges."""

import unittest

from scripts.check_workspace import LAYERS, violations


def resolved_graph(extra_edges=()):
    edges = [
        (name, child, None) for name, children in LAYERS.items() for child in children
    ]
    edges.extend(extra_edges)
    names = set(LAYERS) | {child for _, child, _ in edges}
    return {
        "packages": [{"id": name, "name": name} for name in sorted(names)],
        "workspace_members": list(LAYERS),
        "resolve": {
            "nodes": [
                {
                    "id": name,
                    "deps": [
                        {"pkg": child, "dep_kinds": [{"kind": kind}]}
                        for parent, child, kind in edges
                        if parent == name
                    ],
                }
                for name in sorted(names)
            ]
        },
    }


class WorkspaceBoundariesTests(unittest.TestCase):
    def test_desktop_storage_and_test_only_dependencies_do_not_leak_into_server(self):
        metadata = resolved_graph(
            [
                ("tandem-core", "rusqlite", None),
                ("tandem-core", "keyring", None),
                ("tandem-desktop", "tauri-build", "build"),
                ("tandem-server", "tandem-core", "dev"),
            ]
        )
        self.assertEqual(violations(metadata), [])

    def test_transitive_runtime_or_build_leaks_are_rejected(self):
        for kind in [None, "build"]:
            with self.subTest(kind=kind):
                metadata = resolved_graph(
                    [
                        ("copilot-runtime", "helper", None),
                        ("helper", "rusqlite", kind),
                    ]
                )
                self.assertTrue(
                    any(
                        "must not depend on rusqlite" in error
                        for error in violations(metadata)
                    )
                )

    def test_reverse_workspace_dependency_is_rejected(self):
        metadata = resolved_graph([("tandem-protocol", "tandem-remote", None)])
        self.assertIn(
            "Invalid workspace dependency: tandem-protocol -> tandem-remote.",
            violations(metadata),
        )

    def test_core_cannot_transitively_depend_on_tauri(self):
        metadata = resolved_graph(
            [
                ("tandem-core", "helper", None),
                ("helper", "tauri-runtime", None),
            ]
        )
        self.assertTrue(
            any("found in tandem-core" in error for error in violations(metadata))
        )

    def test_added_workspace_crate_requires_an_explicit_boundary(self):
        metadata = resolved_graph([("tandem-core", "new-component", None)])
        metadata["workspace_members"].append("new-component")
        self.assertTrue(
            any(
                "new workspace crate new-component" in error
                for error in violations(metadata)
            )
        )


if __name__ == "__main__":
    unittest.main()
