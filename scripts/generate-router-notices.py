#!/usr/bin/env python3
"""Package reviewed router notices and exact source archives for release review."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib
import urllib.request


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def component_digest(components):
    pairs = sorted((c['name'], c['version']) for c in components)
    if len(pairs) != len(set(pairs)):
        raise ValueError('duplicate SBOM package identity')
    return sha256(json.dumps(pairs, separators=(',', ':')).encode())


def notice_file(path):
    return path.is_file() and (
        re.search(r'(?i)(^|[._-])(unlicen[cs]e|licen[cs]es?|copying|copyright|notice)([._-]|$)', path.name)
        or any(part.lower() in ('licenses', 'licences') for part in path.parts)
    ) and path.suffix.lower() not in ('.rs', '.c', '.h', '.toml', '.json', '.py')


def package_name(name, version):
    if not re.fullmatch(r'[A-Za-z0-9_-]+', name) or not re.fullmatch(r'[0-9A-Za-z.+_-]+', version):
        raise ValueError('unsafe package identity')
    return f'{name}-{version}'


def checked_download(url, expected):
    if not url.startswith('https://'):
        raise ValueError('notice/source URL must use HTTPS')
    with urllib.request.urlopen(url, timeout=60) as response:
        data = response.read(64 * 1024 * 1024 + 1)
    if len(data) > 64 * 1024 * 1024:
        raise ValueError(f'notice/source download exceeds 64 MiB: {url}')
    if sha256(data) != expected:
        raise ValueError(f'download digest mismatch: {url}')
    return data


def generate(root, metadata, sbom, review, output, cargo_home):
    if review.get('formatVersion') != 1:
        raise ValueError('unsupported router notice review format')
    if review.get('target') != 'x86_64-unknown-linux-gnu' or review.get('features') != ['auth-agql']:
        raise ValueError('unreviewed router artifact profile')
    lock_bytes = (root / 'Cargo.lock').read_bytes()
    if sha256(lock_bytes) != review['cargoLockSha256']:
        raise ValueError('router notice review does not cover this Cargo.lock')
    components = sbom['components']
    if component_digest(components) != review['componentsSha256']:
        raise ValueError('router notice review does not cover this SBOM component set')
    packages = {}
    for package in metadata['packages']:
        key = (package['name'], package['version'])
        if key in packages:
            raise ValueError(f'ambiguous metadata package: {key}')
        packages[key] = package
    lock = tomllib.loads(lock_bytes.decode())
    checksums = {(p['name'], p['version']): p.get('checksum') for p in lock['package']}
    supplements = {(p['name'], p['version']): p for p in review['supplements']}
    source_only = {(p['name'], p['version']) for p in review['sourceOnlyNoticePackages']}
    output.mkdir(parents=True, exist_ok=False)
    rows = []
    downloads = {}
    for component in sorted(components, key=lambda p: (p['name'], p['version'])):
        key = (component['name'], component['version'])
        p = packages[key]
        if not p.get('license'):
            raise ValueError(f'missing license declaration: {key}')
        label = package_name(*key)
        folder = Path(p['manifest_path']).parent.resolve()
        files = {f for f in folder.rglob('*') if notice_file(f)}
        if p.get('license_file'):
            files.add(folder / p['license_file'])
        notices = []
        for f in sorted(files):
            resolved = f.resolve()
            if not resolved.is_relative_to(folder) or f.is_symlink():
                raise ValueError(f'notice escapes package source: {f}')
            destination = output / 'notices' / label / f.relative_to(folder)
            destination.parent.mkdir(parents=True, exist_ok=True)
            data = f.read_bytes()
            destination.write_bytes(data)
            notices.append({'path': str(destination.relative_to(output)), 'sha256': sha256(data)})
        if p['source'] is None:
            if not folder.is_relative_to(root.resolve()):
                raise ValueError('unexpected local package outside the workspace')
            data = (root / 'LICENSE').read_bytes()
            destination = output / 'notices' / label / 'WORKSPACE-LICENSE'
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
            notices.append({'path': str(destination.relative_to(output)), 'sha256': sha256(data)})
        for extra in supplements.get(key, {}).get('notices', []):
            cache_key = (extra['url'], extra['sha256'])
            if cache_key not in downloads:
                downloads[cache_key] = checked_download(*cache_key)
            destination = output / 'notices' / label / ('UPSTREAM-' + extra['sha256'] + '.txt')
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(downloads[cache_key])
            notices.append({'path': str(destination.relative_to(output)), **extra})
        if not notices and key not in source_only:
            raise ValueError(f'unreviewed package without notice files: {key}')
        row = {'name': key[0], 'version': key[1], 'declaredLicense': p['license'],
               'source': p['source'] or 'workspace', 'notices': notices}
        if not notices:
            row['noticeDisposition'] = 'source-only evidence; designated owner review required'
        if 'MPL-2.0' in p['license'] or key in source_only:
            checksum = checksums.get(key)
            if not checksum or not (p['source'] or '').startswith('registry+'):
                raise ValueError(f'no locked registry archive checksum: {key}')
            filename = label + '.crate'
            data = None
            for cached in sorted((cargo_home / 'registry/cache').glob('*/' + filename)):
                candidate = cached.read_bytes()
                if sha256(candidate) == checksum:
                    data = candidate
                    break
            url = f'https://static.crates.io/crates/{key[0]}/{filename}'
            if data is None:
                data = checked_download(url, checksum)
            destination = output / 'sources' / filename
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
            row['sourceArchive'] = {'path': str(destination.relative_to(output)), 'url': url, 'sha256': checksum}
        rows.append(row)
    inventory = {'cargoLockSha256': review['cargoLockSha256'], 'componentsSha256': review['componentsSha256'],
                 'target': review['target'], 'features': review['features'], 'packages': rows}
    (output / 'inventory.json').write_text(json.dumps(inventory, indent=2) + '\n')
    (output / 'README.txt').write_text(
        'Router third-party notice and source evidence\n\n'
        'The inventory retains upstream license declarations, including legacy slash expressions.\n'
        'The notices directory includes packaged, nested/native and reviewed upstream notice files.\n'
        'The sources directory includes exact Cargo.lock-checksummed published archives for MPL\n'
        'components and reviewed packages whose standalone notice files were unavailable.\n'
        'MPL source archives also retain embedded notices, including the Cynic lexer MIT notice.\n'
        'The toolchain directory retains the matching Rust standard-library copyright notices.\n'
        'Source-only notice dispositions require the designated release owner review recorded\n'
        'in the enclosing router-distribution-approval.txt; generation is not that approval.\n'
    )
    return inventory



def collect_rust_notices(sysroot, output, version):
    source = sysroot / 'share/doc/rust/COPYRIGHT-library.html'
    if not source.is_file():
        raise ValueError('matching Rust compiler installation lacks standard-library notices')
    destination = output / 'toolchain/COPYRIGHT-library.html'
    destination.parent.mkdir(parents=True, exist_ok=True)
    data = source.read_bytes()
    destination.write_bytes(data)
    (destination.parent / 'rustc-version.txt').write_text(version)
    return {'compiler': version.strip(), 'notices': [{'path': str(destination.relative_to(output)), 'sha256': sha256(data)}]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--metadata', type=Path, required=True)
    parser.add_argument('--sbom', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--rust-sysroot', type=Path, required=True)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--review', type=Path)
    args = parser.parse_args()
    review_path = args.review or args.root / 'config/router-notice-review.v1.json'
    result = generate(args.root, json.loads(args.metadata.read_text()), json.loads(args.sbom.read_text()),
                      json.loads(review_path.read_text()), args.output,
                      Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo'))))
    version = subprocess.check_output([str(args.rust_sysroot / 'bin/rustc'), '--version', '--verbose'], text=True)
    result['rustToolchain'] = collect_rust_notices(args.rust_sysroot, args.output, version)
    (args.output / 'inventory.json').write_text(json.dumps(result, indent=2) + '\n')
    print(f"Packaged notice/source evidence for {len(result['packages'])} router dependency components")


if __name__ == '__main__':
    main()
