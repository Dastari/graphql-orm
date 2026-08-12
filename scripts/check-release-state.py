#!/usr/bin/env python3
"""Check source-release invariants that are independent of a release ID."""

from __future__ import annotations

import json
from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
VERSION_RE = re.compile(r"^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$")


def main() -> int:
    metadata = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    members = set(metadata["workspace_members"])
    errors: list[str] = []
    for package in sorted(
        (package for package in metadata["packages"] if package["id"] in members),
        key=lambda package: package["name"],
    ):
        if not VERSION_RE.fullmatch(package["version"]):
            errors.append(f"{package['name']}: invalid package version {package['version']}")
        if package.get("publish") != []:
            errors.append(
                f"{package['name']}: Git-only workspace packages must set publish = false"
            )
        if package.get("repository") != "https://github.com/Dastari/graphql-orm":
            errors.append(f"{package['name']}: repository metadata is missing or inconsistent")
        if not package.get("description"):
            errors.append(f"{package['name']}: package description is required")
        if package.get("license") != "MIT":
            errors.append(f"{package['name']}: expected MIT package metadata")
    orm = next(package for package in metadata["packages"] if package["name"] == "graphql-orm")
    macros = next(
        package for package in metadata["packages"] if package["name"] == "graphql-orm-macros"
    )
    if orm["version"] != macros["version"]:
        errors.append("graphql-orm and graphql-orm-macros versions must remain aligned")
    if errors:
        print("Release-state validation failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print(f"Release-state validation passed for {len(members)} workspace packages.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
