#!/usr/bin/env bash
# Resolve a standalone consumer without copying any workspace/fixture lockfile.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/router-fresh-consumer.XXXXXX")
cargo_runner=${ROUTER_CONSUMER_CARGO:-cargo}
mkdir -p "$fixture/src"
python3 - "$root" "$fixture" <<'PY'
import json
from pathlib import Path
import sys

root, fixture = map(Path, sys.argv[1:])
(fixture / "Cargo.toml").write_text(
    '[package]\nname = "router-fresh-consumer"\nversion = "0.0.0"\nedition = "2024"\n'
    '[dependencies]\ngraphql-orm-router = { path = '
    + json.dumps(str(root / "crates/graphql-orm-router")) + ' }\n'
)
(fixture / "src/lib.rs").write_text('pub use graphql_orm_router::RouterConfig;\n')
assert not (fixture / "Cargo.lock").exists()
print(f"Fresh router consumer (no input lockfile): {fixture}")
PY
"$cargo_runner" doc --manifest-path "$fixture/Cargo.toml" -p graphql-orm-router --no-deps
"$cargo_runner" metadata --manifest-path "$fixture/Cargo.toml" --locked --format-version 1 > "$fixture/metadata.json"
python3 - "$fixture/metadata.json" <<'PY'
import json
from pathlib import Path
import sys

packages = json.loads(Path(sys.argv[1]).read_text())["packages"]
datadog = [p["version"] for p in packages if p["name"] == "datadog-opentelemetry"]
sdk = [p["version"] for p in packages if p["name"] == "opentelemetry_sdk"]
assert datadog == ["0.5.2"], datadog
assert len(sdk) == 1 and sdk[0].startswith("0.32."), sdk
print(f"Fresh rustdoc passed: Datadog {datadog[0]}, one OpenTelemetry SDK {sdk[0]}")
PY
"$cargo_runner" tree --manifest-path "$fixture/Cargo.toml" --locked -i datadog-opentelemetry
"$cargo_runner" tree --manifest-path "$fixture/Cargo.toml" --locked -i opentelemetry_sdk
# Retain the fresh manifest, generated lock and metadata for inspection. Builds
# use the caller's Cargo target configuration; no fixture target is requested.
