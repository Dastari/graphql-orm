#!/usr/bin/env bash
set -euo pipefail

# The selected tests own their loopback endpoints and use synthetic credentials.
# Keep proxy configuration from routing those requests outside the fixture.
unset HTTP_PROXY HTTPS_PROXY ALL_PROXY http_proxy https_proxy all_proxy
unset S3_TEST_ENDPOINT S3_TEST_BUCKET S3_TEST_REGION S3_TEST_ACCESS_KEY S3_TEST_SECRET_KEY S3_TEST_PATH_STYLE
export NO_PROXY=127.0.0.1,localhost
export no_proxy=127.0.0.1,localhost

result_dir=$(mktemp -d)
trap 'rm -rf -- "$result_dir"' EXIT

run_owned_test() {
  local target=$1
  local test_name=$2
  shift 2
  local result_log="$result_dir/$target.log"
  cargo test -p graphql-orm-storage --locked --color never --no-default-features \
    --features s3 --test "$target" "$test_name" -- --exact "$@" \
    --color never --test-threads=1 2>&1 | tee "$result_log"
  # HTTPS emits child summaries too. Require the final parent summary, not an
  # earlier child pass, so an empty or incorrectly filtered target fails closed.
  if ! awk '/^test result:/ { final_result = $0 } END { exit !(final_result ~ /^test result: ok\. [1-9][0-9]* passed;/) }' "$result_log"; then
    echo "S3 security gate: $target::$test_name did not execute passing parent tests" >&2
    exit 1
  fi
}

run_owned_test s3_integration single_attempt_delete_does_not_replay_after_lost_response_and_replacement
run_owned_test s3_https_transport owned_https_transport_fixture --ignored
# Resumable multipart conformance on a test-owned, digest-pinned S3 server.
run_owned_test s3_multipart owned_s3_passes_the_multipart_conformance_suite --ignored
