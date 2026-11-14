#!/usr/bin/env python3
"""Verify a reviewed registry archive before installing its locked Cargo tool."""
import argparse
import hashlib
from pathlib import Path
import subprocess
import tarfile
import tempfile
import urllib.request

# Registry archive checksums cross-checked against crates.io version metadata.
PINS = {
    'cargo-audit': ('0.22.2', '700c2b240f7fd330c24b675fe429f73a5b676531fcc6300400b2b67f155ba12a'),
    'cargo-cyclonedx': ('0.5.9', '5d162f67705f0f5038759d73bf546a083bf30e8677c2e944b416bca48d9d69a8'),
}


def verify(archive, checksum):
    if hashlib.sha256(archive.read_bytes()).hexdigest() != checksum:
        raise ValueError('reviewed Cargo tool archive checksum mismatch')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('tool', choices=PINS)
    parser.add_argument('--verify-only', type=Path, help='verify a local archive without installing')
    args = parser.parse_args()
    version, checksum = PINS[args.tool]
    if args.verify_only:
        verify(args.verify_only, checksum)
        return
    with tempfile.TemporaryDirectory(prefix='reviewed-cargo-tool-') as folder:
        root = Path(folder)
        archive = root / 'tool.crate'
        url = f'https://static.crates.io/crates/{args.tool}/{args.tool}-{version}.crate'
        with urllib.request.urlopen(url, timeout=60) as response:
            archive.write_bytes(response.read())
        verify(archive, checksum)
        with tarfile.open(archive) as source:
            source.extractall(root, filter='data')
        package = root / f'{args.tool}-{version}'
        if not (package / 'Cargo.lock').is_file():
            raise ValueError('reviewed tool archive has no locked build graph')
        subprocess.run(['cargo', 'install', '--path', str(package), '--locked'], check=True)


if __name__ == '__main__':
    main()
