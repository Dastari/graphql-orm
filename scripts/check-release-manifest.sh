#!/usr/bin/env bash
set -euo pipefail

if [[ $# -gt 1 ]]; then
  echo "usage: scripts/check-release-manifest.sh [commit]" >&2
  exit 2
fi

repository_root=$(git rev-parse --show-toplevel)
ref=${1:-HEAD}
commit=$(git -C "${repository_root}" rev-parse "${ref}^{commit}")
head_commit=$(git -C "${repository_root}" rev-parse "HEAD^{commit}")
if [[ "${commit}" != "${head_commit}" ]]; then
  echo "release-manifest: selected commit must be checked out" >&2
  exit 2
fi

output_dir=$(mktemp -d)
cleanup() {
  rm -rf -- "${output_dir}"
}
trap cleanup EXIT

for suffix in a b; do
  python3 "${repository_root}/scripts/generate-release-manifest.py" \
    --release-id workspace-2000.01.01.1 \
    --ref "${commit}" \
    --output "${output_dir}/manifest-${suffix}.json" \
    --notes-output "${output_dir}/notes-${suffix}.md"
done

cmp "${output_dir}/manifest-a.json" "${output_dir}/manifest-b.json"
cmp "${output_dir}/notes-a.md" "${output_dir}/notes-b.md"

python3 - "${repository_root}" "${output_dir}/manifest-a.json" <<'PY'
import json
from pathlib import Path
import re
import sys

root = Path(sys.argv[1])
manifest = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
contracts = manifest["contracts"]
names = [contract["name"] for contract in contracts]
if names != sorted(names) or len(names) != len(set(names)):
    raise SystemExit("release-manifest: contract rows must be unique and name-sorted")

source = (root / "crates/graphql-orm-operation-catalog/src/semantic.rs").read_text(
    encoding="utf-8"
)
match = re.search(r"GRAPHQL_SEMANTIC_CATALOG_VERSION:\s*u16\s*=\s*(\d+)", source)
if match is None:
    raise SystemExit("release-manifest: semantic catalogue version is unreadable")
semantic = [
    contract
    for contract in contracts
    if contract["name"] == "graphql-orm-semantic-catalog"
]
if semantic != [{"name": "graphql-orm-semantic-catalog", "version": match.group(1)}]:
    raise SystemExit("release-manifest: semantic catalogue contract row is missing or stale")
PY

echo "release-manifest: deterministic output and semantic catalogue contract passed"
