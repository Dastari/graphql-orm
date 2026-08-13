#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: scripts/check-package-release-policy.sh <base-revision>" >&2
  exit 2
fi

repository_root=$(git rev-parse --show-toplevel)
base_ref=$1
git -C "${repository_root}" cat-file -e "${base_ref}^{commit}" 2>/dev/null || {
  echo "package-release-policy: base revision does not exist: ${base_ref}" >&2
  exit 2
}
base=$(git -C "${repository_root}" merge-base "${base_ref}" HEAD)
repository_changes=$(git -C "${repository_root}" diff --name-only "${base}"...HEAD)

packages=(
  graphql-orm
  graphql-orm-macros
  graphql-orm-operation-catalog
  graphql-orm-ai-tool-profiles
  graphql-orm-storage
  graphql-orm-backup
  graphql-orm-ai
  graphql-orm-router-protocol
  graphql-orm-router
)

repository_changed() {
  grep -Fxq "$1" <<<"${repository_changes}"
}

package_version() {
  awk -F ' *= *' '/^version = / {gsub(/"/, "", $2); print $2; exit}' "$1"
}

for package in "${packages[@]}"; do
  crate_prefix="crates/${package}"
  if ! git -C "${repository_root}" cat-file -e \
    "${base}:${crate_prefix}/Cargo.toml" 2>/dev/null; then
    echo "package-release-policy: ${package} is new relative to the base revision"
    continue
  fi

  changed=$(git -C "${repository_root}" diff --name-only \
    --relative="${crate_prefix}" "${base}"...HEAD -- "${crate_prefix}")
  if [[ -z "${changed}" ]]; then
    echo "package-release-policy: ${package} has no committed changes"
    continue
  fi
  if ! grep -Eq '^(src/.*\.rs|Cargo\.toml)$' <<<"${changed}"; then
    echo "package-release-policy: ${package} has no public/runtime changes"
    continue
  fi

  case "${package}" in
    graphql-orm|graphql-orm-macros|graphql-orm-operation-catalog)
      readme="${crate_prefix}/README.md"
      changelog="CHANGELOG.md"
      migration="MIGRATION.md"
      ;;
    *)
      readme="${crate_prefix}/README.md"
      changelog="${crate_prefix}/CHANGELOG.md"
      migration="${crate_prefix}/MIGRATION.md"
      ;;
  esac

  for required in "${readme}" "${changelog}" "${migration}"; do
    repository_changed "${required}" || {
      echo "package-release-policy: ${package} public/runtime changes require ${required}" >&2
      exit 1
    }
  done

  current_version=$(package_version "${repository_root}/${crate_prefix}/Cargo.toml")
  baseline_version=$(git -C "${repository_root}" show \
    "${base}:${crate_prefix}/Cargo.toml" |
    awk -F ' *= *' '/^version = / {gsub(/"/, "", $2); print $2; exit}')
  if [[ -z "${current_version}" || -z "${baseline_version}" ]]; then
    echo "package-release-policy: could not read ${package} versions" >&2
    exit 1
  fi
  if [[ "${current_version}" == "${baseline_version}" ]]; then
    echo "package-release-policy: ${package} public/runtime changes require a SemVer version change" >&2
    exit 1
  fi
  highest=$(printf '%s\n%s\n' "${baseline_version}" "${current_version}" | sort -V | tail -n 1)
  if [[ "${highest}" != "${current_version}" ]]; then
    echo "package-release-policy: ${package} version must not move backwards" >&2
    exit 1
  fi
done

orm_version=$(package_version "${repository_root}/crates/graphql-orm/Cargo.toml")
macros_version=$(package_version "${repository_root}/crates/graphql-orm-macros/Cargo.toml")
if [[ "${orm_version}" != "${macros_version}" ]]; then
  echo "package-release-policy: graphql-orm and graphql-orm-macros versions must remain aligned" >&2
  exit 1
fi

"${repository_root}/crates/graphql-orm-ai/scripts/check-release-policy.sh" "${base}"
"${repository_root}/scripts/check-router-release-policy.sh" "${base}"

echo "package-release-policy: all workspace package documentation and version gates passed"
