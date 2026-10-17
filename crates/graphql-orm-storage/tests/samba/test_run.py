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
    state['containers'][ident] = {'name': value('--name'), 'owner': owner, 'port': value('-p').split(':')[1]}
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
    print('127.0.0.1:' + state['containers'][args[1]]['port']); sys.exit(0)
if args[0] == 'exec':
    assert args[1] == 'immutable-fixture-id-1'
    assert args[2:] == ['find', '/share', '-path', '/share/.deleted', '-prune', '-o', '-type', 'f', '-name', '*.uploading', '-print', '-quit']
    if mode == 'exec_failure': sys.exit(37)
    if mode == 'find_failure': sys.exit(38)
    if mode == 'leftover_upload': print('/share/test.uploading')
    sys.exit(0)
raise AssertionError('Unexpected Docker invocation: ' + repr(args))
'''
FAKE_CARGO = r'''
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
mode = os.environ.get('FAKE_MODE', '')
with (Path(os.environ['FAKE_ROOT']) / 'cargo-calls.jsonl').open('a') as log:
    log.write(json.dumps({'args': args, 'executable': str(Path(sys.argv[0]).resolve()),
                         'port': os.environ.get('SMB_TEST_PORT'),
                         'container': os.environ.get('SMB_TEST_CONTAINER_NAME'),
                         'fixture_run': os.environ.get('SMB_TEST_FIXTURE_RUN')}) + '\n')
assert '--exact' in args and '--ignored' in args
if mode == 'cargo_failure': sys.exit(74)
name = args[args.index('--') - 1]
if mode == 'zero_tests':
    print('test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;')
elif mode == 'wrong_test':
    print('test unrelated_case ... ok')
    print('test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;')
else:
    if name == 'samba_reconnects_after_server_restart':
        print('test ' + name + ' ... server restart diagnostic\nok')
    else:
        print('test ' + name + ' ... ok')
    print('test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;')
'''


class FixtureOwnershipTests(unittest.TestCase):
    def run_fixture(self, mode="", absolute_cargo=False):
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
            if absolute_cargo:
                cargo_runner = root / "explicit runner" / "bounded cargo"
                cargo_runner.parent.mkdir()
                cargo_runner.write_text("#!" + sys.executable + "\n" + FAKE_CARGO)
                cargo_runner.chmod(0o700)
                environment["SMB_TEST_CARGO"] = str(cargo_runner)
                # A fallback to PATH must fail rather than satisfy this witness.
                (root / "cargo").write_text("#!" + sys.executable + "\nraise SystemExit(91)\n")
            result = subprocess.run(["bash", str(RUNNER)], env=environment,
                                    text=True, capture_output=True, timeout=20)
            calls = [json.loads(line) for line in (root / "calls").read_text().splitlines()]
            state = json.loads((root / "state").read_text()) if (root / "state").exists() else None
            cargo = [json.loads(line) for line in (root / "cargo-calls.jsonl").read_text().splitlines()] if (root / "cargo-calls.jsonl").exists() else []
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

    def test_success_preserves_seven_cases_adds_sdk_cases_and_deletes_only_owned_ids(self):
        result, calls, state, cargo = self.run_fixture()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(cargo), 9)
        self.assertTrue(all(item["port"].isdigit() for item in cargo))
        self.assertEqual(cargo[6]["container"], "immutable-fixture-id-1")
        self.assertTrue(all(item["container"] is None for item in cargo[7:]))
        runs = [args for args in calls if args[0] == "run"]
        self.assertEqual(len(runs), 3)
        mappings = [args[args.index("-p") + 1] for args in runs]
        self.assertTrue(all(mapping.startswith("127.0.0.1:") and mapping.endswith(":445")
                            and mapping.split(":")[1].isdigit() for mapping in mappings))
        self.assertEqual(len(set(mappings)), 3)
        self.assertTrue(all("--pull=never" in args for args in runs))
        expected_run = runs[0][runs[0].index("--label") + 1].split("=", 1)[1]
        self.assertTrue(all(item["fixture_run"] == expected_run for item in cargo[6:]))
        self.assertEqual(len(expected_run), 32)
        self.assertTrue(all(item["fixture_run"] is None for item in cargo[:6]))
        normal_port = mappings[0].split(":")[1]
        self.assertTrue(all(item["port"] == normal_port for item in cargo[6:]))
        expected_names = [
            "samba_rejects_invalid_password",
            "samba_round_trip_streaming_listing_and_atomic_create",
            "samba_oversized_chunk_cancellation_and_concurrency_matrix",
            "samba_round_trip_streaming_listing_and_atomic_create",
            "samba_constrained_max_write_handles_oversized_input_chunk",
            "full_smb_snapshot_lifecycle_and_locking",
            "samba_reconnects_after_server_restart",
            "sdk_directory_multibatch_eof_exact_names",
            "sdk_directory_drop_releases_directory_arc",
        ]
        self.assertEqual([item["args"][item["args"].index("--") - 1] for item in cargo], expected_names)
        self.assertEqual(state, {"containers": {}, "volume": None})
        removals = [args[-1] for args in calls if args[0] == "rm"]
        self.assertEqual(set(removals), {"immutable-fixture-id-1", "immutable-fixture-id-2", "immutable-fixture-id-3"})

    def test_inspection_errors_and_leftovers_fail_closed_and_cleanup(self):
        for mode, expected in [("exec_failure", 37), ("find_failure", 38), ("leftover_upload", 1)]:
            with self.subTest(mode=mode):
                result, calls, state, cargo = self.run_fixture(mode)
                self.assertEqual(result.returncode, expected, result.stderr)
                self.assertEqual(len(cargo), 3)
                self.assertEqual(state, {"containers": {}, "volume": None})

    def test_absolute_cargo_runner_preserves_all_nine_invocations(self):
        default_result, _, _, default_cargo = self.run_fixture()
        self.assertEqual(default_result.returncode, 0, default_result.stderr)
        result, _, state, cargo = self.run_fixture(absolute_cargo=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(cargo), 9)
        self.assertTrue(all(Path(item["executable"]).is_absolute() for item in cargo))
        self.assertTrue(all(item["executable"].endswith("/explicit runner/bounded cargo")
                            for item in cargo))
        self.assertEqual([item["args"] for item in cargo],
                         [item["args"] for item in default_cargo])
        self.assertEqual(state, {"containers": {}, "volume": None})

    def test_zero_or_wrong_native_test_and_cargo_failure_are_not_success(self):
        for mode, expected in [("zero_tests", 1), ("wrong_test", 1), ("cargo_failure", 74)]:
            with self.subTest(mode=mode):
                result, calls, state, cargo = self.run_fixture(mode)
                self.assertEqual(result.returncode, expected, result.stderr)
                self.assertEqual(len(cargo), 1)
                self.assertEqual(state, {"containers": {}, "volume": None})

    def test_changed_ownership_is_not_deleted(self):
        result, calls, state, cargo = self.run_fixture("ownership_mismatch")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(cargo), 9)
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
