#!/usr/bin/env bash
# Runs one fuzz target for a bounded time on the reviewed stable toolchain.
#
# usage: fuzz/run.sh <target> [seconds] [rss-limit-mb]
#
# libFuzzer coverage comes from stable `-C` flags, as `cargo fuzz` with
# `--sanitizer none` passes them, so no nightly toolchain or extra Cargo tool
# is needed. New inputs go to a scratch directory; the retained corpus and
# regressions are read only. A failure leaves its input in `artifacts/`.
set -euo pipefail

if [[ $# -lt 1 || $# -gt 3 ]]; then
  echo "usage: $0 <bearer_structured|bearer_raw|jwks_document> [seconds] [rss-limit-mb]" >&2
  exit 2
fi
target=$1
seconds=${2:-60}
rss_limit=${3:-2048}
case "${target}" in
  bearer_structured|bearer_raw|jwks_document) ;;
  *) echo "unknown fuzz target: ${target}" >&2; exit 2 ;;
esac
[[ "${seconds}" =~ ^[0-9]+$ && "${rss_limit}" =~ ^[0-9]+$ ]] || {
  echo "seconds and rss-limit-mb must be integers" >&2
  exit 2
}

fuzz_dir=$(cd "$(dirname "$0")" && pwd)
export RUSTFLAGS="--cfg fuzzing -Cpasses=sancov-module \
-Cllvm-args=-sanitizer-coverage-level=4 \
-Cllvm-args=-sanitizer-coverage-inline-8bit-counters \
-Cllvm-args=-sanitizer-coverage-pc-table \
-Cllvm-args=-sanitizer-coverage-trace-compares \
-Cdebug-assertions -Coverflow-checks"
# An explicit target keeps the instrumentation flags out of build scripts.
host=$(rustc -vV | sed -n 's/^host: //p')
cargo build --manifest-path "${fuzz_dir}/Cargo.toml" --locked --release \
  --target "${host}" --bin "${target}"

target_dir=${CARGO_TARGET_DIR:-${fuzz_dir}/target}
scratch=$(mktemp -d)
trap 'rm -rf -- "${scratch}"' EXIT
mkdir -p "${fuzz_dir}/artifacts/${target}"
seeds=("${fuzz_dir}/corpus/${target}")
[[ -d "${fuzz_dir}/regressions/${target}" ]] && seeds+=("${fuzz_dir}/regressions/${target}")
"${target_dir}/${host}/release/${target}" \
  -artifact_prefix="${fuzz_dir}/artifacts/${target}/" \
  -max_total_time="${seconds}" \
  -rss_limit_mb="${rss_limit}" \
  -max_len=4096 \
  -timeout=10 \
  -print_final_stats=1 \
  "${scratch}" "${seeds[@]}"
