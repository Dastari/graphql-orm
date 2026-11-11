#!/usr/bin/env python3
"""Positive source/manifest conformance only; never dispatch CI or refusal tests."""

import importlib.util
import json
import re
import types

from replay_release import (COMPILERS, EXCLUDED_JOBS, QUALIFIED, ROOT, commands,
                            metadata_commands, require, source)


def jobs(text):
    matches = list(re.finditer(r"(?m)^  ([a-z][a-z0-9-]*):\n", text))
    return {match[1]: text[match.start():matches[i + 1].start() if i + 1 < len(matches) else len(text)]
            for i, match in enumerate(matches) if match.start() > text.index("jobs:\n")}


def without_timeout(body):
    # A job's wall-clock limit is runner capacity, not verification content.
    return re.sub(r"(?m)^    timeout-minutes: [0-9]+\n", "", body)


# The qualified single ORM job now runs as these parallel jobs (#269).
ORM_JOBS = ["orm", "orm-runtime-graphql", "orm-backends"]
FIXTURE_MANIFEST = "--manifest-path crates/graphql-orm/tests/fixtures/"


def setup_steps(body):
    body = re.sub(r"(?m)^ *#.*\n", "", body)
    return body[body.index("    steps:\n"):body.index("      - name: ")]


def orm_commands(body):
    """Each command line of the job's run blocks, with explicit fixture output."""
    lines = []
    for block in re.findall(r"(?m)^        run: \|\n((?:          .*\n)+)", body):
        for line in block.splitlines():
            line = line.strip()
            # Fixture workspaces build into the shared fixture target directory
            # instead of the source tree; the verified command is unchanged.
            line = re.sub(r"(" + re.escape(FIXTURE_MANIFEST) + r"[^ ]+) --target-dir target/fixtures",
                          r"\1", line)
            lines.append(line)
    return lines


def check_orm_split(old, new):
    qualified = old["orm"]
    for name in ORM_JOBS:
        require(setup_steps(new[name]) == setup_steps(qualified), f"ORM job setup changed: {name}")
        require("    needs: guard\n" in new[name], f"ORM job dependency changed: {name}")
    split = [line for name in ORM_JOBS for line in orm_commands(new[name])]
    expected = orm_commands(qualified)
    # The SQLite migration API fixture command is byte-identical to the one the
    # default suite's migration_api_compatibility_fixture runner executes.
    expected[expected.index("for backend in sqlite postgres mssql; do")] = "for backend in postgres mssql; do"
    # Runtime search coverage was added after the qualified snapshot.
    expected = [line.replace("--test runtime_relations -- --test-threads=1",
                             "--test runtime_relations --test runtime_search_graphql --test runtime_search_writes -- --test-threads=1")
                .replace("--features postgres,runtime-graphql --lib --test runtime_graphql --test runtime_mutations -- --test-threads=1",
                         "--features postgres,runtime-graphql --lib --test runtime_graphql --test runtime_mutations --test runtime_search_graphql --test runtime_search_writes --test runtime_search_ir -- --test-threads=1")
                .replace("--test runtime_search_graphql", "--test runtime_live --test runtime_search_graphql")
                # Runtime operation assurance coverage was added after the qualified snapshot.
                .replace("--test runtime_search_writes -- --test-threads=1",
                         "--test runtime_search_writes --test runtime_operation_assurance -- --test-threads=1")
                # Runtime scalar read-back coverage was added after the qualified snapshot.
                .replace("--features sqlite,runtime-graphql --lib --test runtime_graphql --test runtime_mutations ",
                         "--features sqlite,runtime-graphql --lib --test runtime_graphql --test runtime_mutations --test runtime_scalar_readback ")
                .replace("--features postgres,runtime-graphql --lib --test runtime_graphql --test runtime_mutations ",
                         "--features postgres,runtime-graphql --lib --test runtime_graphql --test runtime_mutations --test runtime_scalar_readback ")
                for line in expected]
    # Backend target coverage is deliberately stronger than the qualified job.
    old_check = "cargo check -p graphql-orm --no-default-features --features postgres --locked"
    expected[expected.index(old_check):expected.index(old_check) + 1] = [
        "for backend in postgres mssql sqlite,mssql; do",
        'cargo check -p graphql-orm --locked --no-default-features --features "$backend" --all-targets',
        'cargo clippy -p graphql-orm -p graphql-orm-macros --locked --no-default-features --features "$backend" --all-targets -- -D warnings',
        "done",
    ]
    require(sorted(split) == sorted(expected), "ORM lane commands changed by the split")
    for line in [line for name in ORM_JOBS for line in new[name].splitlines()
                 if FIXTURE_MANIFEST in line and line.strip().startswith("cargo ")]:
        require("--target-dir target/fixtures" in line, "fixture command builds into the source tree")
    require("CARGO_BUILD_JOBS" not in new["orm"] and "CARGO_BUILD_JOBS" not in new["orm-backends"]
            and new["orm-runtime-graphql"].count('CARGO_BUILD_JOBS: "2"') == 1,
            "ORM lane build parallelism changed")


def main():
    workflow = (ROOT / ".github/workflows/release.yml").read_text()
    old = jobs(source(QUALIFIED, ".github/workflows/release.yml").decode())
    new = jobs(workflow)
    require("default: full\n" in workflow and "- replay-scoped-positive-v1\n" in workflow,
            "full profile must remain the default")
    # Every original matrix command, provider enumeration, failure/skip rule,
    # permission and full publisher step is byte-identical after selection.
    # Only the job-level timeout may differ, and the qualified ORM job is split
    # into parallel jobs that together run the same commands.
    for name in [*(job for job in EXCLUDED_JOBS if job not in ORM_JOBS), "release"]:
        body = new[name].replace("    if: inputs.validation_profile == 'full'\n", "", 1)
        if name == "orm-security-sqlite":
            current = """      - name: Require exact receiver-purpose admission
        run: bash scripts/run-security-test-targets.sh sqlite,receiver-admission receiver_admission
"""
            require(current in body, "receiver admission security gate is required")
            body = body.replace(current, "", 1)
        if name == "sqlserver-security":
            previous = """      - name: Verify TLS, writes, and retained query notifications on owned servers
        run: cargo test -p graphql-orm --locked --no-default-features --features mssql --test mssql_tls --test mssql_writes --test mssql_notifications -- --ignored --test-threads=1
"""
            current = """      - name: Execute the complete owned MSSQL lane
        env:
          GRAPHQL_ORM_TEST_MSSQL_PULL: missing
        run: scripts/run-owned-database-lanes.sh mssql
      - name: Verify TLS and retained query notifications on owned servers
        env:
          GRAPHQL_ORM_TEST_MSSQL_PULL: missing
        run: cargo test -p graphql-orm --locked --no-default-features --features mssql --test mssql_tls --test mssql_notifications -- --ignored --test-threads=1
"""
            require(current in body, "complete owned MSSQL execution is required")
            body = body.replace(current, previous, 1)
        require(without_timeout(body).rstrip() == without_timeout(old[name]).rstrip(),
                f"full profile changed: {name}")
    check_orm_split(old, new)
    validate = new["validate"].replace("    if: inputs.validation_profile == 'full'\n", "", 1)
    lanes = re.findall(r"(?m)^      - ([a-z0-9-]+)\n", validate[validate.index("    needs:\n"):validate.index("    runs-on:")])
    require(sorted(lanes) == sorted(["guard", *EXCLUDED_JOBS]), "validation must need every full lane")
    require(validate[validate.index("    runs-on:"):].rstrip()
            == old["validate"][old["validate"].index("    runs-on:"):].rstrip(), "full profile changed: validate")
    for name in ["replay-scoped-integrity", "replay-scoped-positive", "replay-scoped-validate", "release-scoped"]:
        require("if: inputs.validation_profile == 'replay-scoped-positive-v1'" in new[name],
                "scoped selection must be explicit")
        require("always()" not in new[name] and "continue-on-error" not in new[name],
                "scoped validation must not bypass failures/skips")
    require("needs: [guard, replay-scoped-integrity, replay-scoped-positive]" in new["replay-scoped-validate"],
            "scoped mandatory dependencies changed")
    require("      - replay-scoped-validate\n" in new["release-scoped"]
            and "      - validate\n" in new["release"], "publisher validation boundary changed")
    for step in ["Create checksums", "Attest release assets", "Create annotated release tags", "Publish immutable workspace release"]:
        require(f"- name: {step}" in new["release-scoped"], "immutable publisher step missing")
    require("--verification-evidence" in new["release-scoped"]
            and "git push --atomic origin" in new["release-scoped"], "scoped evidence/atomic publication missing")
    require("environment: release" in new["guard"] and 'test "${INCLUDE_ROUTER}" = false' in new["guard"]
            and 'test "${GITHUB_SHA}" = "${TARGET_REF}"' in new["guard"], "protected source-only entry guard changed")
    for compiler in COMPILERS:
        specs = commands(compiler)
        require(len(specs) == 38, "literal positive command count changed")
        require(all("--all-features" not in spec["command"] for spec in specs), "unsupported workspace feature union")
        tests = [spec for spec in specs if "test" in spec["command"]]
        require(sum(spec["expectedTests"] or 0 for spec in tests) == 16,
                "positive test counts changed")
    literal = json.loads((ROOT / "docs/operations/release/replay-positive-commands.json").read_text())
    require(literal == {"metadata": metadata_commands(), **{c: commands(c) for c in COMPILERS}},
            "literal command reference differs from implementation")
    module_spec = importlib.util.spec_from_file_location("release_manifest", ROOT / "scripts/generate-release-manifest.py")
    module = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(module)
    first = module.build_manifest("workspace-2000.01.01.1", "HEAD", True)
    second = module.build_manifest("workspace-2000.01.01.1", "HEAD", True)
    require(first == second and module.render_notes(first) == module.render_notes(second),
            "positive manifest generation is not deterministic")
    require("verification" not in first, "default full manifest shape changed")
    original = types.ModuleType("original_release_manifest")
    original.__file__ = str(ROOT / "scripts/generate-release-manifest.py")
    exec(compile(source(QUALIFIED, "scripts/generate-release-manifest.py"), original.__file__, "exec"), original.__dict__)
    require(first == original.build_manifest("workspace-2000.01.01.1", "HEAD", True)
            and module.render_notes(first) == original.render_notes(first),
            "default full manifest or notes changed from released generator")
    contracts = first["contracts"]
    require(len({row["name"] for row in contracts}) == len(contracts)
            and contracts == sorted(contracts, key=lambda row: row["name"]), "contract order changed")
    replay = [row for row in contracts if row["name"].startswith("graphql-orm-owned-planning-replay-")]
    require(len(replay) == 3, "both explicit recipes and format must remain recorded")
    print("Positive release conformance: 15 full jobs preserved with explicit backend coverage additions and the ORM job split; 4 explicit scoped jobs; deterministic manifest and three replay contract rows.")


if __name__ == "__main__":
    main()
