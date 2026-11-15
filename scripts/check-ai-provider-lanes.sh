#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: scripts/check-ai-provider-lanes.sh [test|check|clippy|doc] [provider-feature]" >&2
  echo "       scripts/check-ai-provider-lanes.sh --list" >&2
}

if [[ $# -gt 2 ]]; then
  usage
  exit 2
fi

mode=${1:-test}
selected=${2:-}
providers=(
  provider-openai
  provider-anthropic
  provider-xai
  provider-ollama
  provider-openai-compatible
  provider-chat-completions
  local-harness
  provider-codex-app-server
  provider-grok-acp
)

# `--list` prints the provider lanes as a JSON array so a caller that fans the
# lanes across separate runners, each with its own `target/`, derives the set
# from this script instead of restating it.
if [[ "${mode}" == "--list" ]]; then
  if [[ $# -gt 1 ]]; then
    usage
    exit 2
  fi
  printf '['
  for index in "${!providers[@]}"; do
    if [[ "${index}" -gt 0 ]]; then
      printf ','
    fi
    printf '"%s"' "${providers[${index}]}"
  done
  printf ']\n'
  exit 0
fi

case "${mode}" in
  test|check|clippy|doc) ;;
  *)
    echo "provider-lanes: unsupported mode: ${mode}" >&2
    exit 2
    ;;
esac

if [[ -n "${selected}" ]]; then
  found=false
  for provider in "${providers[@]}"; do
    if [[ "${provider}" == "${selected}" ]]; then
      found=true
      providers=("${selected}")
      break
    fi
  done
  if [[ "${found}" != true ]]; then
    echo "provider-lanes: unsupported provider feature: ${selected}" >&2
    exit 2
  fi
fi

repository_root=$(git rev-parse --show-toplevel)
cd "${repository_root}"

for provider in "${providers[@]}"; do
  lane_started=${SECONDS}
  features="sqlite,${provider}"
  echo "provider-lanes: ${mode} ${provider}"
  case "${mode}" in
    test)
      # The ordinary AI library and its large unit-test harness can otherwise
      # compile together and exhaust a hosted runner. Build the harness first;
      # retain normal build parallelism for the remaining, smaller targets.
      CARGO_BUILD_JOBS=1 cargo test -p graphql-orm-ai --no-default-features \
        --features "${features}" --locked --lib --no-run
      if [[ "${GRAPHQL_ORM_AI_PRUNE_TEST_EXECUTABLES:-0}" == 1 ]]; then
        # Cargo identifies the exact executables for this lane, including fresh
        # cached artifacts. Prune only after all tests/doc-tests have succeeded;
        # retain dependency libraries, build scripts and other packages' tests.
        artifacts=$(mktemp)
        trap 'rm -f "${artifacts}"' EXIT
        cargo test -p graphql-orm-ai --no-default-features --features "${features}" \
          --locked --message-format=json | tee "${artifacts}"
        python3 - "${artifacts}" <<'PYTHON'
import json
from pathlib import Path
import sys

executables = set()
for line in Path(sys.argv[1]).read_text().splitlines():
    try:
        artifact = json.loads(line)
    except json.JSONDecodeError:
        continue  # Test harness output is not Cargo JSON.
    if not isinstance(artifact, dict):
        continue
    if (artifact.get("reason") == "compiler-artifact"
            and artifact.get("profile", {}).get("test")
            and artifact.get("executable")):
        path = Path(artifact["executable"])
        if path.parent.name == "deps" and path.parent.parent.name == "debug":
            executables.add(path)
for path in executables:
    path.unlink(missing_ok=True)
print(f"provider-lanes: pruned {len(executables)} completed test executable(s)")
PYTHON
        rm -f "${artifacts}"
        trap - EXIT
      else
        cargo test -p graphql-orm-ai --no-default-features --features "${features}" --locked
      fi
      ;;
    check)
      cargo check -p graphql-orm-ai --all-targets --no-default-features \
        --features "${features}" --locked
      ;;
    clippy)
      cargo clippy -p graphql-orm-ai --all-targets --no-default-features \
        --features "${features}" --locked -- -D warnings
      ;;
    doc)
      RUSTDOCFLAGS="-D warnings -D missing_docs" \
        cargo doc -p graphql-orm-ai --no-default-features \
          --features "${features}" --no-deps --locked
      ;;
  esac
  echo "provider-lanes: ${mode} ${provider} passed in $((SECONDS - lane_started))s"
done

echo "provider-lanes: ${mode} passed for ${#providers[@]} explicit lane(s)"
