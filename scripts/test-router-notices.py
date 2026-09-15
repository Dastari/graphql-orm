#!/usr/bin/env python3
"""Regression tests for router notice review bindings and safe collection."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('router_notices', Path(__file__).with_name('generate-router-notices.py'))
notices = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notices)


class RouterNoticeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.crate = self.root / 'fixture'
        self.crate.mkdir()
        (self.crate / 'Cargo.toml').write_text('[package]\nname="fixture"\nversion="1.0.0"\n')
        (self.crate / 'LICENSE').write_text('Exact upstream license fixture\n')
        self.archive = b'exact locked registry archive fixture'
        self.archive_sha = hashlib.sha256(self.archive).hexdigest()
        (self.root / 'Cargo.lock').write_text(
            'version=4\n[[package]]\nname="fixture"\nversion="1.0.0"\n'
            'source="registry+https://github.com/rust-lang/crates.io-index"\n'
            f'checksum="{self.archive_sha}"\n')
        self.cargo = self.root / 'cargo'
        cache = self.cargo / 'registry/cache/test'
        cache.mkdir(parents=True)
        (cache / 'fixture-1.0.0.crate').write_bytes(self.archive)
        self.metadata = {'packages': [{'name': 'fixture', 'version': '1.0.0', 'license': 'MIT',
            'source': 'registry+https://github.com/rust-lang/crates.io-index',
            'manifest_path': str(self.crate / 'Cargo.toml')}]}
        self.sbom = {'components': [{'name': 'fixture', 'version': '1.0.0'}]}
        self.review = {'formatVersion': 1, 'cargoLockSha256': notices.sha256((self.root / 'Cargo.lock').read_bytes()),
            'componentsSha256': notices.component_digest(self.sbom['components']),
            'target': 'x86_64-unknown-linux-gnu', 'features': ['auth-agql'],
            'supplements': [], 'sourceOnlyNoticePackages': []}
        self.output = self.root / 'output'

    def generate(self):
        return notices.generate(self.root, self.metadata, self.sbom, self.review, self.output, self.cargo)

    def test_collects_exact_notice_with_portable_inventory(self):
        result = self.generate()
        record = result['packages'][0]['notices'][0]
        self.assertEqual((self.output / record['path']).read_bytes(), (self.crate / 'LICENSE').read_bytes())
        self.assertNotIn(str(self.root), json.dumps(result))

    def test_rejects_changed_lock_before_collection(self):
        (self.root / 'Cargo.lock').write_text('changed')
        with self.assertRaisesRegex(ValueError, 'Cargo.lock'):
            self.generate()
        self.assertFalse(self.output.exists())

    def test_rejects_wrong_sbom_component_set(self):
        self.sbom['components'].append({'name': 'extra', 'version': '1.0.0'})
        with self.assertRaisesRegex(ValueError, 'component set'):
            self.generate()

    def test_rejects_duplicate_sbom_identity(self):
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            notices.component_digest(self.sbom['components'] * 2)

    def test_missing_notice_requires_reviewed_source_only_entry(self):
        (self.crate / 'LICENSE').unlink()
        with self.assertRaisesRegex(ValueError, 'without notice'):
            self.generate()

    def test_source_only_entry_includes_exact_archive_and_disposition(self):
        (self.crate / 'LICENSE').unlink()
        self.review['sourceOnlyNoticePackages'] = [{'name': 'fixture', 'version': '1.0.0'}]
        row = self.generate()['packages'][0]
        self.assertEqual((self.output / row['sourceArchive']['path']).read_bytes(), self.archive)
        self.assertIn('owner review required', row['noticeDisposition'])

    def test_mpl_always_includes_locked_source_archive(self):
        self.metadata['packages'][0]['license'] = 'MPL-2.0'
        row = self.generate()['packages'][0]
        self.assertTrue(row['notices'])
        self.assertEqual(row['sourceArchive']['sha256'], self.archive_sha)

    def test_tampered_cache_cannot_replace_locked_archive(self):
        self.metadata['packages'][0]['license'] = 'MPL-2.0'
        (self.cargo / 'registry/cache/test/fixture-1.0.0.crate').write_bytes(b'tampered')
        with patch.object(notices, 'checked_download', return_value=self.archive) as download:
            row = self.generate()['packages'][0]
        download.assert_called_once_with('https://static.crates.io/crates/fixture/fixture-1.0.0.crate', self.archive_sha)
        self.assertEqual((self.output / row['sourceArchive']['path']).read_bytes(), self.archive)

    def test_download_requires_exact_hash_and_https(self):
        data = b'upstream notice'
        with patch.object(notices.urllib.request, 'urlopen', return_value=io.BytesIO(data)):
            self.assertEqual(notices.checked_download('https://example.invalid/LICENSE', notices.sha256(data)), data)
        with patch.object(notices.urllib.request, 'urlopen', return_value=io.BytesIO(data)):
            with self.assertRaisesRegex(ValueError, 'digest mismatch'):
                notices.checked_download('https://example.invalid/LICENSE', '0' * 64)
        with self.assertRaisesRegex(ValueError, 'HTTPS'):
            notices.checked_download('http://example.invalid/LICENSE', '0' * 64)

    def test_rejects_notice_symlink_outside_package(self):
        (self.crate / 'LICENSE').unlink()
        outside = self.root / 'outside'
        outside.write_text('must not copy')
        (self.crate / 'LICENSE').symlink_to(outside)
        with self.assertRaisesRegex(ValueError, 'escapes'):
            self.generate()

    def test_rejects_missing_license_declaration(self):
        self.metadata['packages'][0]['license'] = None
        with self.assertRaisesRegex(ValueError, 'license declaration'):
            self.generate()

    def test_rust_standard_library_notices_are_copied_from_matching_sysroot(self):
        sysroot = self.root / 'rust'
        source = sysroot / 'share/doc/rust/COPYRIGHT-library.html'
        source.parent.mkdir(parents=True)
        source.write_text('exact Rust library notice fixture')
        self.output.mkdir()
        result = notices.collect_rust_notices(sysroot, self.output, 'rustc fixture\n')
        self.assertEqual((self.output / result['notices'][0]['path']).read_bytes(), source.read_bytes())
        self.assertEqual(result['compiler'], 'rustc fixture')
        source.unlink()
        with self.assertRaisesRegex(ValueError, 'rust-docs'):
            notices.collect_rust_notices(sysroot, self.output, 'rustc fixture\n')

    def test_rejects_unsafe_names_and_unreviewed_profiles(self):
        for name, version in [('../escape', '1.0.0'), ('fixture', '../escape')]:
            with self.assertRaises(ValueError):
                notices.package_name(name, version)
        self.review['features'] = []
        with self.assertRaisesRegex(ValueError, 'artifact profile'):
            self.generate()


if __name__ == '__main__':
    unittest.main()
