#!/usr/bin/env python3
"""Validate first-party Markdown governance without third-party packages."""

from __future__ import annotations

import argparse
from datetime import date
import re
from pathlib import Path
import subprocess
import sys
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parents[1]
REQUIRED = (
    "title",
    "kind",
    "status",
    "owner",
    "last_reviewed",
    "review_by",
    "supersedes",
)
KINDS = {"architecture", "decision", "runbook", "plan", "investigation", "reference"}
STATUSES = {"draft", "active", "accepted", "superseded", "archived"}
DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
ADR_RE = re.compile(r"^docs/decisions/ADR-(\d{4})-[a-z0-9-]+\.md$")
LINK_RE = re.compile(r"(?<!!)\[[^\]]*\]\(([^)]+)\)")
FENCE_RE = re.compile(r"```.*?```|~~~.*?~~~", re.DOTALL)
HEADING_RE = re.compile(r"^ {0,3}#{1,6}\s+(.+?)\s*#*\s*$", re.MULTILINE)
STALE_PATTERNS = {
    "retired frontend path": re.compile(r"(?<![A-Za-z0-9_-])frontend/"),
    "old handoff directory": re.compile(r"\.handoffs/"),
    "old standalone checkout": re.compile(
        r"/home/[^/]+/dev/graphql-orm-(?:ai|backup|storage)(?:/|\b)"
    ),
    "old standalone repository": re.compile(
        r"https://github\.com/Dastari/graphql-orm-(?:ai|backup|storage)(?:\.git)?"
    ),
}
CONSUMER_SPECIFIC_RE = re.compile(r"\b(?:GEMA|FAME|JIM|Digitise)\b", re.IGNORECASE)
IMMUTABLE_HISTORICAL_EXCEPTIONS = {
    Path("docs/decisions/ADR-0007-seven-package-workspace-boundaries.md"),
}


def markdown_files() -> list[Path]:
    result = subprocess.run(
        [
            "git",
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "*.md",
        ],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    paths = []
    for raw in result.stdout.splitlines():
        path = Path(raw)
        if ".agents" in path.parts or path.as_posix() == ".github/PULL_REQUEST_TEMPLATE.md":
            continue
        if not (ROOT / path).is_file():
            continue
        paths.append(path)
    return sorted(set(paths))


def frontmatter(path: Path, text: str) -> tuple[dict[str, str], int] | None:
    lines = text.splitlines()
    if not lines or lines[0] != "---":
        return None
    try:
        closing = lines.index("---", 1)
    except ValueError:
        return None
    values: dict[str, str] = {}
    for line in lines[1:closing]:
        if not line or line[0].isspace() or ":" not in line:
            continue
        key, value = line.split(":", 1)
        values[key.strip()] = value.strip()
    return values, closing + 1


def validate_metadata(path: Path, text: str, errors: list[str]) -> dict[str, str] | None:
    parsed = frontmatter(path, text)
    if parsed is None:
        errors.append(f"{path}: missing or unterminated YAML frontmatter")
        return None
    values, _ = parsed
    for key in REQUIRED:
        if key not in values:
            errors.append(f"{path}: missing metadata key {key}")
    for key in ("title", "owner", "last_reviewed", "review_by"):
        if key in values and not values[key]:
            errors.append(f"{path}: metadata key {key} must not be empty")
    if values.get("kind") not in KINDS:
        errors.append(f"{path}: invalid kind {values.get('kind')!r}")
    if values.get("status") not in STATUSES:
        errors.append(f"{path}: invalid status {values.get('status')!r}")
    parsed_dates: dict[str, date] = {}
    for key in ("last_reviewed", "review_by"):
        value = values.get(key, "")
        if key == "review_by" and value == "none":
            continue
        if value and not DATE_RE.fullmatch(value):
            errors.append(f"{path}: {key} must be YYYY-MM-DD or review_by: none")
            continue
        if value:
            try:
                parsed_dates[key] = date.fromisoformat(value)
            except ValueError:
                errors.append(f"{path}: {key} is not a valid calendar date")
    review_by = values.get("review_by")
    if "review_by" in parsed_dates:
        if parsed_dates["review_by"] < date.today():
            errors.append(f"{path}: review_by {review_by} has expired")
        if (
            "last_reviewed" in parsed_dates
            and parsed_dates["review_by"] < parsed_dates["last_reviewed"]
        ):
            errors.append(f"{path}: review_by precedes last_reviewed")
    if values.get("status") == "archived" and review_by != "none":
        errors.append(f"{path}: archived documents must use review_by: none")
    if values.get("status") != "archived" and review_by == "none":
        errors.append(f"{path}: non-archived documents require a review date")
    if values.get("kind") == "plan" and values.get("status") == "active":
        if not re.fullmatch(r"docs/plans/active/[^/]+/README\.md", path.as_posix()):
            errors.append(f"{path}: active plans must be docs/plans/active/<initiative>/README.md")
    return values


def markdown_anchors(text: str) -> set[str]:
    without_fences = FENCE_RE.sub("", text)
    anchors: set[str] = set()
    counts: dict[str, int] = {}
    for match in HEADING_RE.finditer(without_fences):
        heading = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", match.group(1))
        heading = re.sub(r"<[^>]+>", "", heading).replace("`", "").lower()
        slug = re.sub(r"[^\w\- ]", "", heading)
        slug = re.sub(r"\s+", "-", slug.strip())
        count = counts.get(slug, 0)
        counts[slug] = count + 1
        anchors.add(slug if count == 0 else f"{slug}-{count}")
    anchors.update(
        match.group(1)
        for match in re.finditer(r"<(?:a\s+(?:name|id)|[^>]+\s+id)=[\"']([^\"']+)[\"']", text)
    )
    return anchors


def validate_links(path: Path, text: str, errors: list[str]) -> None:
    without_fences = FENCE_RE.sub("", text)
    for match in LINK_RE.finditer(without_fences):
        target = match.group(1).strip()
        if target.startswith("<") and target.endswith(">"):
            target = target[1:-1]
        if " " in target and not target.startswith(("http://", "https://")):
            target = target.split(" ", 1)[0]
        if target.startswith(("http://", "https://", "mailto:", "tel:")):
            continue
        destination, separator, fragment = target.partition("#")
        destination = unquote(destination.split("?", 1)[0])
        resolved = ROOT / path if not destination else (
            (ROOT / destination.lstrip("/"))
            if destination.startswith("/")
            else (ROOT / path).parent / destination
        )
        if not resolved.exists():
            errors.append(f"{path}: broken local link {match.group(1)!r}")
            continue
        if separator and fragment and resolved.is_file() and resolved.suffix.lower() == ".md":
            anchor = unquote(fragment).lower()
            if anchor not in markdown_anchors(resolved.read_text(encoding="utf-8")):
                errors.append(f"{path}: broken Markdown anchor {match.group(1)!r}")


def validate_stale_paths(path: Path, text: str, metadata: dict[str, str], errors: list[str]) -> None:
    if metadata.get("status") in {"archived", "superseded"}:
        return
    if path.name in {"CHANGELOG.md", "MIGRATION.md"} or "completed" in path.parts:
        return
    for label, pattern in STALE_PATTERNS.items():
        match = pattern.search(text)
        if match:
            line = text.count("\n", 0, match.start()) + 1
            errors.append(f"{path}:{line}: {label}: {match.group(0)!r}")


def validate_project_neutrality(
    path: Path, text: str, metadata: dict[str, str], errors: list[str]
) -> None:
    if metadata.get("status") in {"archived", "superseded"}:
        return
    if path in IMMUTABLE_HISTORICAL_EXCEPTIONS:
        return
    match = CONSUMER_SPECIFIC_RE.search(text)
    if match:
        line = text.count("\n", 0, match.start()) + 1
        errors.append(
            f"{path}:{line}: maintained documentation must use project-neutral examples"
        )


def validate_adrs(files: list[Path], metadata_by_path: dict[Path, dict[str, str]], base: str | None, errors: list[str]) -> None:
    numbers: dict[str, Path] = {}
    for path in files:
        match = ADR_RE.fullmatch(path.as_posix())
        metadata = metadata_by_path.get(path, {})
        if match:
            number = match.group(1)
            if number in numbers:
                errors.append(f"duplicate ADR number {number}: {numbers[number]} and {path}")
            numbers[number] = path
            if metadata.get("kind") != "decision":
                errors.append(f"{path}: ADR files must use kind: decision")
        elif path.as_posix().startswith("docs/decisions/ADR-"):
            errors.append(f"{path}: ADR filename must match ADR-NNNN-kebab-case.md")

    if not base:
        return
    changed_result = subprocess.run(
        ["git", "diff", "--name-only", f"{base}...HEAD", "--", "docs/decisions/ADR-*.md"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    for raw in changed_result.stdout.splitlines():
        path = Path(raw)
        base_result = subprocess.run(
            ["git", "show", f"{base}:{raw}"],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        if base_result.returncode != 0:
            continue
        base_metadata = frontmatter(path, base_result.stdout)
        if base_metadata is not None and base_metadata[0].get("status") == "accepted":
            errors.append(f"{path}: accepted ADRs are immutable; add a superseding ADR")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", help="base revision used to enforce accepted ADR immutability")
    args = parser.parse_args()

    files = markdown_files()
    errors: list[str] = []
    metadata_by_path: dict[Path, dict[str, str]] = {}
    for path in files:
        text = (ROOT / path).read_text(encoding="utf-8")
        metadata = validate_metadata(path, text, errors)
        if metadata is None:
            continue
        metadata_by_path[path] = metadata
        validate_links(path, text, errors)
        validate_stale_paths(path, text, metadata, errors)
        validate_project_neutrality(path, text, metadata, errors)

    validate_adrs(files, metadata_by_path, args.base, errors)
    if errors:
        print("Documentation validation failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print(f"Documentation validation passed for {len(files)} governed Markdown files.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
