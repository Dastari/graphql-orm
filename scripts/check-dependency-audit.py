#!/usr/bin/env python3
"""Audit every tracked lock; fail on findings without an exact, current disposition."""
import argparse
from datetime import date
import json
from pathlib import Path
import subprocess
import sys

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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/dependency-audit')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    policy = json.loads((ROOT / 'dependency-audit-dispositions.json').read_text())
    locks = subprocess.check_output(['git', 'ls-files', '*Cargo.lock'], cwd=ROOT, text=True).splitlines()
    failed = False
    for lock in locks:
        result = subprocess.run(['cargo', 'audit', '--json', '--file', lock], cwd=ROOT, capture_output=True, text=True)
        (args.output / (lock.replace('/', '_') + '.json')).write_text(result.stdout)
        if result.returncode not in (0, 1):
            raise SystemExit(result.stderr or 'cargo audit failed to execute')
        report = json.loads(result.stdout)
        # An incomplete scan must never be mistaken for a clean report.
        if not report.get('database', {}).get('last-commit'):
            raise SystemExit('audit database revision missing')
        check_settings(report['settings'])
        unknown = check(report, policy, date.today())
        print(f'{lock}: {len(identities(report))} retained findings; {len(unknown)} undispositioned')
        for finding in sorted(unknown):
            print(f'::error file={lock}::New dependency finding {finding}; update or record an exact reviewed disposition in dependency-audit-dispositions.json')
        failed |= bool(unknown)
    return int(failed)


if __name__ == '__main__':
    sys.exit(main())
