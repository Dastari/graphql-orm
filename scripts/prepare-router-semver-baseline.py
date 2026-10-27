#!/usr/bin/env python3
"""Constrain only the known Hive 0.2.19 baseline telemetry dependency split.

Historical Rust source, package version and feature definitions are unchanged.
This is not a lockfile override: the baseline still resolves its own dependencies.
"""

import argparse
from pathlib import Path
import tomllib


CONSTRAINT = {"version": "=0.5.2", "default-features": False}
DEPENDENCY = "datadog-opentelemetry"


def prepare(root: Path) -> bool:
    manifest = root / "crates/graphql-orm-router/Cargo.toml"
    if not manifest.exists():
        print("router semver baseline: no historical router package")
        return False
    text = manifest.read_text()
    original = tomllib.loads(text)
    hive = original["dependencies"]["hive-router"]
    if isinstance(hive, dict) and hive.get("workspace") is True:
        workspace = tomllib.loads((root / "Cargo.toml").read_text())
        hive = workspace["workspace"]["dependencies"]["hive-router"]
    version = hive.get("version") if isinstance(hive, dict) else hive
    if version != "=0.2.19":
        print("router semver baseline: no Hive 0.2.19 coherence adjustment needed")
        return False
    existing = original["dependencies"].get(DEPENDENCY)
    if existing is not None:
        if existing != CONSTRAINT:
            raise ValueError("unexpected historical Datadog constraint; review the baseline")
        print("router semver baseline: Datadog 0.5.2 already constrained")
        return False
    addition = (
        "[dependencies]\n"
        "# Semver baseline only: Hive 0.2.19 requires Datadog's SDK 0.32 types.\n"
        'datadog-opentelemetry = { version = "=0.5.2", default-features = false }\n'
    )
    if text.count("[dependencies]\n") != 1:
        raise ValueError("unexpected historical dependency table; review the baseline")
    updated = text.replace("[dependencies]\n", addition, 1)
    expected = tomllib.loads(text)
    expected["dependencies"][DEPENDENCY] = CONSTRAINT
    if tomllib.loads(updated) != expected:
        raise ValueError("baseline adjustment changed more than the Datadog constraint")
    manifest.write_text(updated)
    print("router semver baseline: added only Datadog =0.5.2 (default features disabled); "
          "historical Rust source, version and features retained")
    return True


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline_root", type=Path)
    args = parser.parse_args()
    prepare(args.baseline_root)
