#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
NORMAL=graphql-orm-smb-test
ENCRYPTED=graphql-orm-smb-encrypted
PASSWORD='BackupTest-42!'

cleanup() {
  docker rm -f "$NORMAL" "$ENCRYPTED" >/dev/null 2>&1 || true
  docker volume rm graphql-orm-smb-test-data >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup

docker volume create graphql-orm-smb-test-data >/dev/null
docker run -d --rm --name "$NORMAL" -p 1445:445 \
  -v graphql-orm-smb-test-data:/share dperson/samba:latest \
  -p -w WORKGROUP -u "backup;$PASSWORD" \
  -s 'backups;/share;yes;no;no;backup' \
  -g 'server signing = mandatory' >/dev/null
docker run -d --rm --name "$ENCRYPTED" -p 1446:445 dperson/samba:latest \
  -p -w WORKGROUP -u "backup;$PASSWORD" \
  -s 'backups;/share;yes;no;no;backup' \
  -g 'server signing = mandatory' -g 'smb encrypt = required' >/dev/null

sleep 2

cd "$ROOT"
SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT=1445 SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_DOMAIN=WORKGROUP \
  cargo test --features smb --test smb_integration \
  samba_rejects_invalid_password -- --ignored --nocapture

SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT=1445 SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_DOMAIN=WORKGROUP \
  cargo test --features smb --test smb_integration \
  samba_round_trip_streaming_listing_and_atomic_create -- --ignored --nocapture

SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT=1446 SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_REQUIRE_ENCRYPTION=1 \
  cargo test --features smb --test smb_integration \
  samba_round_trip_streaming_listing_and_atomic_create -- --ignored --nocapture

cd "$ROOT/../graphql-orm-backup"
SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT=1445 SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_DOMAIN=WORKGROUP \
  cargo test --no-default-features --features smb --test smb_repository -- --ignored --nocapture

cd "$ROOT"
SMB_TEST_SERVER=127.0.0.1 SMB_TEST_PORT=1445 SMB_TEST_SHARE=backups \
SMB_TEST_USERNAME=backup SMB_TEST_PASSWORD="$PASSWORD" SMB_TEST_DOMAIN=WORKGROUP \
SMB_TEST_CONTAINER_NAME="$NORMAL" \
  cargo test --features smb --test smb_integration \
  samba_reconnects_after_server_restart -- --ignored --nocapture
