---
title: Protected authorized pagination
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-20
review_by: 2027-02-01
supersedes: []
---

# Protected authorized pagination

`EntityQuery::fetch_authorized_scan_with_context` is for custom resolvers that
need a bounded, policy-checked keyset scan over a server-owned dataset. It uses
the installed `AuthorizedScanConfig` and returns the same `AuthorizedScanPage`
and `ScanStatus` as `fetch_authorized_scan`. Its `goms2` continuation is
encrypted and authenticated by the ORM. Existing `goms1` scan cursors and
generated pagination are unchanged and are not accepted by this method.

On **every** request, construct `AuthorizedScanContext` from an unambiguous,
complete encoding of trusted current state. Include the committed dataset or
snapshot generation, source registration and incarnation, workspace or
partition, original and current principal and grant stamps, original credential
identity and expiry, and current credential, configuration and placement
revisions as applicable. Use length-delimited or canonical serialization; do
not concatenate ambiguous user-controlled strings. The caller decides the
members and obtains them from its current authority and catalogue. A cursor
never grants authorization. The ORM still evaluates current entity, row and
field policy on every page.

The binding must contain 1 to 2048 bytes and is supplied by trusted server
code, never a browser field. A changed binding rejects an old continuation
before querying rows. The protected path permits at most 1024 rows per batch,
10,000 examined rows per request, 1024 visible rows per page, and 32,768 cursor
characters. `PageFull` and `BudgetExhausted` may return an empty visible page;
only `Exhausted` means the underlying scan is complete.

`AuthorizedScanConfig::new` receives one random 32-byte service-domain secret.
Replicas that accept one another's cursors need the same key and key ID. New
protected cursors expire after 15 minutes by default; set an integral-second
duration from 1 second to 24 hours with `with_protected_ttl`. A rotation can
select a new active ID with `with_protected_key_id` and accept up to four old
keys through `with_retired_protected_key(id, key, accept_until)`. An old cursor
must pass both its embedded expiry and the old key's acceptance deadline.
Retiring a key or withholding it invalidates its remaining cursors. Key
delivery and storage belong to the host's protected secret custody; do not
derive the key from a provider credential.

Malformed, tampered, mismatched, unknown-version, wrong-key and expired
cursors produce the value-free `CURSOR_INVALID` error. Invalid context or page
bounds produce `INVALID_INPUT`; unavailable scan configuration fails closed.
The codec's plaintext, nonce and framing are private implementation details.
