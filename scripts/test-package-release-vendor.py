#!/usr/bin/env python3
"""Exercise only the storage-owned SMB source release-policy boundary."""
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('check-package-release-policy.sh')


class VendorReleasePolicy(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git('init', '-q')
        self.git('config', 'user.email', 'fixture@example.invalid')
        self.git('config', 'user.name', 'Fixture')
        for package in ['graphql-orm', 'graphql-orm-macros', 'graphql-orm-storage']:
            self.write(f'crates/{package}/Cargo.toml', '[package]\nversion = "0.1.0"\n')
        for path in ['scripts/check-router-release-policy.sh', 'crates/graphql-orm-ai/scripts/check-release-policy.sh']:
            self.write(path, '#!/usr/bin/env bash\nexit 0\n')
            (self.root / path).chmod(0o755)
        self.write('scripts/check-package-release-policy.sh', SCRIPT.read_text())
        self.write('crates/graphql-orm-storage/vendor/smb/src/resource/directory.rs', '// baseline\n')
        self.write('crates/graphql-orm-storage/vendor/smb/Cargo.toml', '[package]\nversion="0.10.2"\n')
        self.commit()
        self.base = self.git('rev-parse', 'HEAD').strip()

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.root, text=True)

    def write(self, path, content):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)

    def commit(self):
        self.git('add', '.')
        self.git('-c', 'core.hooksPath=/dev/null', 'commit', '--no-gpg-sign', '-qm', 'fixture [skip ci]')

    def policy(self):
        self.commit()
        return subprocess.run(['bash', 'scripts/check-package-release-policy.sh', self.base], cwd=self.root, capture_output=True, text=True)

    def test_runtime_edit_requires_owner_documentation(self):
        self.write('crates/graphql-orm-storage/vendor/smb/src/resource/directory.rs', '// changed\n')
        result = self.policy()
        self.assertEqual(result.returncode, 1)
        self.assertIn('require crates/graphql-orm-storage/README.md', result.stderr)

    def test_manifest_edit_requires_owner_documentation(self):
        self.write('crates/graphql-orm-storage/vendor/smb/Cargo.toml', '[package]\nversion="0.10.2"\npublish=false\n')
        result = self.policy()
        self.assertEqual(result.returncode, 1)
        self.assertIn('require crates/graphql-orm-storage/README.md', result.stderr)

    def test_vendor_documentation_alone_is_not_runtime(self):
        self.write('crates/graphql-orm-storage/vendor/smb/README.md', 'upstream documentation\n')
        result = self.policy()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_runtime_edit_with_owner_version_and_docs_passes(self):
        self.write('crates/graphql-orm-storage/vendor/smb/src/resource/directory.rs', '// changed\n')
        for name in ['README.md', 'CHANGELOG.md', 'MIGRATION.md']:
            self.write('crates/graphql-orm-storage/' + name, 'Release fixture\n')
        self.write('crates/graphql-orm-storage/Cargo.toml', '[package]\nversion = "0.1.1"\n')
        result = self.policy()
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
