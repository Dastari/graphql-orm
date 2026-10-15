"""Owned-process harness regressions; fake Docker/Cargo never contact a daemon."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

RUNNER = Path(__file__).with_name("run.sh")
FAKE_DOCKER = r'''
import json, os, sys
from pathlib import Path
root = Path(os.environ['FAKE_ROOT'])
args = sys.argv[1:]
with (root / 'calls').open('a') as log:
    log.write(json.dumps(args) + '\n')
state_path = root / 'state'
state = json.loads(state_path.read_text()) if state_path.exists() else {'containers': {}, 'volume': None}
mode = os.environ.get('FAKE_MODE', '')
def save(): state_path.write_text(json.dumps(state))
def value(flag): return args[args.index(flag) + 1]
if args[0] == 'inspect':
    if '--type' in args: sys.exit(0)
    if '--format' in args:
        item = state['containers'].get(args[-1])
        if not item: sys.exit(1)
        print('foreign-owner' if mode == 'ownership_mismatch' and args[-1] == 'immutable-fixture-id-1' else item['owner']); sys.exit(0)
    sys.exit(0 if mode == 'container_collision' else 1)
if args[:2] == ['volume', 'inspect']:
    if mode == 'volume_collision': sys.exit(0)
    if not state['volume']: sys.exit(1)
    if '--format' in args: print(state['volume']['owner'])
    sys.exit(0)
if args[:2] == ['volume', 'create']:
    owner = value('--label').split('=', 1)[1]
    state['volume'] = {'name': args[-1], 'owner': owner}
    save(); print(args[-1]); sys.exit(0)
if args[:2] == ['volume', 'rm']:
    assert args[-1] == state['volume']['name']
    state['volume'] = None
    save(); sys.exit(0)
if args[0] == 'run':
    ident = 'immutable-fixture-id-' + str(len(state['containers']) + 1)
    owner = value('--label').split('=', 1)[1]
    state['containers'][ident] = {'name': value('--name'), 'owner': owner}
    save()
    if mode == 'lost_create_response' and len(state['containers']) == 2: sys.exit(1)
    print(ident); sys.exit(0)
if args[0] == 'ps':
    owner = value('--filter').split('=', 2)[2]
    for ident, item in state['containers'].items():
        if item['owner'] == owner: print(ident)
    sys.exit(0)
if args[0] == 'rm':
    ident = args[-1]
    assert ident.startswith('immutable-fixture-id-'), 'must delete captured/discovered immutable IDs'
    del state['containers'][ident]
    save(); sys.exit(0)
if args[0] == 'port':
    assert args[1] in state['containers']
    print('127.0.0.1:31445'); sys.exit(0)
if args[0] == 'exec': sys.exit(1)  # no leftover upload file
raise AssertionError('Unexpected Docker invocation: ' + repr(args))
'''
FAKE_CARGO = r'''
import json, os, sys
from pathlib import Path
with (Path(os.environ['FAKE_ROOT']) / 'cargo').open('a') as log:
    log.write(json.dumps({'args': sys.argv[1:], 'port': os.environ.get('SMB_TEST_PORT'),
                         'container': os.environ.get('SMB_TEST_CONTAINER_NAME')}) + '\n')
'''


class FixtureOwnershipTests(unittest.TestCase):
    def run_fixture(self, mode=""):
        with tempfile.TemporaryDirectory(prefix="owned-samba-harness-") as temporary:
            root = Path(temporary)
            for name, source in [("docker", FAKE_DOCKER), ("cargo", FAKE_CARGO)]:
                path = root / name
                path.write_text("#!" + sys.executable + "\n" + source)
                path.chmod(0o700)
            # Only skip socket readiness: fake Docker has no network service.
            # UUID generation and port parsing still use the real interpreter.
            python = root / "python3"
            python.write_text('#!/bin/sh\nif [ "$1" = "-" ]; then cat >/dev/null; exit 0; fi\n'
                              + 'exec ' + repr(sys.executable) + ' "$@"\n')
            python.chmod(0o700)
            environment = {"PATH": str(root) + os.pathsep + os.defpath,
                           "HOME": temporary, "FAKE_ROOT": temporary, "FAKE_MODE": mode}
            result = subprocess.run(["bash", str(RUNNER)], env=environment,
                                    text=True, capture_output=True, timeout=20)
            calls = [json.loads(line) for line in (root / "calls").read_text().splitlines()]
            state = json.loads((root / "state").read_text()) if (root / "state").exists() else None
            cargo = [json.loads(line) for line in (root / "cargo").read_text().splitlines()] if (root / "cargo").exists() else []
            return result, calls, state, cargo

    def test_startup_collisions_fail_before_create_or_delete(self):
        for mode in ["container_collision", "volume_collision"]:
            with self.subTest(mode=mode):
                result, calls, state, cargo = self.run_fixture(mode)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(any(args[0] in {"run", "rm", "ps"} or
                                     args[:2] in [["volume", "create"], ["volume", "rm"]]
                                     for args in calls), calls)
                self.assertIsNone(state)
                self.assertEqual(cargo, [])

    def test_success_preserves_seven_cases_and_deletes_only_owned_ids(self):
        result, calls, state, cargo = self.run_fixture()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(cargo), 7)
        self.assertTrue(all(item["port"] == "31445" for item in cargo))
        self.assertEqual(cargo[-1]["container"], "immutable-fixture-id-1")
        runs = [args for args in calls if args[0] == "run"]
        self.assertEqual(len(runs), 3)
        self.assertTrue(all("127.0.0.1::445" in args and "--pull=never" in args for args in runs))
        self.assertEqual(state, {"containers": {}, "volume": None})
        removals = [args[-1] for args in calls if args[0] == "rm"]
        self.assertEqual(set(removals), {"immutable-fixture-id-1", "immutable-fixture-id-2", "immutable-fixture-id-3"})

    def test_changed_ownership_is_not_deleted(self):
        result, calls, state, cargo = self.run_fixture("ownership_mismatch")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(cargo), 7)
        removals = [args[-1] for args in calls if args[0] == "rm"]
        self.assertNotIn("immutable-fixture-id-1", removals)
        self.assertEqual(set(state["containers"]), {"immutable-fixture-id-1"})
        self.assertIn("ownership mismatch", result.stderr)

    def test_lost_create_response_discovers_owned_id_and_cleans_up(self):
        result, calls, state, cargo = self.run_fixture("lost_create_response")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(cargo, [])
        self.assertEqual(state, {"containers": {}, "volume": None})
        removals = [args[-1] for args in calls if args[0] == "rm"]
        self.assertEqual(set(removals), {"immutable-fixture-id-1", "immutable-fixture-id-2"})
        # Every mutation is preceded by an exact ownership inspection.
        for index, args in enumerate(calls):
            if args[0] == "rm":
                self.assertTrue(any(previous[0] == "inspect" and "--format" in previous
                                    and previous[-1] == args[-1] for previous in calls[:index]))


if __name__ == "__main__":
    unittest.main()
