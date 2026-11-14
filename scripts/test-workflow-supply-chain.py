#!/usr/bin/env python3
"""Regression tests for scripts/check-workflow-supply-chain.py.

Each test copies the files the checker reads into a temporary tree, reverts
one hardening rule, and requires the checker to report it. The unmodified copy
must pass, so every failure is caused by the single reverted rule.
"""

import importlib.util
import shutil
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("supply_chain", ROOT / "scripts/check-workflow-supply-chain.py")
supply_chain = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(supply_chain)

COPIED = [
    ".github",
    "compose.yaml",
    "scripts/check-schema-export.sh",
    "crates/graphql-orm-storage/tests/samba/run.sh",
]
COPIED += [str(p.relative_to(ROOT)) for p in supply_chain.test_source_files(ROOT)]
COPIED += [str(p.relative_to(ROOT)) for p in (ROOT / "crates").rglob("*")
           if p.is_file() and (p.name == "Dockerfile" or p.name.endswith(".Dockerfile") or p.name.startswith("Dockerfile."))]
CHECKOUT = "actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09 # v5"
TOOLCHAIN = "dtolnay/rust-toolchain@cf201f30d2291af293e25b32e435b13c42d51aaf # 1.97.1"
CACHE = "Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2"


class SupplyChainTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="workflow-supply-chain-")
        self.root = Path(self.directory.name)
        for name in COPIED:
            source, target = ROOT / name, self.root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            if source.is_dir():
                shutil.copytree(source, target)
            else:
                shutil.copy2(source, target)

    def tearDown(self):
        self.directory.cleanup()

    def edit(self, name, old, new, count=1):
        path = self.root / name
        text = path.read_text()
        self.assertGreaterEqual(text.count(old), count, f"{name} no longer contains {old!r}")
        path.write_text(text.replace(old, new, count))

    def messages(self):
        return supply_chain.check(self.root)[0]

    def assertReported(self, fragment):
        messages = self.messages()
        self.assertTrue(any(fragment in message for message in messages),
                        f"expected {fragment!r} in {messages}")

    def release_job(self, name):
        return supply_chain.jobs((self.root / ".github/workflows/release.yml").read_text())[name]

    def test_checkout_credentials_are_not_persisted(self):
        self.edit(".github/workflows/ci.yml", "persist-credentials: false", "persist-credentials: true")
        self.assertReported("checkout must disable persisted credentials")

    def test_publisher_tag_validation_is_required(self):
        self.edit(".github/workflows/release.yml", "test \"${package_tags}\" = \"${expected_tags}\"", ": # removed tag binding")
        self.assertReported("publisher must bind tags to workspace manifests")

    def test_floating_rust_postgres_image_is_rejected(self):
        path = self.root / "crates/example/tests/image.rs"
        path.parent.mkdir(parents=True)
        path.write_text('const IMAGE: &str = "postgres:17-alpine";')
        self.assertReported("Rust test image is not pinned by digest")

    def test_scoped_unexpected_manifest_tag_fails_before_tag_creation(self):
        self.test_unexpected_manifest_tag_fails_before_tag_creation("release-scoped", "target/scoped-dist")

    def test_unexpected_manifest_tag_fails_before_tag_creation(self, job_name="release", directory="release-dist"):
        # Run the actual publisher shell with a recording git stub; never push or tag.
        job = self.release_job(job_name)
        script = next(code for code in supply_chain.run_scripts(job) if "tags_to_push=()" in code)
        binary = self.root / "bin"
        binary.mkdir()
        git = binary / "git"
        git.write_text('#!/bin/bash\ncase "$*" in\n*show-ref*) if [[ ${EXISTING_RELEASE:-} == 1 ]]; then exit 0; fi; exit 1;;\n*origin/main*) echo \"${MAIN_SHA:-a}\";;\n*rev-parse*) echo a;;\ntag*) echo tagged >> "$TAG_LOG";;\nesac\n')
        git.chmod(0o755)
        tags = []
        for manifest in ROOT.glob("crates/*/Cargo.toml"):
            target = self.root / manifest.relative_to(ROOT)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(manifest.read_text())
            import tomllib
            package = tomllib.loads(manifest.read_text())["package"]
            tags.append({"tag": package["name"] + "-v" + package["version"]})
        dist = self.root / directory
        dist.mkdir(parents=True)
        manifest = dist / "workspace-2000.01.01.1.json"
        env = {**os.environ, "PATH": str(binary) + ":" + os.environ["PATH"],
               "RELEASE_ID": "workspace-2000.01.01.1", "GITHUB_SHA": "a",
               "GH_TOKEN": "test", "TAG_LOG": str(self.root / "tag.log")}
        for extra in ["other-v1.0.0", "--force", "graphql-orm-v999.0.0"]:
            manifest.write_text(json.dumps({"packages": tags + [{"tag": extra}]}))
            result = subprocess.run(["bash", "-c", script], cwd=self.root, env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((self.root / "tag.log").exists())
        manifest.write_text(json.dumps({"packages": tags}))
        for invalid in ["workspace-2000.01.01.1\n", "..", "refs/workspace-2000.01.01.1", "-workspace-2000.01.01.1"]:
            invalid_result = subprocess.run(["bash", "-c", script], cwd=self.root,
                                            env={**env, "RELEASE_ID": invalid}, capture_output=True)
            self.assertNotEqual(invalid_result.returncode, 0)
            self.assertFalse((self.root / "tag.log").exists())
        moved = subprocess.run(["bash", "-c", script], cwd=self.root,
                               env={**env, "MAIN_SHA": "b"}, capture_output=True)
        self.assertNotEqual(moved.returncode, 0)
        self.assertFalse((self.root / "tag.log").exists())
        existing = subprocess.run(["bash", "-c", script], cwd=self.root,
                                  env={**env, "EXISTING_RELEASE": "1"}, capture_output=True)
        self.assertNotEqual(existing.returncode, 0)
        self.assertFalse((self.root / "tag.log").exists())
        result = subprocess.run(["bash", "-c", script], cwd=self.root, env=env, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.root / "tag.log").exists())

    def test_other_test_images_and_dockerfile_names(self):
        for directory in ["crates/example/tests", "crates/example/src/test_support", "crates/example/test-support"]:
            path = self.root / directory / "images.rs"
            path.parent.mkdir(parents=True, exist_ok=True)
            for ref in ["docker.io/library/postgres:17-alpine", "redis:7", "mcr.microsoft.com/mssql/server:2022-latest", "registry.example:5000/team/image:v1"]:
                path.write_text('const IMAGE: &str = "' + ref + '";')
                self.assertReported("Rust test image is not pinned by digest")
            path.unlink()
        for extension in ['toml', 'yaml', 'json', 'js']:
            path = self.root / "crates/example/tests" / ('images.' + extension)
            path.write_text('"docker.io/library/redis:7"')
            self.assertReported("Rust test image is not pinned by digest")
            path.unlink()
        for name in ["test.Dockerfile", "Dockerfile.test"]:
            path = self.root / "crates/example/tests" / name
            path.write_text("FROM redis:7\n")
            self.assertReported("Dockerfile image is not pinned by digest")
            path.unlink()

    def test_manual_dispatch_is_separate_and_shares_the_postgres_action(self):
        ci = (self.root / '.github/workflows/ci.yml').read_text()
        self.assertNotIn('workflow_dispatch', ci)
        self.assertIn("if: github.event_name == 'push' && github.ref == 'refs/heads/main'",
                      supply_chain.jobs(ci)['owned-postgres-complete'])
        manual = (self.root / '.github/workflows/owned-postgres-manual.yml').read_text()
        self.assertIn('  workflow_dispatch:', manual)
        self.assertNotIn('  push:', manual)
        self.assertNotIn('  pull_request:', manual)
        self.assertIn('group: owned-postgres-manual-', manual)
        for file, name in [('ci.yml', 'owned-postgres-complete'),
                           ('owned-postgres-manual.yml', 'qualify-postgres'),
                           ('release.yml', 'owned-postgres-complete')]:
            body = supply_chain.jobs((self.root / '.github/workflows' / file).read_text())[name]
            self.assertIn('uses: ./.github/actions/owned-postgres-complete', body)
            self.assertIn('timeout-minutes: 120', body)
        self.edit('.github/workflows/owned-postgres-manual.yml',
                  './.github/actions/owned-postgres-complete', './.github/actions/other')
        self.assertReported('owned PostgreSQL caller must use the shared action')

    def test_untagged_container_run_image_is_rejected(self):
        path = self.root / 'crates/example/tests/container.rs'
        path.parent.mkdir(parents=True)
        for call in ['Command::new("docker").args(["run", "redis"]).status();',
                     'Command::new("podman").arg("run").arg("redis").status();',
                     'subprocess.run(["docker", "run", "redis"])',
                     "subprocess.run(['docker', 'run', 'redis'])",
                     'Command::new("docker").args(["run", "-d", "--name", "owned", "docker.io/library/redis"]).status();']:
            path.write_text(call)
            self.assertReported('container run image is not pinned by digest')
        path.write_text('Command::new("docker").args(["run", "redis@sha256:' + 'a' * 64 + '"]).status();')
        self.assertEqual(self.messages(), [])

    def test_credentials_are_masked_and_never_passed_in_git_argv(self):
        workflow = supply_chain.jobs((self.root / '.github/workflows/release.yml').read_text())
        count = 0
        for name, body in workflow.items():
            if 'GIT_CONFIG_COUNT:' not in body:
                continue
            count += 1
            self.assertEqual(body.count('- id: git-auth'), 1)
            self.assertIn('set -euo pipefail\n          encoded=$(', body)
            self.assertIn('echo "::add-mask::${encoded}"', body)
            self.assertIn('echo "::add-mask::${header}"', body)
            self.assertIn('GIT_CONFIG_KEY_0: http.https://github.com/.extraheader', body)
            self.assertNotIn('git -c ', body)
            self.assertNotIn('git config http.', body)
            # Only Git steps receive the config, not the later bundle compiler.
            for step in supply_chain.steps(body):
                if any('name: ' + name in step for name in
                       ['Restore release tag visibility', 'Create annotated release tags']):
                    self.assertNotIn('GH_TOKEN:', step)
                if 'GIT_CONFIG_COUNT:' in step:
                    self.assertTrue('git fetch' in step or 'git push' in step)
                    self.assertNotIn('cargo ', step)
        self.assertEqual(count, 5)

    def test_workspace_integrity_runs_both_contract_suites(self):
        ci = supply_chain.jobs((self.root / '.github/workflows/ci.yml').read_text())
        for script in ['test-release-workflow-contract.py', 'test-reviewed-cargo-tool.py']:
            self.assertIn('python3 scripts/' + script, ci['workspace-integrity'])

    def test_current_tree_passes(self):
        self.assertEqual(self.messages(), [])

    def test_mutable_action_tags_are_rejected(self):
        # The #492 regression: the schema-export job used floating tags.
        self.edit(".github/workflows/ci.yml", f"      - uses: {CHECKOUT}\n", "      - uses: actions/checkout@v4\n")
        self.assertReported("action is not pinned to a full commit SHA: actions/checkout@v4")

    def test_missing_version_comment_is_rejected(self):
        self.edit(".github/workflows/ci.yml", f"uses: {CACHE}", f"uses: {CACHE.split(' #')[0]}")
        self.assertReported("pinned action has no version comment")

    def test_floating_toolchains_are_rejected(self):
        self.edit(".github/workflows/ci.yml", f"uses: {TOOLCHAIN}", "uses: dtolnay/rust-toolchain@stable")
        self.assertReported("selects no reviewed toolchain")

    def test_floating_toolchain_input_is_rejected(self):
        self.edit(".github/workflows/ci.yml", "toolchain: 1.90.0", "toolchain: stable")
        self.assertReported("selects floating toolchain stable")

    def test_publisher_may_not_restore_caches_or_build(self):
        # The #493 regression: the publisher compiled with write permissions.
        verify = "      - name: Verify the bundle artifact identity\n"
        self.assertIn(verify, self.release_job("release"))
        self.edit(".github/workflows/release.yml", verify,
                  f"      - uses: {TOOLCHAIN}\n      - uses: {CACHE}\n"
                  "      - run: cargo build -p graphql-orm-router --release --locked\n" + verify)
        self.assertReported("release: job with write permissions uses dtolnay/rust-toolchain@")
        self.assertReported("release: job with write permissions uses Swatinem/rust-cache@")
        self.assertReported("release: job with write permissions runs cargo")

    def test_publisher_may_not_run_repository_scripts(self):
        verify = "      - name: Verify the bundle artifact identity\n"
        self.edit(".github/workflows/release.yml", verify,
                  "      - run: scripts/check-release-manifest.sh\n" + verify)
        self.assertReported("release: job with write permissions runs a repository script")

    def test_publisher_must_verify_the_bundle_digest(self):
        self.edit(".github/workflows/release.yml", "sha256sum --check --strict SHA256SUMS",
                  "sha256sum --check SHA256SUMS", count=2)
        self.assertReported("release: job with write permissions does not verify asset checksums")
        self.assertReported("release-scoped: job with write permissions does not verify asset checksums")

    def test_publisher_must_consume_a_read_only_build_output(self):
        publisher = self.release_job("release")
        self.edit(".github/workflows/release.yml", publisher,
                  publisher.replace("needs.release-bundle.outputs.", "needs.validate.outputs."))
        self.assertReported("release: job with write permissions consumes no digest from a build job")

    def test_bundle_job_may_not_hold_write_permissions(self):
        bundle = self.release_job("release-bundle")
        self.edit(".github/workflows/release.yml", bundle,
                  bundle.replace("    permissions:\n      contents: read\n",
                                 "    permissions:\n      contents: write\n", 1))
        self.assertReported("release: build job release-bundle holds write permissions")

    def test_workflow_permissions_must_be_read_only(self):
        self.edit(".github/workflows/ci.yml", "permissions:\n  contents: read\n",
                  "permissions:\n  contents: write\n")
        self.assertReported("ci.yml: workflow-level permissions must be explicit and read-only")

    def test_release_validation_must_run_schema_export(self):
        # The #494 regression: the macros tests ran only in CI.
        self.edit(".github/workflows/release.yml", "      - schema-export\n    runs-on", "    runs-on")
        self.assertReported("full release validation does not run scripts/check-schema-export.sh")

    def test_schema_export_script_must_run_locked_macros_tests(self):
        self.edit("scripts/check-schema-export.sh", "cargo test -p graphql-orm-macros --locked\n",
                  "cargo test -p graphql-orm-macros\n")
        self.assertReported("missing locked macros test")

    def test_samba_image_must_be_digest_pinned(self):
        # The #495 regression: the SMB fixture pulled a floating tag.
        path = self.root / "crates/graphql-orm-storage/tests/samba/run.sh"
        text = path.read_text()
        start = text.index("IMAGE=${SMB_TEST_IMAGE:-")
        end = text.index("}\n", start)
        path.write_text(text[:start] + "IMAGE=${SMB_TEST_IMAGE:-dperson/samba:latest" + text[end:])
        self.assertReported("default test image is not pinned by digest: dperson/samba:latest")

    def test_compose_image_ports_and_password_are_hardened(self):
        # The #500 regression: compose published fixed credentials on every interface.
        path = self.root / "compose.yaml"
        text = path.read_text()
        image = next(line for line in text.splitlines() if line.strip().startswith("image:"))
        password = next(line for line in text.splitlines() if line.strip().startswith("POSTGRES_PASSWORD:"))
        text = (text.replace(image, "    image: postgres:16-alpine")
                .replace('"127.0.0.1:55432:5432"', '"55432:5432"')
                .replace(password, "      POSTGRES_PASSWORD: postgres"))
        path.write_text(text)
        self.assertReported("image is not pinned by digest: postgres:16-alpine")
        self.assertReported("port is not bound to IPv4 loopback: 55432:5432")
        self.assertReported("POSTGRES_PASSWORD must come from a required environment variable")

    def test_job_container_images_must_be_digest_pinned(self):
        self.edit(".github/workflows/managed-smb.yml", "    runs-on: ubuntu-latest\n",
                  "    runs-on: ubuntu-latest\n    container: rust:1.97.1\n")
        self.assertReported("job container image is not pinned by digest: rust:1.97.1")


if __name__ == "__main__":
    unittest.main()
