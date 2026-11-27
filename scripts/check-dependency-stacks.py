#!/usr/bin/env python3
"""Check resolved JWT and TLS stacks in workspace-owned lanes."""

from pathlib import Path
import re
import subprocess


ROOT = Path(__file__).resolve().parents[1]


def packages(*args):
    tree = subprocess.check_output(
        ["cargo", "tree", "--locked", "--prefix", "none", "-f", "{p}", *args],
        cwd=ROOT,
        text=True,
    )
    result = {}
    for name, version in re.findall(r"^(\S+) v(\S+)", tree, re.MULTILINE):
        result.setdefault(name, set()).add(version)
    return result


# The expected jsonwebtoken major is the workspace's single supported line. A
# coordinated move to a new major (with agql-auth and the router) must update
# JWT_MAJOR here in the same change.
JWT_MAJOR = "11."


def root_lock_versions(name):
    lock = (ROOT / "Cargo.lock").read_text()
    return re.findall(rf'^name = "{re.escape(name)}"\nversion = "([^"]+)"', lock, re.MULTILINE)


def check():
    errors = []
    locked = root_lock_versions("jsonwebtoken")
    if len(locked) != 1 or not locked[0].startswith(JWT_MAJOR):
        errors.append(f"root Cargo.lock resolves jsonwebtoken {locked}; expected one {JWT_MAJOR}x entry")
    for backend in ("sqlite", "postgres", "mssql"):
        graph = packages(
            "-p", "graphql-orm", "--no-default-features", "--features",
            f"{backend},auth-agql", "-e", "normal,build,dev",
        )
        versions = graph.get("jsonwebtoken", set())
        if len(versions) != 1 or not all(version.startswith(JWT_MAJOR) for version in versions):
            errors.append(f"ORM {backend} + auth-agql resolved jsonwebtoken {sorted(versions)}; expected only {JWT_MAJOR}x")

    fixture = "crates/graphql-orm-macros/fixtures/full-stack/Cargo.toml"
    graph = packages("--manifest-path", fixture, "--no-default-features", "--features", "sqlite")
    unexpected = {"sqlx-postgres", "rustls", "native-tls"} & graph.keys()
    if unexpected:
        errors.append(f"SQLite full-stack fixture resolved unused dependencies: {', '.join(sorted(unexpected))}")

    graph = packages("--manifest-path", fixture, "--no-default-features", "--features", "postgres")
    if (
        "sqlx-postgres" not in graph
        or "sqlx-sqlite" in graph
        or len(graph.get("rustls", set())) != 1
        or "native-tls" in graph
    ):
        errors.append("PostgreSQL full-stack fixture must resolve only the PostgreSQL driver and one rustls stack")

    graph = packages(
        "-p", "graphql-orm-ai", "--no-default-features",
        "--features", "postgres,provider-openai-core,provider-tls-ring", "-e", "normal",
    )
    if (
        "ring" not in graph
        or len(graph.get("rustls", set())) != 1
        or {"aws-lc-rs", "native-tls"} & graph.keys()
    ):
        errors.append("PostgreSQL with AI's explicit ring HTTP stack must resolve only rustls with ring")

    for error in errors:
        print(error)
    if not errors:
        print("Dependency stack checks passed")
    return bool(errors)


if __name__ == "__main__":
    raise SystemExit(check())
