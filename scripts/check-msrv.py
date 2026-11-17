#!/usr/bin/env python3
"""Enforce the workspace Rust 1.97.1 floor and pinned patch-level CI compiler."""
from pathlib import Path
import tomllib
import re
for manifest in Path('crates').glob('*/Cargo.toml'):
    assert tomllib.loads(manifest.read_text())['package']['rust-version'] == '1.97.1', manifest
toolchain = tomllib.loads(Path('rust-toolchain.toml').read_text())['toolchain']
assert toolchain['channel'] == '1.97.1'
assert set(toolchain['components']) == {'rustfmt', 'clippy', 'rust-src'}
for workflow in ['ci', 'release']:
    # Inspect the whole job, excluding the following job at column two.
    text = re.split(r'\n  [a-z][a-z0-9-]*:\n', Path(f'.github/workflows/{workflow}.yml').read_text().split('  router-msrv:\n')[1])[0]
    assert 'toolchain: 1.97.1' in text and 'scripts/check-msrv.sh' in text, workflow
print('MSRV metadata and CI: Rust 1.97.1 (pinned compiler 1.97.1)')
