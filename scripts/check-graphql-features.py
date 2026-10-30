#!/usr/bin/env python3
"""Assert locked normal/build feature graphs in an independent consumer."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / 'crates/graphql-orm/tests/fixtures/graphql-features/Cargo.toml'
AUTH_REV = '1f4b4bfb6d052d04e6c80b80cb7aa3d0c2ebaacd'
AUTH_TAG = 'v0.24.0'
AUTH_SOURCE = f'https://github.com/Dastari/agql-auth.git?tag={AUTH_TAG}'
FORBIDDEN = {'default', 'email-validator', 'graphiql', 'playground', 'tempfile'}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if args.output:
        args.output.mkdir(parents=True, exist_ok=True)
    # Guard every declared edge, including standalone/inheriting fixtures.
    root_dependency = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['dependencies']['async-graphql']
    assert root_dependency == {'version': '7', 'default-features': False}, root_dependency
    manifests = subprocess.check_output(
        ['git', 'ls-files', '--cached', '--others', '--exclude-standard', '--', '*Cargo.toml'],
        cwd=ROOT, text=True,
    ).splitlines()
    for relative in manifests:
        manifest = ROOT / relative
        if 'vendor' in manifest.parts:
            continue
        data = tomllib.loads(manifest.read_text())
        for section in ('dependencies', 'dev-dependencies', 'build-dependencies'):
            edge = data.get(section, {}).get('async-graphql')
            if edge is None:
                continue
            assert isinstance(edge, dict), (manifest, edge)
            if not edge.get('workspace', False):
                assert edge.get('default-features') is False, (manifest, edge)
    # This fixture is independently resolved; root dev-dependency unification
    # cannot supply a missing scalar or accidentally enable dynamic-schema.
    lanes = {
        'orm-none': 'orm',
        'orm-default': None,
        'profiles': 'profiles',
        'auth': 'auth',
        'ai-none': 'ai',
        'combined': 'orm,sqlite,ai,profiles,auth,runtime,router,router-auth',
        'additive': 'orm,sqlite,ai,profiles,auth,runtime,router,router-auth,additive',
    }
    for backend in ('sqlite', 'postgres', 'mssql'):
        for mode in ('', ',runtime', ',auth', ',router'):
            lanes[f'orm-{backend}{mode.replace(",", "-")}'] = f'orm,{backend}{mode}'
        lanes[f'ai-{backend}'] = f'ai,{backend}'
        lanes[f'ai-{backend}-provider'] = f'ai,{backend},provider'
    results = {}
    for name, features in lanes.items():
        command = ['cargo', 'tree', '--manifest-path', str(MANIFEST), '--locked',
                   '--edges', 'normal,build', '--prefix', 'none', '--format', '{p} features=[{f}]']
        if features is not None:
            command += ['--no-default-features', '--features', features]
        tree = subprocess.run(command, cwd=ROOT, check=True, capture_output=True, text=True).stdout
        if args.output:
            (args.output / f'{name}.txt').write_text(tree)
        matches = re.findall(r'^async-graphql v(\S+) .*?features=\[([^]]*)\]', tree, re.M)
        assert matches, (name, 'missing async-graphql')
        assert len({version for version, _ in matches}) == 1, (name, matches)
        actual = set().union(*(set(flags.split(',')) - {''} for _, flags in matches))
        prohibited = FORBIDDEN - ({'graphiql', 'tempfile'} if name == 'additive' else set())
        assert not actual & prohibited, (name, 'unexpected defaults', actual & prohibited)
        assert not re.search(r'^(?:fast_chemail|ascii_utils) v', tree, re.M), name
        selected = set((features or 'orm,sqlite').split(','))
        has_orm = bool(selected & {'orm', 'ai', 'runtime', 'router'})
        required = {'dataloader', 'uuid', 'decimal'} if has_orm else set()
        assert required <= actual, (name, 'missing required', required - actual)
        assert ('dynamic-schema' in actual) == ('runtime' in selected), (name, actual)
        if not has_orm:
            assert not actual & {'dataloader', 'uuid', 'decimal'}, (name, actual)
        if name == 'additive':
            assert {'graphiql', 'tempfile'} <= actual, (name, actual)
        auth = re.findall(r'^agql-auth v(\S+) \(([^\n]+?)\) features=', tree, re.M)
        if selected & {'ai', 'auth', 'router-auth'}:
            assert auth, (name, 'missing Auth')
            for version, source in auth:
                url, _, short = source.partition('#')
                assert version == '0.24.0' and url == AUTH_SOURCE, (name, auth)
                assert len(short) >= 7 and AUTH_REV.startswith(short), (name, auth)
        else:
            assert not auth, (name, 'unexpected Auth', auth)
        results[name] = {'features': sorted(actual), 'auth': sorted(set(auth)), 'command': command}
        print(f'{name}: {",".join(sorted(actual)) or "base APIs only"}', flush=True)
    fixture_manifests = [
        'crates/graphql-orm-macros/fixtures/full-stack/Cargo.toml',
        'crates/graphql-orm-macros/fixtures/postgres-smoke/Cargo.toml',
        'crates/graphql-orm-macros/fixtures/cross-crate-relations/Cargo.toml',
        'crates/graphql-orm-macros/fixtures/relationship-capability/Cargo.toml',
        'crates/graphql-orm/tests/fixtures/backend-coexistence/Cargo.toml',
        'crates/graphql-orm/tests/fixtures/federation-end-to-end/Cargo.toml',
        'crates/graphql-orm/tests/fixtures/migration-api-compatibility/Cargo.toml',
        'crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml',
    ]
    for relative in fixture_manifests:
        name = 'fixture-' + Path(relative).parent.name
        command = ['cargo', 'tree', '--manifest-path', str(ROOT / relative), '--locked',
                   '--edges', 'normal,build', '--prefix', 'none', '--format', '{p} features=[{f}]']
        tree = subprocess.run(command, cwd=ROOT, check=True, capture_output=True, text=True).stdout
        flags = re.findall(r'^async-graphql v\S+ .*?features=\[([^]]*)\]', tree, re.M)
        assert flags, (name, 'missing GraphQL')
        actual = set().union(*(set(value.split(',')) - {''} for value in flags))
        assert not actual & FORBIDDEN, (name, actual)
        assert not re.search(r'^(?:fast_chemail|ascii_utils) v', tree, re.M), name
        assert {'dataloader', 'uuid', 'decimal'} <= actual, (name, actual)
        assert 'dynamic-schema' not in actual, (name, actual)
        results[name] = {'features': sorted(actual), 'command': command}
        if args.output:
            (args.output / f'{name}.txt').write_text(tree)
        print(f'{name}: {",".join(sorted(actual))}', flush=True)
    # Assert the exact full source identity in the lock as well as Cargo's
    # abbreviated tree rendering, including inactive optional fixture deps.
    for lock in [ROOT / 'Cargo.lock', MANIFEST.with_name('Cargo.lock'),
                 *( (ROOT / relative).with_name('Cargo.lock') for relative in fixture_manifests)]:
        packages = tomllib.loads(lock.read_text())['package']
        auth = [p for p in packages if p['name'] == 'agql-auth']
        if not auth:
            continue
        assert len(auth) == 1 and auth[0]['version'] == '0.24.0', (lock, auth)
        assert auth[0]['source'] == f'git+{AUTH_SOURCE}#{AUTH_REV}', (lock, auth)
    if args.output:
        (args.output / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
    print(f'graphql-features: {len(results)} locked standalone lanes passed')


if __name__ == '__main__':
    main()
