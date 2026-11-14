#!/usr/bin/env python3
"""Archive integrity regressions without downloads or tool installation."""
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('installer', Path(__file__).with_name('install-reviewed-cargo-tool.py'))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class ToolTests(unittest.TestCase):
    def test_mismatch_fails_before_installation(self):
        with tempfile.TemporaryDirectory() as folder:
            archive = Path(folder) / 'tool.crate'
            archive.write_bytes(b'reviewed')
            checksum = hashlib.sha256(b'reviewed').hexdigest()
            installer.verify(archive, checksum)
            archive.write_bytes(b'changed')
            with self.assertRaises(ValueError):
                installer.verify(archive, checksum)


if __name__ == '__main__':
    unittest.main()
