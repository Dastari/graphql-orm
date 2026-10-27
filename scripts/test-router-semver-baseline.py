#!/usr/bin/env python3
"""Check the exact dependency-only historical baseline adjustment."""

import importlib.util
from pathlib import Path
import tempfile
import tomllib
import unittest

spec = importlib.util.spec_from_file_location(
    "baseline", Path(__file__).with_name("prepare-router-semver-baseline.py")
)
baseline = importlib.util.module_from_spec(spec)
spec.loader.exec_module(baseline)


class BaselineTests(unittest.TestCase):
    def fixture(self, root: Path, hive: str = "=0.2.19") -> Path:
        (root / "Cargo.toml").write_text(
            f'[workspace.dependencies]\nhive-router = "{hive}"\n'
        )
        crate = root / "crates/graphql-orm-router"
        crate.mkdir(parents=True)
        (crate / "lib.rs").write_text("// historical source\n")
        manifest = crate / "Cargo.toml"
        manifest.write_text(
            '[package]\nname = "graphql-orm-router"\nversion = "0.6.6"\n'
            '[features]\ndefault = []\nauth-agql = ["dep:agql-auth"]\n'
            '[dependencies]\nhive-router = { workspace = true }\n'
            'agql-auth = { version = "0.23", optional = true }\n'
        )
        return manifest

    def test_only_dependency_changes_and_second_run_is_unchanged(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = self.fixture(root)
            original = tomllib.loads(manifest.read_text())
            self.assertTrue(baseline.prepare(root))
            adjusted = tomllib.loads(manifest.read_text())
            del adjusted["dependencies"][baseline.DEPENDENCY]
            self.assertEqual(adjusted, original)
            self.assertEqual((manifest.parent / "lib.rs").read_text(), "// historical source\n")
            self.assertFalse(baseline.prepare(root))

    def test_other_hive_identity_is_untouched(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = self.fixture(root, "=0.2.20")
            original = manifest.read_bytes()
            self.assertFalse(baseline.prepare(root))
            self.assertEqual(manifest.read_bytes(), original)

    def test_conflicting_constraint_fails_without_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = self.fixture(root)
            with manifest.open("a") as stream:
                stream.write('datadog-opentelemetry = "=0.5.3"\n')
            original = manifest.read_bytes()
            with self.assertRaises(ValueError):
                baseline.prepare(root)
            self.assertEqual(manifest.read_bytes(), original)


if __name__ == "__main__":
    unittest.main()
