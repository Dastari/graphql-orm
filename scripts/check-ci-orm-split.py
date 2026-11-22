#!/usr/bin/env python3
"""Require CI's orm job to skip exactly the tests that orm-fixtures runs.

The orm job runs the default graphql-orm suite with `--exact --skip` for the
fixture runner tests, and orm-fixtures runs those runner binaries. This check
keeps the two in step, so a skip can never hide a test that no job runs:

- every skipped name is a non-ignored test of a binary orm-fixtures runs;
- every non-ignored test of those binaries is skipped;
- no test outside those binaries shares a skipped name.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TESTS = ROOT / "crates/graphql-orm/tests"
TEST_FN = re.compile(
    r"#\[(?:tokio::)?test(?:\([^)]*\))?\]\s*((?:#\[[^\]]*\]\s*)*)(?:async\s+)?fn\s+([a-z0-9_]+)"
)


def fail(message):
    sys.exit(f"ci-orm-split: {message}")


def job(workflow, name):
    match = re.search(rf"(?ms)^  {re.escape(name)}:\n(.*?)(?=^  [a-z][a-z0-9-]*:\n|\Z)", workflow)
    if not match:
        fail(f"job {name} is missing")
    return match[1]


def tests_of(path):
    """(name, ignored) for each test function in one integration test file."""
    return [(name, "#[ignore" in attributes) for attributes, name in TEST_FN.findall(path.read_text())]


def tests_of_binary(path):
    yield from tests_of(path)
    if path.name == "main.rs":
        # Grouped integration targets declare sibling files with `mod name;`.
        # Shared support files use #[path] and do not define fixture runner tests.
        for module in re.findall(r"(?m)^mod ([a-z0-9_]+);$", path.read_text()):
            source = path.with_name(f"{module}.rs")
            if not source.is_file():
                continue
            for name, ignored in tests_of(source):
                yield f"{module}::{name}", ignored


def main():
    workflow = (ROOT / ".github/workflows/ci.yml").read_text()
    orm = job(workflow, "orm")
    fixtures = job(workflow, "orm-fixtures")
    suite = re.search(r"cargo test -p graphql-orm -- --exact((?: --skip [a-z0-9_:]+)+)\n", orm)
    if not suite:
        fail("orm must run `cargo test -p graphql-orm -- --exact --skip ...`")
    skipped = re.findall(r"--skip ([a-z0-9_:]+)", suite[1])
    run = re.search(r"cargo test -p graphql-orm((?: --test [a-z0-9_]+)+)\n", fixtures)
    if not run:
        fail("orm-fixtures must run `cargo test -p graphql-orm --test ...`")
    binaries = re.findall(r"--test ([a-z0-9_]+)", run[1])

    targets = {path.stem: path for path in TESTS.glob("*.rs")}
    targets.update((path.parent.name, path) for path in TESTS.glob("*/main.rs"))
    expected = set()
    for binary in binaries:
        if binary not in targets:
            fail(f"orm-fixtures runs a missing test binary: {binary}")
        expected |= {name for name, ignored in tests_of_binary(targets[binary]) if not ignored}
    if len(skipped) != len(set(skipped)) or set(skipped) != expected:
        fail(f"orm skips {sorted(set(skipped) ^ expected)} differently from orm-fixtures")
    for binary, path in sorted(targets.items()):
        if binary in binaries:
            continue
        shared = {name for name, _ in tests_of_binary(path)} & expected
        if shared:
            fail(f"{binary} defines skipped test names {sorted(shared)}")
    print(f"CI orm split: {len(skipped)} fixture tests skipped by orm and run by orm-fixtures from {len(binaries)} binaries.")


if __name__ == "__main__":
    main()
