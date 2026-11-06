#!/usr/bin/env python3
"""RUSTC_WRAPPER: time only the fixture rustc; --run executes the measurement matrix."""
import os
from pathlib import Path
import subprocess
import sys

if len(sys.argv) > 1 and sys.argv[1] in ('--run', '--help', '-h'):
    import argparse
    import shlex
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run', type=Path, required=True, metavar='DIRECTORY')
    parser.add_argument('--entities', type=int, default=56)
    parser.add_argument('--build-target', type=Path, help='Reuse dependency artifacts; fixture rustc is always rebuilt')
    labels = [f'{config}-{repeat}' for config in ('baseline', 'debug-off', 'export') for repeat in (1, 2)]
    labels += ['expanded', 'llvm-lines', 'expanded-export', 'llvm-lines-export', 'time-passes', 'expanded-2', 'llvm-lines-2']
    parser.add_argument('--only', nargs='+', choices=labels, help='Run only these labels (reuse the same fixture/lock)')
    options = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    work = options.run.resolve()
    work.mkdir(parents=True, exist_ok=True)
    subprocess.run([sys.executable, str(root / 'scripts/generate-compile-memory-fixture.py'),
                    str(work / 'fixture'), '--entities', str(options.entities)], check=True)
    env = dict(os.environ, CARGO_BUILD_JOBS='1', CARGO_TARGET_DIR=str((options.build_target or work / 'build').resolve()),
               CARGO_INCREMENTAL='0', RUSTC_WRAPPER=str(Path(__file__).resolve()))
    manifest = str(work / 'fixture/Cargo.toml')
    def measure(label, debug, feature, command, extra=()):
        if options.only and label not in options.only:
            return
        if (work / (label + '.time')).exists() or (work / (label + '.log')).exists():
            raise FileExistsError(f'{label}: use a fresh directory to preserve raw results')
        env.update(CARGO_PROFILE_DEV_DEBUG=debug, COMPILE_MEMORY_LOG=str(work / (label + '.time')))
        (work / 'fixture/src/lib.rs').touch()
        cmd = ['systemd-run', '--user', '--scope', '-q', '-p', 'MemoryHigh=20G',
               '-p', 'MemoryMax=24G', '-p', 'MemorySwapMax=4G', '--', 'cargo', command,
               '--manifest-path', manifest, '--lib', '-j', '1']
        if (work / 'fixture/Cargo.lock').exists():
            cmd += ['--locked']
        if feature:
            cmd += ['--features', feature]
        cmd += list(extra)
        recorded = ['env', *[f'{key}={value}' for key, value in sorted(env.items())
                             if key in ('CARGO_BUILD_JOBS', 'CARGO_TARGET_DIR', 'CARGO_INCREMENTAL',
                                        'CARGO_PROFILE_DEV_DEBUG', 'RUSTC_WRAPPER', 'RUSTC_BOOTSTRAP', 'COMPILE_MEMORY_LOG',
                                        'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTDOCFLAGS')], *cmd]
        (work / (label + '.command')).write_text(shlex.join(recorded) + '\n')
        print(label, flush=True)
        with (work / (label + '.stdout')).open('w') as out, (work / (label + '.log')).open('w') as err:
            subprocess.run(cmd, env=env, stdout=out, stderr=err, check=True)
    for config, debug, feature in [('baseline', 'line-tables-only', ''),
                                   ('debug-off', '0', ''),
                                   ('export', 'line-tables-only', 'schema-export-only')]:
        for repeat in (1, 2):
            measure(f'{config}-{repeat}', debug, feature, 'rustc')
    measure('expanded', 'line-tables-only', '', 'expand')
    measure('llvm-lines', 'line-tables-only', '', 'llvm-lines')
    measure('expanded-2', 'line-tables-only', '', 'expand')
    measure('llvm-lines-2', 'line-tables-only', '', 'llvm-lines')
    measure('expanded-export', 'line-tables-only', 'schema-export-only', 'expand')
    measure('llvm-lines-export', 'line-tables-only', 'schema-export-only', 'llvm-lines')
    env['RUSTC_BOOTSTRAP'] = '1'
    measure('time-passes', 'line-tables-only', '', 'rustc', ('--', '-Z', 'time-passes'))
    import hashlib
    metadata = subprocess.check_output(['rustc', '-vV'], text=True)
    metadata += 'base=' + subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True)
    lock = work / 'fixture/Cargo.lock'
    if lock.exists():
        metadata += 'lock_sha256=' + hashlib.sha256(lock.read_bytes()).hexdigest() + '\n'
    (work / 'metadata.txt').write_text(metadata)
    print(f'Raw measurements: {work}')
    sys.exit(0)

args = sys.argv[1:]
if '--crate-name' in args and args[args.index('--crate-name') + 1] == 'compile_memory_fixture':
    output = Path(os.environ['COMPILE_MEMORY_LOG'])
    output.parent.mkdir(parents=True, exist_ok=True)
    args = ['/usr/bin/time', '-v', '-o', str(output), *args]
sys.exit(subprocess.call(args, close_fds=False))
