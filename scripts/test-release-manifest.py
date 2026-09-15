#!/usr/bin/env python3
"""Validate external dependency identities without network or live repositories."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "release_manifest", Path(__file__).with_name("generate-release-manifest.py")
)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ExternalGitDependenciesTests(unittest.TestCase):
    commit = "a" * 40
    url = "https://github.com/Dastari/agql-auth.git"

    def fixture(self, query, commit=None, name="agql-auth"):
        source = f"git+{self.url}?{query}"
        metadata = {"workspace_members": ["consumer"], "packages": [{
            "id": "consumer", "name": "consumer", "dependencies": [
                {"name": name, "source": source}
            ]
        }]}
        lock = {"package": [{"name": name, "source": f"{source}#{commit or self.commit}"}]}
        return metadata, lock

    def test_full_revision_record_is_unchanged(self):
        rows = module.external_git_dependencies(*self.fixture(f"rev={self.commit}"))
        self.assertEqual(rows, [{"consumers": ["consumer"], "name": "agql-auth",
                                 "revision": self.commit, "url": self.url}])

    def test_auth_tag_records_the_locked_commit(self):
        rows = module.external_git_dependencies(*self.fixture("tag=v0.19.0"))
        self.assertEqual(rows[0]["revision"], self.commit)
        self.assertEqual(rows[0]["tag"], "v0.19.0")

    def test_revision_and_lock_must_agree(self):
        with self.assertRaisesRegex(SystemExit, "does not match"):
            module.external_git_dependencies(*self.fixture(f"rev={self.commit}", "b" * 40))

    def test_missing_or_ambiguous_lock_is_rejected(self):
        metadata, lock = self.fixture("tag=v0.19.0")
        with self.assertRaisesRegex(SystemExit, "exactly one"):
            module.external_git_dependencies(metadata, {"package": []})
        second = dict(lock["package"][0])
        second["source"] = second["source"].replace(self.commit, "b" * 40)
        lock["package"].append(second)
        with self.assertRaisesRegex(SystemExit, "exactly one"):
            module.external_git_dependencies(metadata, lock)

    def test_tag_reference_cannot_be_satisfied_by_same_commit_revision(self):
        metadata, _ = self.fixture("tag=v0.19.0")
        _, wrong_source = self.fixture(f"rev={self.commit}")
        with self.assertRaisesRegex(SystemExit, "exactly one"):
            module.external_git_dependencies(metadata, wrong_source)

    def test_unreviewed_references_are_rejected(self):
        for query in ["branch=main", "tag=latest", "tag=v01.0.0", "rev=abcdef",
                      f"tag=v0.19.0&rev={self.commit}", "tag=v0.19.0&tag=v0.19.1"]:
            with self.subTest(query=query), self.assertRaises(SystemExit):
                module.external_git_dependencies(*self.fixture(query))
        with self.assertRaises(SystemExit):
            module.external_git_dependencies(*self.fixture("tag=v0.19.0", name="other-crate"))


if __name__ == "__main__":
    unittest.main()
