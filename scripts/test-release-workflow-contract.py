#!/usr/bin/env python3
"""Require the complete reviewed tag-hardening block, including all commands."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('replay_check', Path(__file__).with_name('check-replay-release-positive.py'))
contract = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contract)


class ContractTests(unittest.TestCase):
    def test_orm_setup_requires_only_the_reviewed_source_component(self):
        previous = '    steps:\n      - uses: toolchain@reviewed\n        with:\n          components: clippy\n      - name: Tests\n'
        current = '    steps:\n      - uses: toolchain@reviewed\n        with:\n          components: clippy,rust-src\n      - name: Tests\n'
        contract.check_orm_setup(current, previous)
        for mutation in [previous, current.replace('reviewed', 'other'), current.replace('      - name:', '      - run: other_command\n      - name:')]:
            with self.assertRaises(SystemExit):
                contract.check_orm_setup(mutation, previous)

    def test_workspace_msrv_expansion_is_exact(self):
        previous = ('  router-msrv:\n    name: Router minimum supported Rust version\n'
                    '          toolchain: 1.90.0\n'
                    '      - name: Verify router minimum supported Rust version\n'
                    '        run: |\n'
                    '          cargo test -p graphql-orm-router-protocol --locked\n'
                    '          cargo test -p graphql-orm-router --locked\n')
        current = ('  router-msrv:\n    name: Workspace minimum supported Rust version\n'
                   '          toolchain: 1.97.1\n'
                   '      - name: Verify workspace minimum supported Rust version\n'
                   '        run: |\n          scripts/check-msrv.sh\n')
        contract.check_msrv_expansion(current, previous, Path(__file__).with_name('check-msrv.sh').read_text())
        for mutation in [current.replace('1.97.1', 'stable'), current + '          other_command\n',
                         current.replace('scripts/check-msrv.sh', 'true')]:
            with self.assertRaises(SystemExit):
                contract.check_msrv_expansion(mutation, previous, contract.MSRV_SCRIPT)
        for mutation in [contract.MSRV_SCRIPT.replace('cargo check --workspace --all-targets --locked', 'true'),
                         contract.MSRV_SCRIPT + 'other_command\n']:
            with self.assertRaises(SystemExit):
                contract.check_msrv_expansion(current, previous, mutation)

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
