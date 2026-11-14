#!/usr/bin/env python3
"""Regressions for fail-closed dependency disposition matching."""
import importlib.util
from datetime import date
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('audit', Path(__file__).with_name('check-dependency-audit.py'))
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class AuditTests(unittest.TestCase):
    def test_exact_matching_and_new_versions(self):
        report = {'vulnerabilities': {'list': [{'advisory': {'id': 'RUSTSEC-2026-0001'}, 'package': {'name': 'example', 'version': '1.0.0'}}]}, 'warnings': {}}
        finding = 'RUSTSEC-2026-0001:example:1.0.0'
        row = {'finding': finding, 'reason': 'Reviewed path', 'review_by': '2026-12-01'}
        self.assertEqual(audit.check(report, [row], date(2026, 11, 14)), set())
        self.assertEqual(audit.check(report, [], date(2026, 11, 14)), {finding})
        row['finding'] = finding.replace('1.0.0', '1.0.1')
        self.assertEqual(audit.check(report, [row], date(2026, 11, 14)), {finding})
        with self.assertRaises(ValueError):
            audit.check(report, [row], date(2026, 12, 2))

    def test_scan_settings_cannot_suppress_findings(self):
        settings = {'ignore': [], 'severity': None, 'target_arch': [], 'target_os': [],
                    'informational_warnings': ['unmaintained', 'unsound', 'notice']}
        audit.check_settings(settings)
        for key, value in [('ignore', ['RUSTSEC-2026-0001']), ('severity', 'high'),
                           ('target_os', ['linux']), ('target_arch', ['x86_64']),
                           ('informational_warnings', [])]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                audit.check_settings({**settings, key: value})

    def test_warning_and_yank_are_findings(self):
        report = {'vulnerabilities': {'list': []}, 'warnings': {'yanked': [{'advisory': None, 'package': {'name': 'example', 'version': '1'}}]}}
        self.assertEqual(audit.identities(report), {'yanked:example:1'})


if __name__ == '__main__':
    unittest.main()
