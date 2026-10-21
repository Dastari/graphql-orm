#!/usr/bin/env bash
set -euo pipefail

STORAGE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORKSPACE_ROOT="$(cd "$STORAGE_ROOT/../.." && pwd)"
# Each run owns a fresh namespace; never remove or reuse pre-existing fixtures.
RUN_ID=$(python3 -c 'import uuid; print(uuid.uuid4().hex)')
LABEL=graphql-orm.samba-fixture-run
NORMAL="graphql-orm-smb-${RUN_ID}"
ENCRYPTED="graphql-orm-smb-encrypted-${RUN_ID}"
CONSTRAINED="graphql-orm-smb-constrained-${RUN_ID}"
VOLUME="graphql-orm-smb-data-${RUN_ID}"
IMAGE=${SMB_TEST_IMAGE:-dperson/samba:latest}
PULL_POLICY=${SMB_TEST_PULL_POLICY:-never}
SMB_TEST_CARGO=${SMB_TEST_CARGO:-cargo}
PASSWORD='BackupTest-42!'
CLEANUP_READY=false
case "$PULL_POLICY" in
  never|missing) ;;
  *) echo 'SMB_TEST_PULL_POLICY must be never or missing' >&2; exit 64 ;;
esac

cleanup() {
  local status=$? id owner ids
  trap - EXIT
  if [[ $CLEANUP_READY != true ]]; then exit "$status"; fi
  # Discover only this invocation's exact label, including a create whose response
  # was interrupted. Validate ownership again before deleting each immutable ID.
  if ! ids=$(docker ps -aq --no-trunc --filter "label=$LABEL=$RUN_ID"); then
    echo 'Could not list owned Samba fixtures; no broad cleanup attempted' >&2
    status=1
  else
    for id in $ids; do
      owner=$(docker inspect --format "{{index .Config.Labels \"$LABEL\"}}" "$id") || { status=1; continue; }
      if [[ $owner == "$RUN_ID" ]]; then
        docker rm -fv "$id" >/dev/null || status=1
      else
        echo "Refusing fixture ownership mismatch: $id" >&2
        status=1
      fi
    done
  fi
  if docker volume inspect "$VOLUME" >/dev/null 2>&1; then
    owner=$(docker volume inspect --format "{{index .Labels \"$LABEL\"}}" "$VOLUME") || owner=
    if [[ $owner == "$RUN_ID" ]]; then
      docker volume rm "$VOLUME" >/dev/null || status=1
    else
      echo "Refusing volume ownership mismatch: $VOLUME" >&2
      status=1
    fi
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
for name in "$NORMAL" "$ENCRYPTED" "$CONSTRAINED"; do
  if docker inspect "$name" >/dev/null 2>&1; then
    echo "Fixture name already exists; refusing reuse: $name" >&2
    exit 1
  fi
done
if docker volume inspect "$VOLUME" >/dev/null 2>&1; then
  echo "Fixture volume already exists; refusing reuse: $VOLUME" >&2
  exit 1
fi
CLEANUP_READY=true
# Default local execution requires a reviewed cached image; disposable hosted CI
# explicitly opts into pulling a missing image.
if [[ $PULL_POLICY == never ]]; then docker inspect --type image "$IMAGE" >/dev/null; fi
# Select explicit ports so Docker restart cannot assign a new ephemeral mapping.
# Sockets are released before Docker binds; a competing bind fails startup closed.
read -r NORMAL_PORT ENCRYPTED_PORT CONSTRAINED_PORT < <(python3 -c '
import socket
sockets = [socket.socket() for _ in range(3)]
for sock in sockets:
    sock.bind(("127.0.0.1", 0))
print(*(sock.getsockname()[1] for sock in sockets))
for sock in sockets:
    sock.close()
')
docker volume create --label "$LABEL=$RUN_ID" "$VOLUME" >/dev/null
NORMAL_ID=$(docker run -d --name "$NORMAL" --label "$LABEL=$RUN_ID" --pull="$PULL_POLICY" \
  -p "127.0.0.1:$NORMAL_PORT:445" -v "$VOLUME:/share" "$IMAGE" \
  -p -w WORKGROUP -u "backup;$PASSWORD" \
  -s 'backups;/share;yes;no;no;backup' -g 'server signing = mandatory')
ENCRYPTED_ID=$(docker run -d --name "$ENCRYPTED" --label "$LABEL=$RUN_ID" --pull="$PULL_POLICY" \
  -p "127.0.0.1:$ENCRYPTED_PORT:445" "$IMAGE" \
  -p -w WORKGROUP -u "backup;$PASSWORD" \
  -s 'backups;/share;yes;no;no;backup' \
  -g 'server signing = mandatory' -g 'smb encrypt = required')
CONSTRAINED_ID=$(docker run -d --name "$CONSTRAINED" --label "$LABEL=$RUN_ID" --pull="$PULL_POLICY" \
  -p "127.0.0.1:$CONSTRAINED_PORT:445" "$IMAGE" \
  -p -w WORKGROUP -u "backup;$PASSWORD" \
  -s 'backups;/share;yes;no;no;backup' \
  -g 'server signing = mandatory' -g 'smb2 max write = 65536' \
  -g 'max disk size = 64')
port_for() {
  docker port "$1" 445/tcp | python3 -c 'import sys; print(sys.stdin.read().strip().rsplit(":", 1)[1])'
}
[[ $(port_for "$NORMAL_ID") == "$NORMAL_PORT" ]]
[[ $(port_for "$ENCRYPTED_ID") == "$ENCRYPTED_PORT" ]]
[[ $(port_for "$CONSTRAINED_ID") == "$CONSTRAINED_PORT" ]]
printf 'Owned Samba run=%s containers=%s,%s,%s volume=%s ports=%s,%s,%s\n' \
  "$RUN_ID" "$NORMAL_ID" "$ENCRYPTED_ID" "$CONSTRAINED_ID" "$VOLUME" \
  "$NORMAL_PORT" "$ENCRYPTED_PORT" "$CONSTRAINED_PORT"
python3 - "$NORMAL_PORT" "$ENCRYPTED_PORT" "$CONSTRAINED_PORT" <<'PYREADY'
import socket, sys, time
for port in map(int, sys.argv[1:]):
    for attempt in range(30):
        try:
            with socket.create_connection(('127.0.0.1', port), timeout=1):
                break
        except OSError:
            time.sleep(1)
    else:
        raise SystemExit('owned Samba fixture did not become ready')
PYREADY

# Cargo exit zero can mean zero matching tests. Require the exact native case
# and completion; restart diagnostics may split the test-start/ok line.
run_test() {
  local expected=$1 output status
  shift
  if output=$("$@" 2>&1); then status=0; else status=$?; fi
  printf '%s\n' "$output"
  if (( status != 0 )); then return "$status"; fi
  printf '%s\n' "$output" | python3 -c '
import re, sys
text = sys.stdin.read()
summaries = re.findall(r"^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;", text, re.M)
if summaries != [("1", "0", "0")] or not re.search(r"^test " + re.escape(sys.argv[1]) + r" \.\.\.", text, re.M):
    raise SystemExit("expected exactly one completed native test: " + sys.argv[1])
' "$expected"
}

cd "$STORAGE_ROOT"
SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT="$NORMAL_PORT" SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_DOMAIN=WORKGROUP \
  run_test samba_rejects_invalid_password "$SMB_TEST_CARGO" test --locked --features smb --test smb_integration \
  samba_rejects_invalid_password -- --ignored --exact --nocapture

SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT="$NORMAL_PORT" SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_DOMAIN=WORKGROUP \
  run_test samba_round_trip_streaming_listing_and_atomic_create "$SMB_TEST_CARGO" test --locked --features smb --test smb_integration \
  samba_round_trip_streaming_listing_and_atomic_create -- --ignored --exact --nocapture

SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT="$NORMAL_PORT" SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_DOMAIN=WORKGROUP \
  run_test samba_oversized_chunk_cancellation_and_concurrency_matrix "$SMB_TEST_CARGO" test --locked --features smb --test smb_integration \
  samba_oversized_chunk_cancellation_and_concurrency_matrix -- --ignored --exact --nocapture

# Empty output from a successful find is clean; Docker/find failures are not.
if uploading=$(docker exec "$NORMAL_ID" find /share -path /share/.deleted -prune -o -type f -name '*.uploading' -print -quit); then
  if [[ -n $uploading ]]; then
    echo "cancelled or failed upload left an SMB temporary file" >&2
    exit 1
  fi
else
  status=$?
  echo "could not inspect owned Samba temporary-file cleanup" >&2
  exit "$status"
fi

SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT="$ENCRYPTED_PORT" SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_REQUIRE_ENCRYPTION=1 \
  run_test samba_round_trip_streaming_listing_and_atomic_create "$SMB_TEST_CARGO" test --locked --features smb --test smb_integration \
  samba_round_trip_streaming_listing_and_atomic_create -- --ignored --exact --nocapture

SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT="$CONSTRAINED_PORT" SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_DOMAIN=WORKGROUP \
SMB_TEST_EXPECT_MAX_WRITE=65536 \
  run_test samba_constrained_max_write_handles_oversized_input_chunk "$SMB_TEST_CARGO" test --locked --features smb --test smb_integration \
  samba_constrained_max_write_handles_oversized_input_chunk -- --ignored --exact --nocapture

cd "$WORKSPACE_ROOT/crates/graphql-orm-backup"
SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT="$NORMAL_PORT" SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_DOMAIN=WORKGROUP \
  run_test full_smb_snapshot_lifecycle_and_locking "$SMB_TEST_CARGO" test --locked --no-default-features --features smb --test smb_repository full_smb_snapshot_lifecycle_and_locking -- --ignored --exact --nocapture

cd "$STORAGE_ROOT"
SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT="$NORMAL_PORT" SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_DOMAIN=WORKGROUP \
SMB_TEST_CONTAINER_NAME="$NORMAL_ID" SMB_TEST_FIXTURE_RUN="$RUN_ID" \
  run_test samba_reconnects_after_server_restart "$SMB_TEST_CARGO" test --locked --features smb --test smb_integration \
  samba_reconnects_after_server_restart -- --ignored --exact --nocapture


# Actual SDK directory-stream regressions; the original seven cases above stay intact.
SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT="$NORMAL_PORT" SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_DOMAIN=WORKGROUP \
SMB_TEST_FIXTURE_RUN="$RUN_ID" \
  run_test sdk_directory_multibatch_eof_exact_names "$SMB_TEST_CARGO" test --locked --features smb --test smb_directory_stream \
  sdk_directory_multibatch_eof_exact_names -- --ignored --exact --nocapture

SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT="$NORMAL_PORT" SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_DOMAIN=WORKGROUP \
SMB_TEST_FIXTURE_RUN="$RUN_ID" \
  run_test sdk_directory_drop_releases_directory_arc "$SMB_TEST_CARGO" test --locked --features smb --test smb_directory_stream \
  sdk_directory_drop_releases_directory_arc -- --ignored --exact --nocapture

# Capacity observations: authenticated caller allocation on the normal share and
# the share-level size limit Samba applies on the constrained fixture.
SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT="$NORMAL_PORT" SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_DOMAIN=WORKGROUP \
SMB_TEST_FIXTURE_RUN="$RUN_ID" \
  run_test samba_capacity_observes_authenticated_share_allocation "$SMB_TEST_CARGO" test --locked --features smb --test smb_integration \
  samba_capacity_observes_authenticated_share_allocation -- --ignored --exact --nocapture

SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT="$CONSTRAINED_PORT" SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_DOMAIN=WORKGROUP \
SMB_TEST_FIXTURE_RUN="$RUN_ID" SMB_TEST_EXPECT_MAX_DISK_BYTES=67108864 \
  run_test samba_capacity_reports_share_size_limit "$SMB_TEST_CARGO" test --locked --features smb --test smb_integration \
  samba_capacity_reports_share_size_limit -- --ignored --exact --nocapture
