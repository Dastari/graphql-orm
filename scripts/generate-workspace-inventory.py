#!/usr/bin/env python3
"""Generate the documentation inventory sourced from Cargo metadata."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "docs/reference/workspace-packages.md"
BEGIN = "<!-- BEGIN GENERATED WORKSPACE PACKAGES -->"
END = "<!-- END GENERATED WORKSPACE PACKAGES -->"


def cargo_metadata() -> dict[str, object]:
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def repository_url(source: str) -> str:
    """Strip the `git+` prefix and any `?rev=`/`?tag=`/`#commit` suffix."""
    return source.removeprefix("git+").split("?", 1)[0].split("#", 1)[0]


def render(metadata: dict[str, object]) -> str:
    packages = metadata["packages"]
    workspace_members = set(metadata["workspace_members"])
    members = [package for package in packages if package["id"] in workspace_members]
    names = {package["name"] for package in members}
    members.sort(key=lambda package: package["name"])

    lines = [
        BEGIN,
        "",
        "Versions are not recorded here so version bumps do not conflict. Read each",
        "package version from its `Cargo.toml` and the released versions, external",
        "requirements and pinned revisions from the release manifest.",
        "",
        "| Package | Path | Default features | Direct internal dependencies |",
        "| --- | --- | --- | --- |",
    ]
    for package in members:
        manifest = Path(package["manifest_path"])
        path = manifest.parent.relative_to(ROOT).as_posix()
        defaults = package["features"].get("default", [])
        default_text = ", ".join(f"`{feature}`" for feature in defaults) or "none"
        internal = []
        for dependency in package["dependencies"]:
            if dependency["name"] not in names:
                continue
            label = f"`{dependency['name']}`"
            if dependency.get("kind") == "dev":
                label += " (dev-only)"
            elif dependency.get("optional"):
                label += " (optional)"
            internal.append((dependency["name"], label))
        dependency_text = ", ".join(label for _, label in sorted(internal)) or "none"
        lines.append(
            f"| `{package['name']}` | `{path}` | "
            f"{default_text} | {dependency_text} |"
        )

    auth_dependencies = []
    for package in members:
        for dependency in package["dependencies"]:
            if dependency["name"] == "agql-auth" and dependency.get("source"):
                # Requirement versions and pinned revisions change on release
                # bumps and live in Cargo.toml and the release manifest, so only
                # the stable repository identity is recorded here.
                auth_dependencies.append(
                    (repository_url(dependency["source"]), package["name"])
                )
    if auth_dependencies:
        url, _ = sorted(auth_dependencies)[0]
        consumers = sorted({entry[1] for entry in auth_dependencies})
        lines.extend(
            [
                "",
                "External Git dependency:",
                "",
                f"- `agql-auth`, source repository `{url}`, consumed by "
                + ", ".join(f"`{consumer}`" for consumer in consumers)
                + ".",
            ]
        )

    lines.extend(["", END])
    return "\n".join(lines)


def update_document(generated: str, check: bool) -> int:
    existing = OUTPUT.read_text(encoding="utf-8")
    if BEGIN not in existing or END not in existing:
        raise SystemExit(f"generated markers are missing from {OUTPUT.relative_to(ROOT)}")
    prefix, remainder = existing.split(BEGIN, 1)
    _, suffix = remainder.split(END, 1)
    expected = prefix + generated + suffix
    if existing == expected:
        return 0
    if check:
        # Preserve the canonical Cargo-derived result for review in CI logs.
        # Check mode still refuses the stale tracked file and never rewrites it.
        print(generated)
        print(
            "docs/reference/workspace-packages.md is stale; run "
            "python3 scripts/generate-workspace-inventory.py",
            file=sys.stderr,
        )
        print("Actual Cargo-derived generated block follows:", file=sys.stderr)
        print(generated, file=sys.stderr)
        return 1
    OUTPUT.write_text(expected, encoding="utf-8")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    return update_document(render(cargo_metadata()), args.check)


if __name__ == "__main__":
    raise SystemExit(main())
