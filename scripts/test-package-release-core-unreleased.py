#!/usr/bin/env python3
"""Keep coordinated unreleased ORM versions distinct from immutable releases."""
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('check-package-release-policy.sh')


class CoreUnreleasedVersionPolicy(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git('init', '-q')
        self.git('config', 'user.email', 'fixture@example.invalid')
        self.git('config', 'user.name', 'Fixture')
        for package in ['graphql-orm', 'graphql-orm-macros']:
            self.write(f'crates/{package}/Cargo.toml', '[package]\nversion = "0.96.0"\n')
            self.write(f'crates/{package}/README.md', 'Version 0.96.0 baseline\n')
            self.write(f'crates/{package}/src/lib.rs', '// baseline\n')
        for path in ['scripts/check-router-release-policy.sh', 'crates/graphql-orm-ai/scripts/check-release-policy.sh']:
            self.write(path, '#!/usr/bin/env bash\nexit 0\n')
            (self.root / path).chmod(0o755)
        self.write('scripts/check-package-release-policy.sh', SCRIPT.read_text())
        self.write('CHANGELOG.md', '# Changelog\n\n## Unreleased\n\nORM runtime/macros **0.96.0**.\n\n- Baseline\n')
        self.write('MIGRATION.md', '# Migration\n')
        self.commit()
        self.base = self.git('rev-parse', 'HEAD').strip()

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.root, text=True)

    def write(self, path, text):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)

    def commit(self):
        self.git('add', '.')
        self.git('-c', 'core.hooksPath=/dev/null', 'commit', '--no-gpg-sign', '-qm', 'fixture [skip ci]')

    def change(self, package='graphql-orm'):
        self.write(f'crates/{package}/src/lib.rs', '// changed\n')
        self.write(f'crates/{package}/README.md', 'Version 0.96.0 changed\n')
        self.write('CHANGELOG.md', '# Changelog\n\n## Unreleased\n\nORM runtime/macros **0.96.0**.\n\n- Changed\n')
        self.write('MIGRATION.md', '# Migration\n\nChanged; no data migration.\n')

    def policy(self):
        self.commit()
        return subprocess.run(['bash', 'scripts/check-package-release-policy.sh', self.base], cwd=self.root, capture_output=True, text=True)

    def test_announced_untagged_core_version_can_receive_coordinated_changes(self):
        self.change()
        result = self.policy()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_announced_untagged_macro_version_can_receive_coordinated_changes(self):
        self.change('graphql-orm-macros')
        result = self.policy()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_existing_qualified_tag_always_requires_new_version(self):
        self.git('tag', 'graphql-orm-v0.96.0')
        self.change()
        result = self.policy()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('SemVer version change', result.stderr)

    def test_existing_macro_tag_always_requires_new_version(self):
        self.git('tag', 'graphql-orm-macros-v0.96.0')
        self.change('graphql-orm-macros')
        result = self.policy()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('SemVer version change', result.stderr)

    def test_historical_version_mention_does_not_announce_unreleased_version(self):
        self.write('CHANGELOG.md', '# Changelog\n\n## Unreleased\n\n- No version announced\n\nORM runtime/macros **0.96.0**.\n')
        self.commit()
        self.base = self.git('rev-parse', 'HEAD').strip()
        self.change()
        result = self.policy()
        self.assertNotEqual(result.returncode, 0)

    def test_unannounced_baseline_version_still_requires_bump(self):
        self.write('CHANGELOG.md', '# Changelog\n\n## Unreleased\n\n- No version announced\n')
        self.commit()
        self.base = self.git('rev-parse', 'HEAD').strip()
        self.change()
        result = self.policy()
        self.assertNotEqual(result.returncode, 0)

    def test_removing_unreleased_announcement_is_not_a_bypass(self):
        self.change()
        self.write('CHANGELOG.md', '# Changelog\n\n## Unreleased\n\n- Changed\n')
        result = self.policy()
        self.assertNotEqual(result.returncode, 0)

    def test_docs_remain_required_for_coordinated_changes(self):
        self.change()
        self.write('crates/graphql-orm/README.md', 'Version 0.96.0 baseline\n')
        result = self.policy()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('require crates/graphql-orm/README.md', result.stderr)


if __name__ == '__main__':
    unittest.main()
