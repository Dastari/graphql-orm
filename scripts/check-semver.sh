#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: scripts/check-semver.sh <base-revision>" >&2
  exit 2
fi

command -v cargo-semver-checks >/dev/null 2>&1 || {
  echo "semver: cargo-semver-checks is required for the local release gate" >&2
  exit 2
}

repository_root=$(git rev-parse --show-toplevel)
base_ref=$1
git -C "${repository_root}" cat-file -e "${base_ref}^{commit}" 2>/dev/null || {
  echo "semver: base revision does not exist: ${base_ref}" >&2
  exit 2
}
base=$(git -C "${repository_root}" merge-base "${base_ref}" HEAD)
temporary_root=$(mktemp -d)
baseline_root="${temporary_root}/baseline"
cleanup() {
  if [[ -d "${baseline_root}" ]]; then
    git -C "${repository_root}" worktree remove --force "${baseline_root}" >/dev/null
  fi
  rmdir "${temporary_root}" 2>/dev/null || true
}
trap cleanup EXIT
git -C "${repository_root}" worktree add --detach "${baseline_root}" "${base}" >/dev/null

packages=(
  graphql-orm
  graphql-orm-operation-catalog
  graphql-orm-ai-tool-profiles
  graphql-orm-storage
  graphql-orm-backup
  graphql-orm-ai
  graphql-orm-router-protocol
  graphql-orm-router
)

# `cargo-semver-checks` has no proc-macro target mode. Macro compatibility is
# covered by the aligned package-version gate plus the full compile/trybuild
# matrix, so this runner checks only packages with a Rust library target.

checked=0
for package in "${packages[@]}"; do
  crate_prefix="crates/${package}"
  if [[ ! -f "${baseline_root}/${crate_prefix}/Cargo.toml" ]]; then
    echo "semver: ${package} is new relative to the baseline"
    continue
  fi
  if ! git -C "${repository_root}" diff --quiet "${base}"...HEAD -- \
    "${crate_prefix}/Cargo.toml" "${crate_prefix}/src"; then
    echo "semver: checking ${package}"
    cargo semver-checks \
      --manifest-path "${repository_root}/${crate_prefix}/Cargo.toml" \
      --baseline-root "${baseline_root}/${crate_prefix}" \
      --default-features
    checked=$((checked + 1))
  else
    echo "semver: ${package} has no public/runtime changes"
  fi
done

echo "semver: ${checked} changed package lane(s) passed"
