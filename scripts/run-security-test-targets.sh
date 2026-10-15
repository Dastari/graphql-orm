#!/usr/bin/env bash
set -euo pipefail

if (( $# < 2 )); then
  echo "usage: $0 <features> <test-target> [test-target ...]" >&2
  exit 2
fi

features=$1
shift
result_dir=$(mktemp -d)
trap 'rm -rf -- "$result_dir"' EXIT

for target in "$@"; do
  case "$target" in
    ''|*[!a-zA-Z0-9_-]*)
      echo "invalid integration test target: $target" >&2
      exit 2
      ;;
  esac
  result_log="$result_dir/$target.log"
  cargo test -p graphql-orm --locked --color never --no-default-features \
    --features "$features" --test "$target" -- --color never --test-threads=1 \
    2>&1 | tee "$result_log"
  # A feature-gated integration target can succeed while running zero tests.
  # Require at least one passed test in every explicitly selected target.
  if ! awk '/^test result: ok\. [1-9][0-9]* passed;/ { found = 1 } END { exit !found }' "$result_log"; then
    echo "security gate: $target did not execute any passing tests ($features)" >&2
    exit 1
  fi
done
