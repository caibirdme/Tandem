"""Keep skill acceptance independent of SDK canonical path representations."""

import copy
import tempfile
import unittest
from pathlib import Path, PureWindowsPath

from skill_acceptance import (
    broken_skill_root,
    selection,
    skills_without_root,
    write_project_skills,
)


class SkillSourceTests(unittest.TestCase):
    def test_invalid_root_excludes_its_skills_after_selection_paths_change(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary).resolve()
            project = directory / "project"
            home = directory / "home"
            project.mkdir()
            home.mkdir()
            fixtures = write_project_skills(project, home)
            for canonical_root in (
                None,
                PureWindowsPath(r"\\?\C:\acceptance"),
                PureWindowsPath(r"\\?\UNC\server\share\acceptance"),
            ):
                skills = copy.deepcopy(fixtures)
                if canonical_root is not None:
                    for skill in skills.values():
                        relative = Path(skill["path"]).relative_to(directory)
                        skill["path"] = str(canonical_root.joinpath(*relative.parts))
                original = copy.deepcopy(skills)
                for source, expected in (
                    (project, {"global"}),
                    (home, {"auto", "manual", "deleted"}),
                ):
                    with self.subTest(canonical_root=canonical_root, source=source.name):
                        with broken_skill_root(source):
                            available = skills_without_root(skills, source)
                            self.assertEqual(
                                {name for name, skill in available.items() if skill["available"]},
                                expected,
                            )
                            for name, skill in skills.items():
                                if not skill["available"]:
                                    self.assertFalse(available[name]["available"])
                                self.assertEqual(available[name]["path"], skill["path"])
                        self.assertEqual(skills, original)
                        self.assertTrue((source / ".agents/skills").is_dir())

    def test_selection_exports_only_name_and_canonical_path(self):
        skill = {
            "name": "review",
            "path": r"\\?\C:\acceptance\project\.agents\skills\review\SKILL.md",
            "root": Path("project/.agents/skills"),
            "description": "Review the changes.",
            "marker": "PRIVATE_FIXTURE_BODY",
            "available": True,
        }
        self.assertEqual(selection(skill), {"name": skill["name"], "path": skill["path"]})


if __name__ == "__main__":
    unittest.main()
