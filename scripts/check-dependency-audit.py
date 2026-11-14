#!/usr/bin/env python3
"""Audit every tracked lock; fail on findings without an exact, current disposition."""
import argparse
from datetime import date
import json
from pathlib import Path
import subprocess
import sys
import os
import hashlib
import re
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def identities(report):
    rows = list(report['vulnerabilities']['list'])
    for group, warnings in report['warnings'].items():
        for warning in warnings:
            rows.append({**warning, 'kind': group})
    return {(row.get('advisory') or {}).get('id', row.get('kind')) + ':' +
            row['package']['name'] + ':' + row['package']['version'] for row in rows}


def check_settings(settings):
    if (settings['ignore'] or settings['severity'] is not None
            or settings['target_arch'] or settings['target_os']
            or set(settings['informational_warnings']) != {'unmaintained', 'unsound', 'notice'}):
        raise ValueError('cargo-audit applied filtering; raw unfiltered findings are required')


def check(report, dispositions, today):
    allowed = set()
    for row in dispositions:
        if not row['reason'].strip() or date.fromisoformat(row['review_by']) < today:
            raise ValueError('missing reason or expired disposition: ' + row['finding'])
        allowed.add(row['finding'])
    return identities(report) - allowed


def check_database(report, revision):
    # --no-fetch deliberately omits revision metadata in cargo-audit 0.22.2.
    # Bind that raw report to the separately verified fresh Git checkout.
    if (not re.fullmatch(r'[0-9a-f]{40}', revision)
            or report.get('database', {}).get('advisory-count', 0) <= 0
            or report['database'].get('last-commit') not in (None, revision)):
        raise ValueError('audit database revision/count mismatch')


def check_preflight(returncode, output):
    if (returncode not in (0, 1) or 'Updating' not in output or 'crates.io index' not in output
            or "couldn't" in output.lower()):
        raise ValueError('advisory database/index freshness preflight failed; refusing cached-only acceptance')


def check_policy(dispositions, findings, today):
    allowed = {row['finding'] for row in dispositions}
    stale = allowed - findings
    if stale:
        raise ValueError('stale dispositions: ' + ', '.join(sorted(stale)))
    if len(allowed) != len(dispositions):
        raise ValueError('duplicate disposition identities')
    warnings = []
    for row in dispositions:
        remaining = (date.fromisoformat(row['review_by']) - today).days
        if not row['reason'].strip() or remaining < 0:
            raise ValueError('missing reason or expired disposition: ' + row['finding'])
        if remaining <= 14:
            warnings.append(row['finding'])
    return warnings


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/dependency-audit')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    policy = json.loads((ROOT / 'dependency-audit-dispositions.json').read_text())
    locks = subprocess.check_output(['git', 'ls-files', '*Cargo.lock'], cwd=ROOT, text=True).splitlines()
    # Audit 0.22.2's JSON mode hides index-fetch warnings. Reject implicit
    # configuration and require a human-mode freshness preflight before JSON.
    cargo_home = Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo'))
    for config in [ROOT / '.cargo/audit.toml', cargo_home / 'audit.toml']:
        if config.exists():
            raise SystemExit(f'implicit cargo-audit configuration is forbidden: {config}')
    # A cached database or index must not turn a network outage into a pass.
    with urllib.request.urlopen('https://index.crates.io/config.json', timeout=30) as response:
        json.load(response)
    with tempfile.TemporaryDirectory(prefix='dependency-audit-db-') as folder:
        database = Path(folder) / 'advisory-db'
        subprocess.run(['git', 'clone', '--quiet', '--depth', '1',
                        'https://github.com/RustSec/advisory-db.git', str(database)], check=True)
        preflight = subprocess.run(['cargo', 'audit', '--db', str(database), '--file', locks[0]],
                                   cwd=ROOT, capture_output=True, text=True)
        (args.output / 'index-preflight.txt').write_text(preflight.stdout + preflight.stderr)
        check_preflight(preflight.returncode, preflight.stdout + preflight.stderr)
        return scan(locks, policy, args.output, database)


def scan(locks, policy, output, database):
    failed = False
    findings = set()
    revision = subprocess.check_output(['git', '-C', str(database), 'rev-parse', 'HEAD'], text=True).strip()
    provenance = {'advisoryDatabaseCommit': revision, 'reports': {}}
    (output / 'database-provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
    for lock in locks:
        if subprocess.check_output(['git', '-C', str(database), 'rev-parse', 'HEAD'], text=True).strip() != revision:
            raise ValueError('advisory checkout changed during scan')
        result = subprocess.run(['cargo', 'audit', '--json', '--no-fetch', '--db', str(database), '--file', lock], cwd=ROOT, capture_output=True, text=True)
        (output / (lock.replace('/', '_') + '.json')).write_text(result.stdout)
        if result.returncode not in (0, 1):
            raise SystemExit(result.stderr or 'cargo audit failed to execute')
        report = json.loads(result.stdout)
        check_database(report, revision)
        provenance['reports'][lock] = hashlib.sha256(result.stdout.encode()).hexdigest()
        (output / 'database-provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
        check_settings(report['settings'])
        findings |= identities(report)
        unknown = check(report, policy, date.today())
        print(f'{lock}: {len(identities(report))} retained findings; {len(unknown)} undispositioned')
        for finding in sorted(unknown):
            print(f'::error file={lock}::New dependency finding {finding}; update or record an exact reviewed disposition in dependency-audit-dispositions.json')
        failed |= bool(unknown)
    subprocess.run(['git', '-C', str(database), 'diff', '--exit-code', 'HEAD'], check=True)
    for finding in check_policy(policy, findings, date.today()):
        print(f'::warning file=dependency-audit-dispositions.json::Disposition review due within 14 days: {finding}')
    return int(failed)


if __name__ == '__main__':
    sys.exit(main())
