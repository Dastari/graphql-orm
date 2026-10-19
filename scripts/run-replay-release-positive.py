#!/usr/bin/env python3
"""Run only the reviewed literal scoped-release commands; no publishing."""

import argparse
import os
from pathlib import Path
import re
import subprocess
import time

from replay_release import (COMPILERS, HOST, QUALIFIED, ROOT, commands, git,
                            metadata_commands, require, reuse, sha, write_json)


def run(spec, output, env):
    started = time.monotonic()
    log = output / (re.sub(r"[^a-zA-Z0-9.-]", "-", spec["label"]) + ".log")
    with log.open("wb") as handle:
        process = subprocess.run(spec["command"], cwd=ROOT,
                                 env={**env, **spec["environment"]},
                                 stdout=handle, stderr=subprocess.STDOUT)
    data = log.read_bytes()
    result = {**spec, "exit": process.returncode,
              "seconds": round(time.monotonic() - started, 2),
              "logFile": log.name, "logSha256": sha(data)}
    if spec["expectedTests"] is not None:
        counts = re.findall(rb"test result: ok\. (\d+) passed; 0 failed", data)
        result["passedTests"] = sum(map(int, counts))
        require(result["passedTests"] == spec["expectedTests"], "selected positive tests missing")
    require(process.returncode == 0, f"positive command failed: {spec['label']}; retain {log}")
    print(f"passed: {spec['label']}", flush=True)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("lane", choices=["metadata", *COMPILERS])
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    require(not output.exists(), "use a new output directory; retain previous results")
    output.mkdir(parents=True)
    env = dict(os.environ)
    record = {"sourceCommit": git("rev-parse", "HEAD"), "results": []}
    if args.lane == "metadata":
        write_json(output / "reuse.json", reuse("HEAD"))
        specs = metadata_commands()
    else:
        compiler = args.lane
        for key in ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTC", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"]:
            require(not env.get(key), "qualified runtime lane requires stock compiler settings")
        env["RUSTUP_TOOLCHAIN"] = compiler
        env["CARGO_BUILD_TARGET"] = HOST
        identity = subprocess.check_output(["rustup", "run", compiler, "rustc", "-vV"], env=env).decode()
        details = dict(line.split(": ", 1) for line in identity.splitlines() if ": " in line)
        require(details.get("release") == compiler and details.get("commit-hash") == COMPILERS[compiler]
                and details.get("host") == HOST, "exact qualified compiler required")
        (output / "rustc.txt").write_text(identity)
        record.update(compiler=compiler, compilerCommit=COMPILERS[compiler], host=HOST, target=HOST)
        specs = commands(compiler)
    try:
        for spec in specs:
            record["results"].append(run(spec, output, env))
            write_json(output / "results.json", record)
        if args.lane == "metadata":
            record["exit"] = 0
            write_json(output / "metadata.json", record)
        else:
            trees = {}
            for backend in ["sqlite", "postgres", "mssql"]:
                # Keep complete logs, but exclude Cargo's cache/download status
                # rows from graph equality. No dependency-tree row is normalized.
                def tree(replay):
                    label = "tree-" + backend + (",owned-planning-replay" if replay else "")
                    row = next(r for r in record["results"] if r["label"] == label)
                    return "\n".join(line for line in (output / row["logFile"]).read_text().splitlines()
                                     if "Blocking waiting for file lock" not in line
                                     and not re.fullmatch(
                                         r"\s*(?:Downloading crates \.\.\.|Downloaded \S+ v\S+)\s*",
                                         re.sub(r"\x1b\[[0-9;]*m", "", line)))
                trees[backend] = tree(False) == tree(True)
            require(all(trees.values()), "replay changed dependency graph")
            record["treeEquality"] = trees
            write_json(output / "results.json", record)
    except BaseException:
        write_json(output / "incomplete.json", record)
        raise


if __name__ == "__main__":
    main()
