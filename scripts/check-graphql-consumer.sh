#!/usr/bin/env bash
set -euo pipefail
root=$(git rev-parse --show-toplevel)
cd "$root"
manifest=crates/graphql-orm/tests/fixtures/graphql-features/Cargo.toml
combined=orm,sqlite,ai,profiles,auth,runtime,router,router-auth
cargo test --manifest-path "$manifest" --locked --no-default-features --features "$combined"
cargo test --manifest-path "$manifest" --locked --no-default-features \
  --features "$combined,additive,probe-ui,probe-tempfile" --all-targets
logs=$(mktemp -d)
trap 'rm -rf "$logs"' EXIT
for probe in ui upload-file; do
  feature=probe-ui
  [[ "$probe" != upload-file ]] || feature=probe-tempfile
  if cargo check --manifest-path "$manifest" --locked --features "$feature" \
    --example "$probe" >"$logs/$probe.log" 2>&1; then
    echo "graphql-consumer: negative $probe unexpectedly compiled" >&2
    exit 1
  fi
  cat "$logs/$probe.log"
  if [[ "$probe" == ui ]]; then
    grep -q 'GraphiQLSource' "$logs/$probe.log"
    grep -q 'error\[E0433\]' "$logs/$probe.log"
  else
    grep -q 'error\[E0308\]' "$logs/$probe.log"
    grep -q 'expected.*Bytes.*found.*File' "$logs/$probe.log"
  fi
done
echo 'graphql-consumer: minimal/additive hosts and two expected negative probes passed'
