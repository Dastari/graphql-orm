#!/usr/bin/env bash
set -euo pipefail

repository_root=$(git rev-parse --show-toplevel)
metadata_file=$(mktemp)
trap 'rm -f "${metadata_file}"' EXIT

if grep -nE 'git[[:space:]]*=.*graphql-orm-(ai|backup|storage|router(-protocol)?)(\.git)?' \
  "${repository_root}/Cargo.toml" \
  "${repository_root}"/crates/*/Cargo.toml; then
  echo "workspace-dependencies: internal Git dependency found" >&2
  exit 1
fi

if grep -nE 'git\+https://github\.com/Dastari/graphql-orm-(ai|backup|storage|router(-protocol)?)' \
  "${repository_root}/Cargo.lock"; then
  echo "workspace-dependencies: old internal Git source found in Cargo.lock" >&2
  exit 1
fi

cargo metadata \
  --manifest-path "${repository_root}/Cargo.toml" \
  --format-version 1 \
  --locked >"${metadata_file}"

python3 - "${metadata_file}" "${repository_root}" <<'PY'
import json
import pathlib
import re
import sys

metadata_path = pathlib.Path(sys.argv[1])
repository_root = pathlib.Path(sys.argv[2]).resolve()
metadata = json.loads(metadata_path.read_text())

expected = {
    "graphql-orm",
    "graphql-orm-macros",
    "graphql-orm-storage",
    "graphql-orm-backup",
    "graphql-orm-ai",
    "graphql-orm-ai-tool-profiles",
    "graphql-orm-router-protocol",
    "graphql-orm-router",
    "graphql-orm-operation-catalog",
}

for package_name in sorted(expected):
    matches = [
        package
        for package in metadata["packages"]
        if package["name"] == package_name
    ]
    if len(matches) != 1:
        raise SystemExit(
            f"workspace-dependencies: expected one {package_name} package, "
            f"found {len(matches)}"
        )

    package = matches[0]
    manifest_path = pathlib.Path(package["manifest_path"]).resolve()
    if package["source"] is not None or repository_root not in manifest_path.parents:
        raise SystemExit(
            f"workspace-dependencies: {package_name} is not resolved from "
            "this workspace"
        )

packages_by_id = {package["id"]: package for package in metadata["packages"]}
nodes_by_id = {node["id"]: node for node in metadata["resolve"]["nodes"]}
workspace_member_ids = set(metadata["workspace_members"])
workspace_names = {packages_by_id[package_id]["name"] for package_id in workspace_member_ids}

# Manifest declarations include optional edges even when inactive in the
# resolved graph. Keep dependency kinds so the guide can distinguish dev
# dependencies from production dependencies, including parallel declarations.
declared_internal_edges = set()
for package_id in workspace_member_ids:
    package = packages_by_id[package_id]
    for dependency in package["dependencies"]:
        if dependency["name"] not in workspace_names:
            continue
        label = {
            None: "",
            "dev": " (dev-only)",
            "build": " (build)",
        }[dependency["kind"]]
        if dependency["optional"]:
            label += " (optional)"
        declared_internal_edges.add(
            f'`{package["name"]} -> {dependency["name"]}`{label}'
        )

guide = (repository_root / "AGENTS.md").read_text()
heading = "- Preserve this acyclic dependency direction."
_, separator, directions = guide.partition(heading)
if not separator:
    raise SystemExit("workspace-dependencies: dependency directions missing from AGENTS.md")
directions = directions.split("\n- ", 1)[0]
documented_internal_edges = set(re.findall(r"^  - (.+)$", directions, re.MULTILINE))

edge_errors = []
for description, edges in (
    ("missing from AGENTS.md", declared_internal_edges - documented_internal_edges),
    ("listed in AGENTS.md but not declared", documented_internal_edges - declared_internal_edges),
):
    edge_errors.extend(f"  {edge}: {description}" for edge in sorted(edges))
if edge_errors:
    raise SystemExit("workspace-dependencies: documented edges differ:\n" + "\n".join(edge_errors))

allowed_internal_edges = {
    ("graphql-orm-ai", "graphql-orm-ai-tool-profiles"),
    # AI requires the producer contract version directly as well as via profiles.
    ("graphql-orm-ai", "graphql-orm-operation-catalog"),
    ("graphql-orm-ai", "graphql-orm-storage"),
    ("graphql-orm-ai", "graphql-orm"),
    ("graphql-orm-ai-tool-profiles", "graphql-orm-operation-catalog"),
    ("graphql-orm-backup", "graphql-orm-storage"),
    ("graphql-orm-backup", "graphql-orm"),
    ("graphql-orm", "graphql-orm-macros"),
    ("graphql-orm", "graphql-orm-operation-catalog"),
    # The runnable router-descriptor example uses the protocol directly. The
    # edge is test-only; production support remains routed through the
    # operation-catalog feature boundary.
    ("graphql-orm", "graphql-orm-router-protocol"),
    ("graphql-orm-operation-catalog", "graphql-orm-router-protocol"),
    ("graphql-orm-router", "graphql-orm-router-protocol"),
    # Test-only end-to-end coverage proves that profile manifests survive the
    # router protocol's canonical extension transport. This edge must never
    # become a normal or build dependency.
    ("graphql-orm-ai-tool-profiles", "graphql-orm-router-protocol"),
    # Real-producer filter contract tests compile the derived SDL and semantic
    # metadata through profiles. This edge remains dev-only.
    ("graphql-orm", "graphql-orm-ai-tool-profiles"),
}
test_only_internal_edges = {
    ("graphql-orm", "graphql-orm-router-protocol"),
    ("graphql-orm-ai-tool-profiles", "graphql-orm-router-protocol"),
    ("graphql-orm", "graphql-orm-ai-tool-profiles"),
}
actual_internal_edges = set()
for package_id in workspace_member_ids:
    source = packages_by_id[package_id]["name"]
    for dependency in nodes_by_id[package_id]["deps"]:
        if dependency["pkg"] in workspace_member_ids:
            target = packages_by_id[dependency["pkg"]]["name"]
            edge = (source, target)
            actual_internal_edges.add(edge)
            if edge in test_only_internal_edges and any(
                dependency_kind["kind"] != "dev"
                for dependency_kind in dependency["dep_kinds"]
            ):
                raise SystemExit(
                    "workspace-dependencies: test-only internal edge became "
                    f"a runtime/build dependency: {source} -> {target}"
                )

unexpected_edges = actual_internal_edges - allowed_internal_edges
if unexpected_edges:
    rendered = ", ".join(
        f"{source} -> {target}" for source, target in sorted(unexpected_edges)
    )
    raise SystemExit(
        f"workspace-dependencies: disallowed internal edge(s): {rendered}"
    )

protocol_id = next(
    package["id"]
    for package in metadata["packages"]
    if package["name"] == "graphql-orm-router-protocol"
)
protocol_forbidden = {
    "agql-auth",
    "axum",
    "graphql-orm",
    "hive-router",
    "ntex",
    "sqlx",
}
pending = [protocol_id]
visited = set()
while pending:
    package_id = pending.pop()
    if package_id in visited:
        continue
    visited.add(package_id)
    pending.extend(dependency["pkg"] for dependency in nodes_by_id[package_id]["deps"])

forbidden_resolved = sorted(
    {
        packages_by_id[package_id]["name"]
        for package_id in visited
        if packages_by_id[package_id]["name"] in protocol_forbidden
    }
)
if forbidden_resolved:
    raise SystemExit(
        "workspace-dependencies: protocol resolves forbidden runtime "
        f"dependencies: {', '.join(forbidden_resolved)}"
    )

neutral_boundaries = {
    "graphql-orm-operation-catalog": {
        "graphql-orm",
        "graphql-orm-ai",
        "graphql-orm-backup",
        "graphql-orm-storage",
        "graphql-orm-macros",
        "sqlx",
        "tiberius",
    },
    "graphql-orm-ai-tool-profiles": {
        "graphql-orm",
        "graphql-orm-ai",
        "graphql-orm-backup",
        "graphql-orm-storage",
        "sqlx",
        "tiberius",
        "reqwest",
    },
}
for neutral_package, forbidden in neutral_boundaries.items():
    neutral_id = next(
        package["id"]
        for package in metadata["packages"]
        if package["name"] == neutral_package
    )
    pending = [neutral_id]
    visited = set()
    while pending:
        package_id = pending.pop()
        if package_id in visited:
            continue
        visited.add(package_id)
        pending.extend(
            dependency["pkg"] for dependency in nodes_by_id[package_id]["deps"]
        )
    resolved = sorted(
        {
            packages_by_id[package_id]["name"]
            for package_id in visited
            if packages_by_id[package_id]["name"] in forbidden
        }
    )
    if resolved:
        raise SystemExit(
            f"workspace-dependencies: {neutral_package} resolves forbidden "
            f"dependencies: {', '.join(resolved)}"
        )

print(
    "workspace-dependencies: path sources, internal directions, and protocol "
    "boundary are valid"
)
PY

if cargo tree \
  --manifest-path "${repository_root}/Cargo.toml" \
  -p graphql-orm \
  --no-default-features \
  --edges normal,build \
  --prefix none \
  --locked | grep -q '^graphql-orm-router-protocol v'; then
  echo "workspace-dependencies: graphql-orm resolves router protocol with its feature disabled" >&2
  exit 1
fi

if ! cargo tree \
  --manifest-path "${repository_root}/Cargo.toml" \
  -p graphql-orm \
  --no-default-features \
  --features router-protocol \
  --edges normal,build \
  --prefix none \
  --locked | grep -q '^graphql-orm-router-protocol v'; then
  echo "workspace-dependencies: graphql-orm router-protocol feature does not resolve the protocol package" >&2
  exit 1
fi

# SQLx's PostgreSQL macros prefer time over chrono when both are enabled.
postgres_sqlx_features=$(cargo tree \
  --manifest-path "${repository_root}/Cargo.toml" \
  -p graphql-orm \
  --no-default-features \
  --features postgres \
  --edges features \
  --invert sqlx \
  --locked)
if grep -q 'sqlx feature "time"' <<<"${postgres_sqlx_features}"; then
  echo "workspace-dependencies: graphql-orm postgres changes SQLx temporal macro types by enabling sqlx/time" >&2
  exit 1
fi

# Keep the ORM from changing consumers' signed-year parsing through feature unification.
orm_time_features=$(cargo tree \
  --manifest-path "${repository_root}/Cargo.toml" \
  -p graphql-orm \
  --features postgres,mssql \
  --edges features \
  --invert time \
  --locked)
if grep -q 'time feature "large-dates"' <<<"${orm_time_features}"; then
  echo "workspace-dependencies: graphql-orm changes consumer time parsing by enabling time/large-dates" >&2
  exit 1
fi
