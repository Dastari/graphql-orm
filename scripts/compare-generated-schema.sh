#!/usr/bin/env bash
# Compare what the entity derives generate at a base revision and in the working
# tree: the SDL, the operation catalogue and the semantic catalogue of the
# full-stack fixture, for SQLite and PostgreSQL. Both sides compile the working
# tree's fixture source, so only the macros and runtime differ.
#
# Usage: scripts/compare-generated-schema.sh <base-ref>
# Run under the caller's Cargo resource limits and target directory. No database.
set -euo pipefail
base=${1:?usage: scripts/compare-generated-schema.sh <base-ref>}
root=$(git rev-parse --show-toplevel)
cd "$root"
fixture=crates/graphql-orm-macros/fixtures/full-stack
work=$(mktemp -d)
cleanup() {
  git -C "$root" worktree remove --force "$work/base" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
git worktree add --detach "$work/base" "$base" >/dev/null
rm -rf "$work/base/$fixture"
cp -R "$root/$fixture" "$work/base/$fixture"
target=${CARGO_TARGET_DIR:-$root/target}
for backend in sqlite postgres; do
  cargo run --manifest-path "$fixture/Cargo.toml" --locked --no-default-features \
    --features "$backend" --example export_generated_surface >"$work/head.txt"
  # The base revision may carry other package versions, so its lockfile is not pinned.
  cargo run --manifest-path "$work/base/$fixture/Cargo.toml" --target-dir "$target/compare-base" \
    --no-default-features --features "$backend" --example export_generated_surface >"$work/base.txt"
  if ! cmp "$work/base.txt" "$work/head.txt"; then
    diff -u "$work/base.txt" "$work/head.txt" | head -200 || true
    echo "compare-generated-schema: $backend output differs from $base" >&2
    exit 1
  fi
  echo "compare-generated-schema: $backend SDL and catalogues are byte-identical to $base ($(wc -c <"$work/head.txt") bytes)"
done
