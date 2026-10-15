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
  rm -f "${temporary_root}"/*.log
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
version_permitted=0
for package in "${packages[@]}"; do
  crate_prefix="crates/${package}"
  if [[ ! -f "${baseline_root}/${crate_prefix}/Cargo.toml" ]]; then
    echo "semver: ${package} is new relative to the baseline"
    continue
  fi
  if ! git -C "${repository_root}" diff --quiet "${base}"...HEAD -- \
    "${crate_prefix}/Cargo.toml" "${crate_prefix}/src"; then
    echo "semver: checking ${package}"
    native_log="${temporary_root}/${package}.log"
    CARGO_TERM_COLOR=never cargo semver-checks \
      --package "${package}" \
      --verbose \
      --manifest-path "${repository_root}/${crate_prefix}/Cargo.toml" \
      --baseline-root "${baseline_root}/${crate_prefix}" \
      --default-features 2>&1 | tee "${native_log}"
    # An empty successful report is possible for implicitly selected private
    # workspace packages. Require native per-package execution evidence before
    # assigning any result; fail closed if the tool changes its output format.
    counts=$(python3 - "${native_log}" "${package}" <<'PYCOUNTS'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
package = re.escape(sys.argv[2])
checking = re.findall(r"^\s*Checking " + package + r" v\S+ -> v\S+ \([^\n]+\)$", text, re.M)
started = re.findall(r"^\s*Starting (\d+) checks, (\d+) unnecessary", text, re.M)
finished = re.findall(r"^\s*Checked \[\s*[0-9.]+s\] (\d+) checks:", text, re.M)
if len(checking) != 1 or len(started) != 1 or [started[0][0]] != finished:
    sys.exit("semver: missing or inconsistent native package/check-count evidence")
if int(started[0][0]) == 0:
    if int(started[0][1]) == 0 or not re.search(
        r"\((?:assume )?major change\)$", checking[0]
    ):
        sys.exit("semver: zero checks without native major-version policy evidence")
print(started[0][0])
PYCOUNTS
)
    if [[ ${counts} -gt 0 ]]; then
      checked=$((checked + 1))
      echo "semver: ${package}: ${counts} native compatibility checks completed"
    else
      version_permitted=$((version_permitted + 1))
      echo "semver: ${package}: version policy permits changes; zero compatibility checks (not API compatibility proof)"
    fi
  else
    echo "semver: ${package} has no public/runtime changes"
  fi
done

echo "semver: ${checked} changed package lane(s) checked; ${version_permitted} version-permitted zero-check lane(s)"
