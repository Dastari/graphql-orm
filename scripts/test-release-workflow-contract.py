#!/usr/bin/env python3
"""Require the complete reviewed tag-hardening block, including all commands."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('replay_check', Path(__file__).with_name('check-replay-release-positive.py'))
contract = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contract)


class ContractTests(unittest.TestCase):
    def test_only_the_exact_block_is_normalized(self):
        expected = contract.TAG_HARDENING.replace('BUNDLE_DIR', 'release-dist')
        suffix = '          tags_to_push=()\n'
        contract.check_tag_hardening(expected + suffix, 'release-dist')
        for mutation in [expected.replace('git fetch origin main --tags --force', ':'),
                         expected + '          malicious_command\n',
                         expected.replace('test "${package_tags}" = "${expected_tags}"', ':'),
                         expected.replace('exit 1', 'exit 0')]:
            with self.assertRaises(SystemExit):
                contract.check_tag_hardening(mutation + suffix, 'release-dist')


if __name__ == '__main__':
    unittest.main()
