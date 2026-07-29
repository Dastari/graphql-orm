#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: scripts/check-release-policy.sh <base-revision>" >&2
  exit 2
fi

base_ref=$1
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
crate_dir=$(cd -- "${script_dir}/.." && pwd)
repository_root=$(git -C "${crate_dir}" rev-parse --show-toplevel)
crate_prefix=${crate_dir#"${repository_root}/"}

git -C "${repository_root}" cat-file -e "${base_ref}^{commit}" 2>/dev/null || {
  echo "release-policy: base revision does not exist: ${base_ref}" >&2
  exit 2
}
base=$(git -C "${repository_root}" merge-base "${base_ref}" HEAD)

if ! git -C "${repository_root}" cat-file -e \
  "${base}:${crate_prefix}/Cargo.toml" 2>/dev/null; then
  echo "release-policy: package is new relative to the base revision"
  exit 0
fi

changed=$(git -C "${repository_root}" diff --name-only \
  --relative="${crate_prefix}" "${base}"...HEAD -- "${crate_prefix}")

if [[ -z "${changed}" ]]; then
  echo "release-policy: no committed changes"
  exit 0
fi

has_change() {
  grep -Fxq "$1" <<<"${changed}"
}

public_changed=false
if grep -Eq '^(src/.*\.rs|Cargo\.toml)$' <<<"${changed}"; then
  public_changed=true
fi

if [[ "${public_changed}" == true ]]; then
  has_change README.md || {
    echo "release-policy: public/runtime changes require README.md" >&2
    exit 1
  }
  has_change CHANGELOG.md || {
    echo "release-policy: public/runtime changes require CHANGELOG.md" >&2
    exit 1
  }
  has_change MIGRATION.md || {
    echo "release-policy: public/runtime changes require MIGRATION.md" >&2
    exit 1
  }

  current_version=$(awk -F ' *= *' '/^version = / {gsub(/"/, "", $2); print $2; exit}' "${crate_dir}/Cargo.toml")
  baseline_version=$(git -C "${repository_root}" show \
    "${base}:${crate_prefix}/Cargo.toml" |
    awk -F ' *= *' '/^version = / {gsub(/"/, "", $2); print $2; exit}')
  if [[ -z "${current_version}" || -z "${baseline_version}" ]]; then
    echo "release-policy: could not read current/baseline package version" >&2
    exit 1
  fi
  if [[ "${current_version}" == "${baseline_version}" ]]; then
    echo "release-policy: public/runtime changes require a SemVer version change" >&2
    exit 1
  fi
  highest=$(printf '%s\n%s\n' "${baseline_version}" "${current_version}" | sort -V | tail -n 1)
  if [[ "${highest}" != "${current_version}" ]]; then
    echo "release-policy: package version must not move backwards" >&2
    exit 1
  fi
fi

if grep -Eq '^src/persistence\.rs$' <<<"${changed}"; then
  has_change MIGRATION.md || {
    echo "release-policy: persistence changes require MIGRATION.md" >&2
    exit 1
  }
  current_schema=$(sed -n \
    's/.*AI_SCHEMA_MODULE_VERSION: &str = "\([^"]*\)".*/\1/p' \
    "${crate_dir}/src/persistence.rs")
  baseline_schema=$(git -C "${repository_root}" show \
    "${base}:${crate_prefix}/src/persistence.rs" |
    sed -n 's/.*AI_SCHEMA_MODULE_VERSION: &str = "\([^"]*\)".*/\1/p')
  if [[ -n "${baseline_schema}" && "${current_schema}" == "${baseline_schema}" ]]; then
    echo "release-policy: persistence changes require a new schema-module version" >&2
    exit 1
  fi
fi

echo "release-policy: readme, changelog, migration, crate-version, and schema-version checks passed"
