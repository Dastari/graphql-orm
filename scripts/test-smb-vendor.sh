#!/usr/bin/env bash
# Run the owner-local SMB vendor unit tests (including the GHSA-439h-2cx8-xwgx
# verify-before-status regression tests) with storage's selected features.
#
# The vendored package is an isolated, excluded workspace. Tests run on a
# temporary copy with the archived upstream lockfile, under --locked, so the
# vendor directory keeps its exact reviewed file set and the resolved graph
# cannot drift.
set -euo pipefail

repository_root=$(git rev-parse --show-toplevel)
vendor="${repository_root}/crates/graphql-orm-storage/vendor/smb"
work=$(mktemp -d "${TMPDIR:-/tmp}/smb-vendor-test-XXXXXX")
trap 'rm -rf "${work}"' EXIT

cp -R "${vendor}" "${work}/smb"
cp "${work}/smb/Cargo.lock.upstream" "${work}/smb/Cargo.lock"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${work}/target}"

cargo test --manifest-path "${work}/smb/Cargo.toml" --lib --locked \
  --no-default-features --features async,encrypt,sign,netbios-transport "$@"
