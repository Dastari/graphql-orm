#!/usr/bin/env bash
# Verifies graphql-orm-ai's provider TLS selection: the ring build carries no
# aws-lc or QUIC dependency, every TLS stack compiles with every adapter, and
# zero or two TLS features fail at compile time with the documented message.
set -euo pipefail

usage() {
  echo "usage: scripts/check-ai-provider-tls.sh [graph|check|guard|all]" >&2
}

if [[ $# -gt 1 ]]; then
  usage
  exit 2
fi
mode=${1:-all}
case "${mode}" in
  graph|check|guard|all) ;;
  *)
    usage
    exit 2
    ;;
esac

repository_root=$(git rev-parse --show-toplevel)
cd "${repository_root}"

adapters=provider-openai-core,provider-anthropic-core,provider-xai-core,provider-ollama-core,provider-openai-compatible-core
forbidden=(aws-lc-rs aws-lc-sys quinn jni)
# cargo tree resolves the host target only unless asked; check the shipped ones.
targets=(x86_64-unknown-linux-gnu x86_64-pc-windows-msvc aarch64-apple-darwin)

graph() {
  local tree target
  for target in "${targets[@]}"; do
    tree=$(cargo tree -p graphql-orm-ai --locked --no-default-features \
      --features "sqlite,${adapters},provider-tls-ring" -e normal --prefix none \
      --target "${target}")
    for package in "${forbidden[@]}"; do
      if grep -Eq "^${package} v" <<<"${tree}"; then
        echo "provider-tls: the ring build for ${target} depends on ${package}" >&2
        exit 1
      fi
    done
    if ! grep -Eq '^ring v' <<<"${tree}"; then
      echo "provider-tls: the ring build for ${target} does not depend on ring" >&2
      exit 1
    fi
    echo "provider-tls: ${target} ring build excludes ${forbidden[*]}"
  done
}

check() {
  for tls in provider-tls-ring provider-tls-native provider-tls-aws-lc; do
    echo "provider-tls: check ${tls}"
    cargo check -p graphql-orm-ai --locked --all-targets --no-default-features \
      --features "sqlite,${adapters},${tls}"
  done
}

expect_guard() {
  local features=$1
  local output
  if output=$(cargo check -p graphql-orm-ai --locked --no-default-features \
    --features "${features}" 2>&1); then
    echo "provider-tls: ${features} compiled but must be refused" >&2
    exit 1
  fi
  if ! grep -q 'graphql-orm-ai provider adapters' <<<"${output}"; then
    echo "${output}" >&2
    echo "provider-tls: ${features} failed without the TLS guard message" >&2
    exit 1
  fi
  echo "provider-tls: ${features} refused by the TLS guard"
}

guard() {
  expect_guard "sqlite,provider-openai-compatible-core"
  expect_guard "sqlite,provider-openai-compatible-core,provider-tls-ring,provider-tls-native"
}

case "${mode}" in
  graph) graph ;;
  check) check ;;
  guard) guard ;;
  all)
    graph
    guard
    check
    ;;
esac
