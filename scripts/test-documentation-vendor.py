"""Bounded controls for the one immutable vendor-document exception."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("documentation_checker", REPO / "scripts/check-documentation.py")
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class SmbVendorDocumentationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="smb-vendor-docs-")
        self.root = Path(self.temporary.name)
        self.original_root = checker.ROOT
        for name in [*checker.SMB_VENDOR_MARKDOWN_SHA256, "PROVENANCE.json"]:
            source = REPO / checker.SMB_VENDOR / name
            target = self.root / checker.SMB_VENDOR / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(source.read_bytes())
        checker.ROOT = self.root

    def tearDown(self):
        checker.ROOT = self.original_root
        self.temporary.cleanup()

    def test_exact_documents_only(self):
        expected = {checker.SMB_VENDOR / name for name in checker.SMB_VENDOR_MARKDOWN_SHA256}
        self.assertEqual(checker.reviewed_smb_vendor_markdown(), expected)
        extra = checker.SMB_VENDOR / "extra.md"
        (self.root / extra).write_text("not governed metadata")
        self.assertNotIn(extra, checker.reviewed_smb_vendor_markdown())
        self.assertNotIn(Path("crates/another/vendor/smb/README.md"), checker.reviewed_smb_vendor_markdown())

    def test_missing_or_malformed_provenance_is_not_exempt(self):
        path = self.root / checker.SMB_VENDOR / "PROVENANCE.json"
        path.unlink()
        self.assertEqual(checker.reviewed_smb_vendor_markdown(), set())
        path.write_text("not JSON")
        self.assertEqual(checker.reviewed_smb_vendor_markdown(), set())

    def test_altered_document_even_with_rewritten_provenance_is_not_exempt(self):
        import hashlib
        path = self.root / checker.SMB_VENDOR / "README.md"
        path.write_text("modified upstream documentation")
        provenance = self.root / checker.SMB_VENDOR / "PROVENANCE.json"
        data = json.loads(provenance.read_text())
        data["documentation_sha256"]["README.md"] = hashlib.sha256(path.read_bytes()).hexdigest()
        provenance.write_text(json.dumps(data))
        self.assertEqual(checker.reviewed_smb_vendor_markdown(), set())

    def test_different_archive_or_patch_is_not_exempt(self):
        path = self.root / checker.SMB_VENDOR / "PROVENANCE.json"
        original = json.loads(path.read_text())
        for section, key in [("upstream", "archive_sha256"), ("backport", "commit")]:
            data = json.loads(json.dumps(original))
            data[section][key] = "0" * len(data[section][key])
            path.write_text(json.dumps(data))
            self.assertEqual(checker.reviewed_smb_vendor_markdown(), set())

    def test_missing_document_is_not_exempt(self):
        (self.root / checker.SMB_VENDOR / "README.md").unlink()
        self.assertEqual(checker.reviewed_smb_vendor_markdown(), set())

    def test_symlink_document_is_not_exempt(self):
        path = self.root / checker.SMB_VENDOR / "README.md"
        outside = self.root / "outside.md"
        outside.write_bytes(path.read_bytes())
        path.unlink()
        path.symlink_to(outside)
        self.assertEqual(checker.reviewed_smb_vendor_markdown(), set())


if __name__ == "__main__":
    unittest.main()
