#!/usr/bin/env python3
"""Verify this one SMB backport against its immutable registry archive, offline."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ARCHIVE = 'f1315ec423faffdc9715f518c5cab5660b26484df42adaa87a3325f70c73417b'
PATCH = '5614aa2e0d3090d1051cae66858014c17432c959a817bfd0a628b778f3ad78ba'
LICENSE = '59889992272e339d6af995962dca57cb4565c364255f9a453b0f6f20b0365816'
ROOT = Path(__file__).resolve().parents[1]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def verify(archive):
    vendor = ROOT / 'crates/graphql-orm-storage/vendor/smb'
    require(digest(archive.read_bytes()) == ARCHIVE, 'registry archive checksum')
    require(not any(p.is_symlink() for p in vendor.rglob('*')), 'vendor symlink')
    provenance = json.loads((vendor / 'PROVENANCE.json').read_text())
    require(provenance['upstream']['archive_sha256'] == ARCHIVE, 'invalid SMB backport provenance')
    require(provenance['backport']['commit'] == 'f4524d032c8951cc288e1c8231ad68588583abda', 'invalid SMB backport provenance')
    patch = vendor / 'PATCHES/f4524d0.patch'
    require(digest(patch.read_bytes()) == PATCH, 'reviewed patch checksum')
    require(digest((vendor / 'LICENSE').read_bytes()) == LICENSE, 'upstream MIT checksum')
    with tarfile.open(archive) as source:
        originals = {}
        for member in source.getmembers():
            if member.isdir():
                continue
            path = Path(member.name)
            require(member.isfile() and path.parts[0] == 'smb-0.10.2', 'invalid SMB backport provenance')
            require('..' not in path.parts and not path.is_absolute(), 'invalid SMB backport provenance')
            relative = str(Path(*path.parts[1:]))
            require(relative not in originals, 'invalid SMB backport provenance')
            originals[relative] = source.extractfile(member).read()
    expected = {('Cargo.lock.upstream' if p == 'Cargo.lock' else p) for p in originals}
    expected |= {'LICENSE', 'PATCHES/f4524d0.patch', 'PROVENANCE.json'}
    actual = {str(p.relative_to(vendor)) for p in vendor.rglob('*') if p.is_file()}
    require(actual == expected, f'exact vendor file set: {actual ^ expected}')
    with tempfile.TemporaryDirectory(prefix='smb-backport-verify-') as folder:
        restored = Path(folder) / 'smb'
        shutil.copytree(vendor, restored)
        subprocess.run(['git', 'apply', '--reverse', str(patch)], cwd=restored, check=True)
        manifest = restored / 'Cargo.toml'
        data = manifest.read_bytes()
        require(data.count(b'publish = false\n') == 1, 'sole manifest metadata change')
        require(data.endswith(b'\n[workspace]\n'), 'isolated workspace metadata')
        data = data[:-len(b'\n[workspace]\n')]
        manifest.write_bytes(data.replace(b'publish = false\n', b'', 1))
        for name, data in originals.items():
            stored = 'Cargo.lock.upstream' if name == 'Cargo.lock' else name
            require((restored / stored).read_bytes() == data, f'archive mismatch: {name}')
    rows = provenance['archive_files']
    require(len(rows) == len(originals) and {r['archive_path'] for r in rows} == set(originals), 'invalid SMB backport provenance')
    for row in rows:
        require(row['upstream_sha256'] == digest(originals[row['archive_path']]), 'invalid SMB backport provenance')
        require(row['bytes'] == len(originals[row['archive_path']]), 'invalid SMB backport provenance')
        require(provenance['result_hashes'][row['vendor_path']] == digest((vendor / row['vendor_path']).read_bytes()), 'invalid SMB backport provenance')
    print(f'SMB_BACKPORT_VERIFIED archive_files={len(originals)} vendor_files={len(actual)}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive', type=Path, required=True)
    verify(parser.parse_args().archive)
