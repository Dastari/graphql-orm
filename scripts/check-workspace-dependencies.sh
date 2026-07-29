#!/usr/bin/env bash
set -euo pipefail

repository_root=$(git rev-parse --show-toplevel)
metadata_file=$(mktemp)
trap 'rm -f "${metadata_file}"' EXIT

if grep -nE 'git[[:space:]]*=.*graphql-orm-(ai|backup|storage)(\.git)?' \
  "${repository_root}/Cargo.toml" \
  "${repository_root}"/crates/*/Cargo.toml; then
  echo "workspace-dependencies: internal Git dependency found" >&2
  exit 1
fi

if grep -nE 'git\+https://github\.com/Dastari/graphql-orm-(ai|backup|storage)' \
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

print("workspace-dependencies: one path source for every internal package")
PY
