#!/usr/bin/env python3
"""Source-qualified, positive-only release evidence. Does not publish or scan."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT / "docs/archive/2026/replay-0.52.0-qualification"
PROFILE = "replay-scoped-positive-v1"
QUALIFIED = "1e49f1c63728865b294795684e38a27e48258b5a"
BASELINE = "62fe2ddde77c49092fb70644af37a92e8fb31f06"
HOST = "x86_64-unknown-linux-gnu"
COMPILERS = {
    "1.97.1": "8bab26f4f68e0e26f0bb7960be334d5b520ea452",
    "1.98.1": "48a229ceaefd4985c50990b14116b6d856af0985",
}
GATES = ["replay-scoped-integrity", "replay-scoped-positive"]
EXCLUDED_JOBS = [
    "workspace-integrity", "neutral-packages", "s3-security", "orm",
    "orm-security-sqlite", "orm-security-portable", "sqlserver-security",
    "ai-provider-matrix", "ai-provider-lanes", "ai-graphql-naming",
    "ai-postgres-parity", "alternate-features", "router", "router-msrv",
]
UNEXECUTED = [
    "wrong-recipe/compiler/platform/wrapper and unsupported-capture refusal",
    "malformed/tampered/duplicate/unknown/missing-member and resource refusal",
    "sensitive-disclosure, failed-live-check and apply-authority scenarios",
    "negative authorization, fault/cancellation and security concurrency",
    "fuzz/property attacks, penetration/red-team and broad mixed suites",
    "new dependency audit or advisory refresh",
    "full release-manifest and router-notice security/refusal dispatchers",
    "full workspace/companion/provider/router runtime matrix at release source",
]


def require(condition, message):
    if not condition:
        raise SystemExit(f"replay release: {message}")


def sha(data):
    return hashlib.sha256(data).hexdigest()


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT).decode().strip()


def source(ref, path):
    return subprocess.check_output(["git", "show", f"{ref}:{path}"], cwd=ROOT)


def write_json(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n")


def read_archive(name, expected):
    data = (EVIDENCE / name).read_bytes()
    require(sha(data) == expected, "retained archive digest changed")
    # Read only regular, explicitly checksummed members; never extract paths.
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        members = archive.getmembers()
        require(len(members) <= 200 and sum(m.size for m in members) <= 8_388_608,
                "retained archive bounds changed")
        require(all(m.isfile() and m.name and not m.name.startswith("/")
                    and ".." not in Path(m.name).parts for m in members),
                "retained archive member changed")
        require(len({m.name for m in members}) == len(members), "duplicate archive member")
        return {m.name: archive.extractfile(m).read() for m in members}


def reuse(ref):
    """Verify immutable inputs; retain original sources/counts rather than reseal."""
    commit = git("rev-parse", f"{ref}^{{commit}}")
    require(commit == git("rev-parse", "HEAD"), "selected source must be checked out")
    subprocess.run(["git", "merge-base", "--is-ancestor", QUALIFIED, commit],
                   cwd=ROOT, check=True)
    protected = ["crates", "Cargo.toml", "Cargo.lock", "release.toml",
                 "scripts/check-owned-planning-replay-source.py"]
    # Preserve all crate code, build admission, fixtures, manifests and vendor
    # source, plus all existing compiler/Cargo configuration, as exact Git blobs.
    for name in git("ls-tree", "--name-only", QUALIFIED).splitlines():
        if name.startswith("rust-toolchain") or name == ".cargo":
            protected.append(name)
    identities = {}
    for path in protected:
        old, new = git("rev-parse", f"{QUALIFIED}:{path}"), git("rev-parse", f"{commit}:{path}")
        require(old == new, f"qualified source changed: {path}")
        # Also catch unstaged/staged source edits during local verification.
        require(not git("diff", "HEAD", "--", path), f"working source changed: {path}")
        identities[path] = new
    allowed_scripts = {"scripts/replay_release.py", "scripts/run-replay-release-positive.py",
                       "scripts/check-replay-release-positive.py", "scripts/generate-release-manifest.py"}
    for path in git("diff", "--name-only", QUALIFIED, commit).splitlines():
        require(path.startswith("docs/") or path in allowed_scripts
                or path in {".github/workflows/release.yml", "CHANGELOG.md", "MIGRATION.md"},
                f"change outside scoped release policy: {path}")
    config = json.loads((EVIDENCE / "provenance.json").read_text())
    require(config["qualificationSource"] == QUALIFIED
            and config["publishedBaseline"] == BASELINE and config["profile"] == PROFILE,
            "qualification identity changed")
    packet = read_archive("qualification.tar.gz", config["archives"]["qualification.tar.gz"])
    sums = packet["SHA256SUMS"]
    require(sha(sums) == "0fdb2d7e484a54a446286a87620bb75cac12fac38e4ae63174a487bf307ce6b0",
            "original qualification checksum record changed")
    checksums = dict((line.split(maxsplit=1)[1].lstrip("* "), line.split()[0])
                     for line in sums.decode().splitlines())
    require(len(checksums) == 154 and len(packet) - 1 == 109,
            "qualification subset changed")
    for name, data in packet.items():
        if name != "SHA256SUMS":
            require(sha(data) == checksums[name], "original qualification member changed")
    summary = json.loads(packet["verification-summary.json"])
    matrix = json.loads(packet["matrix-results.json"])
    require(summary["source_commit"] == QUALIFIED and len(matrix) == 82
            and all(row["exit"] == 0 for row in matrix), "original matrix evidence changed")
    for compiler in ("1971", "1981"):
        results = json.loads(packet[f"qualified{compiler}-results.json"])
        require(len(results) == 3 and all(row["exit"] == 0 and row["source_commit"] == QUALIFIED
                                         for row in results), "final qualification evidence changed")
    for lane in ("off", "on"):
        log = packet[f"semver-minor-{lane}.log"].decode()
        require(re.search(r"196 checks: 196 pass, 57 skip", log), "native SemVer count evidence changed")
    failure = json.loads(packet["supplementary-mssql-test-target-limitation.json"])
    require(failure["exit"] == 101, "retained MSSQL failure changed")
    audit = read_archive("audit.tar.gz", config["archives"]["audit.tar.gz"])
    require(set(audit) == set(config["auditArchiveMembers"]), "audit members changed")
    for name, digest in config["auditArchiveMembers"].items():
        require(sha(audit[name]) == digest, "retained unsuppressed audit member changed")
    provenance = json.loads(audit["dependency-audit-provenance.json"])
    reconciliation = json.loads(audit["dependency-reconciliation.json"])
    require(provenance["source"] == BASELINE and provenance["exitCode"] == 1
            and provenance["advisoryDatabaseCommit"] == "999ec89d5d3f10f20be14750ec8c2d8b44e0ac9b"
            and reconciliation["auditExitCode"] == 1, "audit identity/findings changed")
    require(sha(audit["Cargo.lock.release"]) == provenance["lockSha256"], "audit lock changed")
    old_lock = tomllib.loads(audit["Cargo.lock.release"].decode())
    new_lock = tomllib.loads(source(commit, "Cargo.lock").decode())
    for lock in (old_lock, new_lock):
        for package in lock["package"]:
            if package["name"] in {"graphql-orm", "graphql-orm-macros"}:
                package["version"] = "aligned-orm-version"
    require(old_lock == new_lock, "external locked identities or graph changed")
    for path in reconciliation["conditionSourceFilesByteIdentical"]:
        require(source(BASELINE, path) == source(commit, path), "audit applicability source changed")
    require(audit["dependency-security-disposition.md"] == source(commit, "docs/reference/dependency-security-disposition.md"),
            "audit disposition changed")
    # A new optional replay API has no dependency edges; do not infer features
    # from lock identity alone. Compare the released graph and full vendor tree.
    for name in ["graphql-orm", "graphql-orm-macros"]:
        path = f"crates/{name}/Cargo.toml"
        old = tomllib.loads(source(BASELINE, path).decode())
        new = tomllib.loads(source(commit, path).decode())
        if name == "graphql-orm":
            # This internal path/version pair advances with the aligned ORM;
            # preserve its path, flags and every other graph property exactly.
            dep = "graphql-orm-macros"
            require(old["dependencies"][dep]["version"] == "0.51.0"
                    and new["dependencies"][dep]["version"] == "0.52.0",
                    "aligned macro dependency version changed")
            old["dependencies"][dep]["version"] = "aligned-orm-version"
            new["dependencies"][dep]["version"] = "aligned-orm-version"
        for key in ["features", "dependencies", "dev-dependencies", "build-dependencies", "target"]:
            require(old.get(key) == new.get(key), "audited feature/dependency configuration changed")
    for name in git("ls-tree", "--name-only", f"{BASELINE}:crates").splitlines():
        if name not in {"graphql-orm", "graphql-orm-macros"}:
            path = f"crates/{name}"
            require(git("rev-parse", f"{BASELINE}:{path}") == git("rev-parse", f"{commit}:{path}"),
                    "audited companion/vendor source changed")
    return {
        "formatVersion": 1, "profile": PROFILE, "sourceCommit": commit,
        "qualificationSource": QUALIFIED, "protectedSourceIdentities": identities,
        "originalProducerSource": config["originalProducerSource"],
        "archiveSha256": config["archives"], "publishedQualificationFiles": 109,
        "originalQualificationChecksumFileSha256": sha(sums),
        "originalQualificationFiles": 154,
        "omittedPrivateOperationalRecords": config["omittedQualificationRecords"],
        "reusedQualification": summary,
        "reuseMeaning": "original source-qualified results; not fresh execution at release source",
        "audit": {"provenance": provenance, "reconciliation": reconciliation,
                  "runtimeScanExecuted": False, "newAdvisoriesAssessed": False,
                  "coverageGap": "path/Git/vendor SMB not assessed by ordinary cargo-audit; retained dispositions are conditional",
                  "ageGap": "advisories frozen at recorded snapshot; no current coverage or clean graph claim"},
        "retainedMssqlFailure": failure,
        "unexecutedObligations": UNEXECUTED,
    }


def commands(compiler):
    """Finite literal templates; never execute commands from evidence/input."""
    require(compiler in COMPILERS, "unknown compiler lane")
    cargo = ["cargo", f"+{compiler}"]
    common = ["-p", "graphql-orm", "--locked", "--no-default-features"]
    result = []
    def add(label, args, tests=None, env=None):
        result.append({"label": label, "command": args, "expectedTests": tests, "environment": env or {}})
    add("recipe-source", ["python3", "scripts/check-owned-planning-replay-source.py"])
    for backend in ["sqlite", "postgres", "mssql", "sqlite,mssql", "sqlite,postgres"]:
        for replay in [False, True]:
            features = backend + (",owned-planning-replay" if replay else "")
            add(f"build-{features}", cargo + ["check", *common, "--features", features, "--lib"])
    for backend in ["sqlite", "postgres", "mssql"]:
        for replay in [False, True]:
            features = backend + (",owned-planning-replay" if replay else "")
            add(f"clippy-{features}", cargo + ["clippy", *common, "--features", features, "--lib", "--", "-D", "warnings"])
            add(f"rustdoc-{features}", cargo + ["doc", *common, "--features", features, "--lib", "--no-deps"], env={"RUSTDOCFLAGS": "-D warnings"})
            add(f"tree-{features}", cargo + ["tree", *common, "--features", features, "-e", "features"])
            add(f"api-{features}", cargo + ["test", "--manifest-path", "crates/graphql-orm/tests/fixtures/migration-api-compatibility/Cargo.toml", "--locked", "--no-default-features", "--features", backend + (",replay" if replay else ""), "--lib"], 3 if replay else 1)
    for backend in ["sqlite", "postgres"]:
        add(f"positive-{backend}", cargo + ["test", *common, "--features", backend + ",owned-planning-replay", "--lib", "replay_positive"], 2)
    add("mssql-unit-compile", cargo + ["test", *common, "--features", "mssql,owned-planning-replay", "--lib", "--no-run"])
    return result


def check_lane(record, compiler, commit):
    require(record["sourceCommit"] == commit and record["compiler"] == compiler
            and record["compilerCommit"] == COMPILERS[compiler]
            and record["host"] == HOST and record["target"] == HOST,
            "positive lane source/compiler changed")
    expected = commands(compiler)
    require(len(record["results"]) == len(expected), "positive commands missing")
    for actual, spec in zip(record["results"], expected):
        require(all(actual.get(k) == v for k, v in spec.items()) and actual["exit"] == 0,
                "positive command did not succeed")
        if spec["expectedTests"] is not None:
            require(actual["passedTests"] == spec["expectedTests"], "selected positive tests missing")
    require(len(record["treeEquality"]) == 3 and all(record["treeEquality"].values()),
            "feature dependency tree mismatch")


def metadata_commands():
    return [{"label": label, "command": command, "expectedTests": None, "environment": {}}
            for label, command in [
                ("documentation", ["python3", "scripts/check-documentation.py", "--base", QUALIFIED]),
                ("inventory", ["python3", "scripts/generate-workspace-inventory.py", "--check"]),
                ("release-state", ["python3", "scripts/check-release-state.py"]),
                ("dependencies", ["bash", "scripts/check-workspace-dependencies.sh"]),
                ("package-policy", ["bash", "scripts/check-package-release-policy.sh", QUALIFIED]),
                ("format", ["cargo", "+1.97.1", "fmt", "--all", "--", "--check"]),
                ("diff", ["git", "diff", QUALIFIED, "--check"]),
                ("positive-policy", ["python3", "scripts/check-replay-release-positive.py"]),
            ]]


def bundle(results, release_id, approval, output):
    require(bool(approval.strip()), "publication approval reference is required")
    require(re.fullmatch(r"workspace-\d{4}\.\d{2}\.\d{2}\.[1-9]\d*", release_id),
            "invalid release identity")
    try:
        datetime.strptime(release_id.removeprefix("workspace-").rsplit(".", 1)[0], "%Y.%m.%d")
    except ValueError:
        raise SystemExit("replay release: invalid release calendar date") from None
    current = reuse("HEAD")
    commit = current["sourceCommit"]
    reused = json.loads((results / "replay-scoped-integrity/reuse.json").read_text())
    require(current == reused, "integrity lane evidence changed")
    for compiler in COMPILERS:
        lane = json.loads((results / f"replay-scoped-{compiler}" / "results.json").read_text())
        check_lane(lane, compiler, commit)
        for row in lane["results"]:
            log = results / f"replay-scoped-{compiler}" / row["logFile"]
            require(sha(log.read_bytes()) == row["logSha256"], "positive log digest changed")
    metadata = json.loads((results / "replay-scoped-integrity/metadata.json").read_text())
    require(metadata["sourceCommit"] == commit and metadata["exit"] == 0,
            "metadata lane did not succeed")
    require(len(metadata["results"]) == len(metadata_commands()), "metadata commands missing")
    for row, expected in zip(metadata["results"], metadata_commands()):
        require(all(row.get(k) == v for k, v in expected.items()) and row["exit"] == 0,
                "metadata command did not succeed")
        require(sha((results / "replay-scoped-integrity" / row["logFile"]).read_bytes()) == row["logSha256"],
                "metadata log digest changed")
    # The validator also checks actual GitHub needs. No excluded lane is a
    # dependency here or converted to a passed test in this record.
    record = {
        "formatVersion": 1, "profile": PROFILE, "releaseId": release_id,
        "sourceCommit": commit, "approvalReference": approval,
        "workflowRun": os.environ.get("GITHUB_RUN_ID"),
        "workflowRunAttempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
        "recordedAtUtc": datetime.now(timezone.utc).isoformat(),
        "fullReleaseMatrixExecuted": False,
        "mandatoryGates": {gate: "success" for gate in GATES},
        "excludedGates": {gate: "unexecuted" for gate in EXCLUDED_JOBS},
        "unexecutedObligations": UNEXECUTED,
        "reusedEvidence": current,
        "freshPositiveLanes": {compiler: json.loads((results / f"replay-scoped-{compiler}" / "results.json").read_text()) for compiler in COMPILERS},
        "metadataGate": metadata,
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    for name in ["qualification.tar.gz", "audit.tar.gz"]:
        shutil.copyfile(EVIDENCE / name, output.parent / f"{release_id}-{name}")
    # Publish all fresh positive logs and records as deterministic data, not an
    # executable dispatcher. No retained producer or witness is rewritten.
    dest = output.parent / f"{release_id}-positive-results.tar.gz"
    with dest.open("wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as gz, tarfile.open(fileobj=gz, mode="w") as archive:
        for file in sorted(results.rglob("*")):
            if file.is_file():
                data = file.read_bytes()
                info = tarfile.TarInfo(file.relative_to(results).as_posix())
                info.size, info.mode = len(data), 0o644
                archive.addfile(info, io.BytesIO(data))
    record["evidenceAssets"] = {file.name: sha(file.read_bytes()) for file in sorted(output.parent.glob(f"{release_id}-*.tar.gz"))}
    scanned = datetime.fromisoformat(current["audit"]["provenance"]["timeUtc"])
    record["auditSnapshotAgeSeconds"] = int((datetime.now(timezone.utc) - scanned).total_seconds())
    write_json(output, record)
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    check = sub.add_parser("reuse")
    check.add_argument("--ref", default="HEAD")
    check.add_argument("--output", required=True, type=Path)
    publish = sub.add_parser("bundle")
    publish.add_argument("--results", required=True, type=Path)
    publish.add_argument("--release-id", required=True)
    publish.add_argument("--approval-reference", required=True)
    publish.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if args.operation == "reuse":
        write_json(args.output, reuse(args.ref))
    else:
        bundle(args.results, args.release_id, args.approval_reference, args.output)


if __name__ == "__main__":
    main()
