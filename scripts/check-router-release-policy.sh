#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: scripts/check-router-release-policy.sh <base-revision>" >&2
  exit 2
fi

base_ref=$1
repository_root=$(git rev-parse --show-toplevel)
git cat-file -e "${base_ref}^{commit}" 2>/dev/null || {
  echo "router-release-policy: base revision does not exist: ${base_ref}" >&2
  exit 2
}
base=$(git merge-base "${base_ref}" HEAD)

for crate_prefix in crates/graphql-orm-router-protocol crates/graphql-orm-router; do
  package=${crate_prefix#crates/}
  if ! git cat-file -e "${base}:${crate_prefix}/Cargo.toml" 2>/dev/null; then
    echo "router-release-policy: ${package} is new relative to the base revision"
    continue
  fi

  changed=$(git diff --name-only --relative="${crate_prefix}" \
    "${base}"...HEAD -- "${crate_prefix}")
  if [[ -z "${changed}" ]]; then
    echo "router-release-policy: ${package} has no committed changes"
    continue
  fi

  if grep -Eq '^(src/.*\.rs|Cargo\.toml)$' <<<"${changed}"; then
    for required in README.md CHANGELOG.md MIGRATION.md; do
      grep -Fxq "${required}" <<<"${changed}" || {
        echo "router-release-policy: ${package} public/runtime changes require ${required}" >&2
        exit 1
      }
    done

    current_version=$(awk -F ' *= *' '/^version = / {gsub(/"/, "", $2); print $2; exit}' \
      "${repository_root}/${crate_prefix}/Cargo.toml")
    baseline_version=$(git show "${base}:${crate_prefix}/Cargo.toml" | \
      awk -F ' *= *' '/^version = / {gsub(/"/, "", $2); print $2; exit}')
    if [[ -z "${current_version}" || -z "${baseline_version}" ]]; then
      echo "router-release-policy: could not read ${package} versions" >&2
      exit 1
    fi
    if [[ "${current_version}" == "${baseline_version}" ]]; then
      echo "router-release-policy: ${package} public/runtime changes require a SemVer version change" >&2
      exit 1
    fi
    highest=$(printf '%s\n%s\n' "${baseline_version}" "${current_version}" | sort -V | tail -n 1)
    if [[ "${highest}" != "${current_version}" ]]; then
      echo "router-release-policy: ${package} version must not move backwards" >&2
      exit 1
    fi
  fi
done

echo "router-release-policy: documentation and version checks passed"
