#!/usr/bin/env python3
"""Generate and validate an immutable workspace release bill of materials."""

from __future__ import annotations

import argparse
from datetime import date
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib
from typing import Any
from urllib.parse import parse_qs, urlsplit


ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / "release.toml"
RELEASE_ID_RE = re.compile(r"^workspace-\d{4}\.\d{2}\.\d{2}\.\d+$")
FULL_SHA_RE = re.compile(r"^[0-9a-f]{40}$")


def run(*args: str, check: bool = True) -> str:
    result = subprocess.run(
        list(args),
        cwd=ROOT,
        check=check,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def load_config() -> dict[str, Any]:
    with CONFIG.open("rb") as handle:
        config = tomllib.load(handle)
    if config.get("format_version") != 1:
        raise SystemExit("release.toml: unsupported format_version")
    if config.get("distribution") != "git":
        raise SystemExit("release.toml: only the reviewed Git distribution is supported")
    if config.get("workspace_tag_prefix") != "workspace-":
        raise SystemExit("release.toml: workspace_tag_prefix must remain workspace-")
    if config.get("package_tag_template") != "{package}-v{version}":
        raise SystemExit("release.toml: unsupported package_tag_template")
    return config


def cargo_metadata() -> dict[str, Any]:
    return json.loads(
        run("cargo", "metadata", "--format-version", "1", "--no-deps", "--locked")
    )


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def source_at(commit: str, relative_path: str) -> str:
    return run("git", "rev-parse", f"{commit}:{relative_path}")


def exists_at(commit: str, relative_path: str) -> bool:
    result = subprocess.run(
        ["git", "cat-file", "-e", f"{commit}:{relative_path}"],
        cwd=ROOT,
        capture_output=True,
    )
    return result.returncode == 0


def exact_commit(ref: str) -> str:
    commit = run("git", "rev-parse", f"{ref}^{{commit}}")
    if not FULL_SHA_RE.fullmatch(commit):
        raise SystemExit(f"release ref did not resolve to a full commit: {ref}")
    return commit


def require_clean_worktree() -> None:
    if run("git", "status", "--porcelain"):
        raise SystemExit("release manifests must be generated from a clean worktree")


def parse_contracts(commit: str) -> list[dict[str, str]]:
    contracts = (
        (
            "graphql-orm-ai-schema-module",
            "crates/graphql-orm-ai/src/persistence.rs",
            r'AI_SCHEMA_MODULE_VERSION:\s*&str\s*=\s*"([^"]+)"',
        ),
        (
            "graphql-orm-router-protocol",
            "crates/graphql-orm-router-protocol/src/version.rs",
            r"SUPPORTED_PROTOCOL_VERSION:.*?major:\s*(\d+),\s*minor:\s*(\d+)",
        ),
        (
            "graphql-orm-ai-tool-manifest",
            "crates/graphql-orm-ai-tool-profiles/src/profiles.rs",
            r"AI_GRAPHQL_TOOL_MANIFEST_VERSION:\s*u16\s*=\s*(\d+)",
        ),
        (
            "graphql-orm-ai-query-capability",
            "crates/graphql-orm-ai-tool-profiles/src/query_plans.rs",
            r"AI_GRAPHQL_QUERY_CAPABILITY_VERSION:\s*u16\s*=\s*(\d+)",
        ),
        (
            "graphql-orm-ai-mutation-capability",
            "crates/graphql-orm-ai-tool-profiles/src/query_plans.rs",
            r"AI_GRAPHQL_MUTATION_CAPABILITY_VERSION:\s*u16\s*=\s*(\d+)",
        ),
        (
            "graphql-orm-ai-subscription-capability",
            "crates/graphql-orm-ai-tool-profiles/src/query_plans.rs",
            r"AI_GRAPHQL_SUBSCRIPTION_CAPABILITY_VERSION:\s*u16\s*=\s*(\d+)",
        ),
        (
            "graphql-orm-semantic-catalog",
            "crates/graphql-orm-operation-catalog/src/semantic.rs",
            r"GRAPHQL_SEMANTIC_CATALOG_VERSION:\s*u16\s*=\s*(\d+)",
        ),
        (
            "graphql-orm-operation-assurance-manifest",
            "crates/graphql-orm/src/graphql/assurance.rs",
            r"OPERATION_ASSURANCE_MANIFEST_VERSION:\s*u32\s*=\s*(\d+)",
        ),
    )
    values: list[dict[str, str]] = []
    for name, path, pattern in contracts:
        text = run("git", "show", f"{commit}:{path}")
        match = re.search(pattern, text, re.DOTALL)
        if match is None:
            raise SystemExit(f"could not read {name} from {path}")
        version = ".".join(match.groups())
        values.append({"name": name, "version": version})
    return sorted(values, key=lambda contract: contract["name"])


def package_kind(package: dict[str, Any]) -> list[str]:
    kinds = {
        kind
        for target in package["targets"]
        for kind in target["kind"]
        if kind not in {"example", "test", "bench", "custom-build"}
    }
    return sorted(kinds)


def manifest_packages(
    metadata: dict[str, Any], config: dict[str, Any], commit: str
) -> list[dict[str, Any]]:
    members = set(metadata["workspace_members"])
    packages = [package for package in metadata["packages"] if package["id"] in members]
    packages.sort(key=lambda package: package["name"])
    package_tag_template = config["package_tag_template"]
    result: list[dict[str, Any]] = []
    for package in packages:
        if package.get("publish") != []:
            raise SystemExit(
                f"{package['name']}: every Git-only workspace package must set publish = false"
            )
        manifest_path = Path(package["manifest_path"])
        relative_manifest = manifest_path.relative_to(ROOT).as_posix()
        package_path = manifest_path.parent.relative_to(ROOT).as_posix()
        package_changelog = f"{package_path}/CHANGELOG.md"
        changelog_path = (
            package_changelog if exists_at(commit, package_changelog) else "CHANGELOG.md"
        )
        result.append(
            {
                "changelogPath": changelog_path,
                "manifestPath": relative_manifest,
                "name": package["name"],
                "packageSourceTree": source_at(commit, package_path),
                "tag": package_tag_template.format(
                    package=package["name"], version=package["version"]
                ),
                "targets": package_kind(package),
                "version": package["version"],
            }
        )
    return result


def external_git_dependencies(
    metadata: dict[str, Any], lock: dict[str, Any] | None = None
) -> list[dict[str, Any]]:
    if lock is None:
        with (ROOT / "Cargo.lock").open("rb") as handle:
            lock = tomllib.load(handle)
    workspace_members = set(metadata["workspace_members"])
    aggregated: dict[tuple[str, str, str, str], set[str]] = {}
    for package in metadata["packages"]:
        if package["id"] not in workspace_members:
            continue
        for dependency in package["dependencies"]:
            source = dependency.get("source") or ""
            if not source.startswith("git+"):
                continue
            parsed = urlsplit(source.removeprefix("git+"))
            query = parse_qs(parsed.query, keep_blank_values=True)
            revisions = query.get("rev", [])
            tags = query.get("tag", [])
            url = f"{parsed.scheme}://{parsed.netloc}{parsed.path}"
            exact_revision = (
                set(query) == {"rev"}
                and len(revisions) == 1
                and FULL_SHA_RE.fullmatch(revisions[0])
            )
            auth_release = (
                set(query) == {"tag"}
                and len(tags) == 1
                and dependency["name"] == "agql-auth"
                and url == "https://github.com/Dastari/agql-auth.git"
                and re.fullmatch(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", tags[0])
            )
            if not (exact_revision or auth_release):
                raise SystemExit(
                    f"{package['name']}: Git dependency {dependency['name']} requires "
                    "a full revision or an agql-auth version release tag"
                )
            resolved = {
                entry["source"].partition("#")[2]
                for entry in lock.get("package", [])
                if entry.get("name") == dependency["name"]
                and entry.get("source", "").partition("#")[0] == source
            }
            if len(resolved) != 1 or not FULL_SHA_RE.fullmatch(next(iter(resolved), "")):
                raise SystemExit(f"{dependency['name']}: expected exactly one full locked Git commit")
            revision = resolved.pop()
            if exact_revision and revision != revisions[0]:
                raise SystemExit(f"{dependency['name']}: locked commit does not match the full revision")
            tag = tags[0] if auth_release else ""
            key = (dependency["name"], url, revision, tag)
            aggregated.setdefault(key, set()).add(package["name"])
    return [
        {
            "consumers": sorted(consumers),
            "name": name,
            "revision": revision,
            "url": url,
            **({"tag": tag} if tag else {}),
        }
        for (name, url, revision, tag), consumers in sorted(aggregated.items())
    ]


def verify_package_tags(packages: list[dict[str, Any]], commit: str) -> None:
    local_tags = set(run("git", "tag", "--list").splitlines())
    for package in packages:
        tag = package["tag"]
        if tag not in local_tags:
            continue
        tagged_commit = exact_commit(tag)
        package_path = str(Path(package["manifestPath"]).parent)
        tagged_tree = source_at(tagged_commit, package_path)
        current_tree = source_at(commit, package_path)
        if tagged_tree != current_tree:
            raise SystemExit(
                f"{tag}: package source changed without a new package version"
            )


def build_manifest(release_id: str, ref: str, verify_tags: bool) -> dict[str, Any]:
    match = RELEASE_ID_RE.fullmatch(release_id)
    if match is None:
        raise SystemExit(
            "release ID must match workspace-YYYY.MM.DD.N (for example workspace-2026.08.11.1)"
        )
    date_text, sequence_text = release_id.removeprefix("workspace-").rsplit(".", 1)
    try:
        date.fromisoformat(date_text.replace(".", "-"))
    except ValueError as error:
        raise SystemExit(f"release ID contains an invalid calendar date: {date_text}") from error
    if int(sequence_text) < 1:
        raise SystemExit("release ID sequence must be at least 1")
    config = load_config()
    commit = exact_commit(ref)
    if commit != exact_commit("HEAD"):
        raise SystemExit("check out the selected release commit before generating its manifest")
    metadata = cargo_metadata()
    packages = manifest_packages(metadata, config, commit)
    if verify_tags:
        verify_package_tags(packages, commit)
    return {
        "artifactProfiles": {"router": config["router_artifact"]},
        "contracts": parse_contracts(commit),
        "distribution": {
            "crateRegistryPublishing": False,
            "kind": config["distribution"],
        },
        "externalGitDependencies": external_git_dependencies(metadata),
        "formatVersion": config["format_version"],
        "packages": packages,
        "releaseId": release_id,
        "repository": config["repository"],
        "source": {
            "cargoLockSha256": file_sha256(ROOT / "Cargo.lock"),
            "commit": commit,
        },
    }


def render_notes(manifest: dict[str, Any]) -> str:
    source_url = (
        f"https://github.com/{manifest['repository']}/blob/"
        f"{manifest['source']['commit']}"
    )
    lines = [
        f"# {manifest['releaseId']}",
        "",
        "This is an immutable, Git-only release of the tested workspace package set.",
        "Consumers pin the annotated workspace tag "
        f"(`tag = \"{manifest['releaseId']}\"`) as one Git reference for every "
        "package in this workspace, and record the commit "
        f"`{manifest['source']['commit']}` from the attached manifest in their own "
        "reviewed pin record.",
        "A published workspace tag is annotated and is never moved.",
        "Package tags identify package versions within this release and are not "
        "pinning selectors.",
        "",
        "## Packages",
        "",
        "| Package | Version | Tag | Changes |",
        "| --- | --- | --- | --- |",
    ]
    for package in manifest["packages"]:
        lines.append(
            f"| `{package['name']}` | `{package['version']}` | `{package['tag']}` | "
            f"[changelog]({source_url}/{package['changelogPath']}) |"
        )
    lines.extend(
        [
            "",
            "## Wire and persistence contracts",
            "",
            "| Contract | Version |",
            "| --- | --- |",
        ]
    )
    for contract in manifest["contracts"]:
        lines.append(f"| `{contract['name']}` | `{contract['version']}` |")
    lines.extend(
        [
            "",
            "The attached JSON manifest is the canonical release bill of materials.",
        ]
    )
    return "\n".join(lines) + "\n"


def write_or_print(content: str, output: Path | None) -> None:
    if output is None:
        sys.stdout.write(content)
        return
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(content, encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--release-id", required=True)
    parser.add_argument("--ref", default="HEAD")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--notes-output", type=Path)
    parser.add_argument("--check-clean", action="store_true")
    parser.add_argument("--verify-tags", action="store_true")
    args = parser.parse_args()
    if args.check_clean:
        require_clean_worktree()
    manifest = build_manifest(args.release_id, args.ref, args.verify_tags)
    serialized = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    write_or_print(serialized, args.output)
    if args.notes_output is not None:
        write_or_print(render_notes(manifest), args.notes_output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
