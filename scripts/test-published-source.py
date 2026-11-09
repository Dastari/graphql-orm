#!/usr/bin/env python3
"""Integration regressions using Cargo's real package selection at two commits."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import published_source
from published_source import PublishedSources

SCRIPTS = Path(__file__).parent
spec = importlib.util.spec_from_file_location('release_manifest', SCRIPTS / 'generate-release-manifest.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class PublishedSourceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git('init', '-q')
        self.git('config', 'user.email', 'fixture@example.invalid')
        self.git('config', 'user.name', 'Fixture')
        self.write('Cargo.toml', '[workspace]\nmembers=["crates/graphql-orm", "crates/graphql-orm-macros"]\nresolver="2"\n[workspace.package]\nlicense="MIT"\n')
        for name in ['graphql-orm', 'graphql-orm-macros']:
            self.write(f'crates/{name}/Cargo.toml', f'[package]\nname="{name}"\nversion="0.1.0"\nlicense.workspace=true\nexclude=["tests/", "examples/", "benches/"]\n')
            self.write(f'crates/{name}/src/lib.rs', '// source\n')
            self.write(f'crates/{name}/README.md', 'Read me\n')
        self.prefix = 'crates/graphql-orm/'
        self.manifest = self.prefix + 'Cargo.toml'
        self.write(self.prefix + 'tests/example.rs', '// test\n')
        self.write(self.prefix + 'tests/fixtures/nested/Cargo.toml', '[package]\nname="fixture"\nversion="0.1.0"\n[workspace]\n')
        self.write(self.prefix + 'tests/fixtures/nested/Cargo.lock', '# fixture lock\n')
        self.write(self.prefix + 'build.rs', 'fn main() {}\n')
        self.write(self.prefix + 'LICENSE', 'License text\n')
        for script in ['check-package-release-policy.sh', 'published_source.py']:
            self.write('scripts/' + script, (SCRIPTS / script).read_text())
        for script in ['scripts/check-router-release-policy.sh', 'crates/graphql-orm-ai/scripts/check-release-policy.sh']:
            self.write(script, '#!/bin/sh\nexit 0\n')
            (self.root / script).chmod(0o755)
        self.commit()
        self.base = self.git('rev-parse', 'HEAD').strip()
        self.tag = 'graphql-orm-v0.1.0'
        self.git('tag', self.tag)

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.root, text=True)

    def write(self, path, content):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)

    def commit(self):
        self.git('add', '.')
        self.git('-c', 'core.hooksPath=/dev/null', 'commit', '--no-gpg-sign', '-qm', 'fixture')

    def check_change(self, path, expected, content='changed\n'):
        self.write(path, content)
        self.commit()
        with PublishedSources(self.root) as sources:
            self.assertEqual(sources.changed(self.base, 'HEAD', self.manifest), expected)
        with patch.object(release, 'ROOT', self.root):
            packages = [{'tag': self.tag, 'manifestPath': self.manifest}]
            if expected:
                with self.assertRaisesRegex(SystemExit, 'published package source changed'):
                    release.verify_package_tags(packages, 'HEAD')
            else:
                release.verify_package_tags(packages, 'HEAD')
        result = subprocess.run(['bash', 'scripts/check-package-release-policy.sh', self.base], cwd=self.root, capture_output=True, text=True)
        self.assertEqual(result.returncode, int(expected), result.stdout + result.stderr)

    def test_excluded_test_only_change(self):
        self.check_change(self.prefix + 'tests/example.rs', False)

    def test_new_excluded_test(self):
        self.check_change(self.prefix + 'tests/new_test.rs', False)

    def test_fixture_lock_only_change(self):
        self.check_change(self.prefix + 'tests/fixtures/nested/Cargo.lock', False)

    def test_source_change(self):
        self.check_change(self.prefix + 'src/lib.rs', True)

    def test_published_readme_change(self):
        self.check_change(self.prefix + 'README.md', True)

    def test_build_script_change(self):
        self.check_change(self.prefix + 'build.rs', True)

    def test_license_change(self):
        self.check_change(self.prefix + 'LICENSE', True)

    def test_added_published_file(self):
        self.check_change(self.prefix + 'data/schema.json', True)

    def test_deleted_published_file(self):
        (self.root / self.prefix / 'LICENSE').unlink()
        self.check_change(self.prefix + 'tests/example.rs', True)

    def test_inherited_manifest_change(self):
        self.check_change('Cargo.toml', True, (self.root / 'Cargo.toml').read_text().replace('license="MIT"', 'license="Apache-2.0"'))

    def inherited_dependency(self, requirement='0.1.0', extra='', name='graphql-orm-macros'):
        root_manifest = self.root / 'Cargo.toml'
        root_manifest.write_text(root_manifest.read_text() +
            f'\n[workspace.dependencies]\n{name} = {{ version="{requirement}"{extra} }}\n')
        manifest = self.root / self.manifest
        manifest.write_text(manifest.read_text() + f'\n[dependencies]\n{name}.workspace=true\n')
        self.commit()
        self.base = self.git('rev-parse', 'HEAD').strip()
        self.git('tag', '-f', self.tag)

    def test_sibling_requirement_drift_passes(self):
        self.inherited_dependency(extra=', path="crates/graphql-orm-macros"')
        self.check_change('Cargo.toml', False,
            (self.root / 'Cargo.toml').read_text().replace('version="0.1.0"', 'version="0.1.1"'))

    def test_strict_switch_rejects_sibling_requirement_drift(self):
        self.inherited_dependency(extra=', path="crates/graphql-orm-macros"')
        manifest = self.root / 'Cargo.toml'
        manifest.write_text(manifest.read_text().replace('version="0.1.0"', 'version="0.1.1"'))
        self.commit()
        with patch.object(published_source, 'WORKSPACE_SIBLING_REQUIREMENT_DRIFT', 'strict'):
            with PublishedSources(self.root) as sources:
                self.assertTrue(sources.changed(self.base, 'HEAD', self.manifest))

    def test_external_requirement_change_fails(self):
        self.inherited_dependency(name='serde', requirement='1.0.0')
        self.check_change('Cargo.toml', True,
            (self.root / 'Cargo.toml').read_text().replace('version="1.0.0"', 'version="1.0.1"'))

    def test_external_dependency_with_sibling_name_stays_strict(self):
        self.inherited_dependency()
        self.check_change('Cargo.toml', True,
            (self.root / 'Cargo.toml').read_text().replace('version="0.1.0"', 'version="0.1.1"'))

    def test_sibling_feature_change_fails(self):
        sibling = self.root / 'crates/graphql-orm-macros/Cargo.toml'
        sibling.write_text(sibling.read_text() + '\n[features]\nextra=[]\n')
        self.inherited_dependency(extra=', path="crates/graphql-orm-macros"')
        self.check_change('Cargo.toml', True,
            (self.root / 'Cargo.toml').read_text().replace('path="crates/graphql-orm-macros"',
                'path="crates/graphql-orm-macros", features=["extra"]'))

    def test_sibling_optional_change_fails(self):
        self.inherited_dependency(extra=', path="crates/graphql-orm-macros"')
        self.check_change(self.manifest, True,
            (self.root / self.manifest).read_text().replace('graphql-orm-macros.workspace=true', 'graphql-orm-macros={workspace=true, optional=true}'))

    def test_tests_that_cargo_publishes_remain_strict(self):
        path = self.root / self.manifest
        path.write_text(path.read_text().replace('exclude=["tests/", "examples/", "benches/"]', ''))
        self.commit()
        self.base = self.git('rev-parse', 'HEAD').strip()
        self.git('tag', '-f', self.tag)
        self.check_change(self.prefix + 'tests/example.rs', True)


if __name__ == '__main__':
    unittest.main()
