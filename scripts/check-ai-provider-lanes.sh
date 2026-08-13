#!/usr/bin/env bash
set -euo pipefail

if [[ $# -gt 2 ]]; then
  echo "usage: scripts/check-ai-provider-lanes.sh [test|check|clippy|doc] [provider-feature]" >&2
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
  local-harness
  provider-codex-app-server
)

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
  features="sqlite,${provider}"
  echo "provider-lanes: ${mode} ${provider}"
  case "${mode}" in
    test)
      cargo test -p graphql-orm-ai --no-default-features --features "${features}" --locked
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
done

echo "provider-lanes: ${mode} passed for ${#providers[@]} explicit lane(s)"
