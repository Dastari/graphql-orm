---
title: "Recovery, Retention, Backup, and Restore"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-11-10
review_by: 2027-02-01
supersedes: []
---

# Recovery, Retention, Backup, and Restore

Status: Slice 4 audit complete; ordinary lifecycle closure and enforced
runtime closure after restore are implemented, while opening a restored
runtime and privileged uncertain-effect resolution remain blocked.

This guide classifies every current externally uncertain or content-bearing
state against the [canonical ordering proof](ordering-history.md). It does not
grant an operator authority to infer success, repeat an effect, release
uncertain budget, or reopen a restored runtime.

## Current lifecycle classification

| State/fact | Current outcome |
|---|---|
| Pre-provider expired lease | Requeue under a new attempt/generation |
| Exact final-output checkpoint | Finalize the already durable output |
| Exact complete read-only batch | Requeue only for current-authority adoption |
| Exact complete single supervised retained-result batch | Requeue only for current-authority adoption |
| Accepted OpenAI background response | Bounded exact terminal reconciliation |
| Ambiguous provider/resolver/mutation transport | `RecoveryRequired`; never replay |
| Uncertain budget without authoritative usage | Full reservation remains held |
| Live stateless approval wait | Bounded current-policy keep/cancel/recovery classification under its active in-attempt fence |
| Live retained approval wait | Lease-free exact parked checkpoint/outcome graph; confirm or cancel/recover, then fresh-attempt claim |
| Restored approval/tool/provider wait | `RecoveryRequired` under a new lease generation; never live-resumed |
| Restored pending/approved/resume-claimed approval | Expired by restore closure; never claimed or consumed |
| Restored unrevoked egress consent | Revoked by restore closure; the user consents again |
| Same-database replayable subscription wait | Reclaim only from its exact protected cursor and waiter fence |
| Portable-restored subscription wait | `RecoveryRequired`; protected cursor/outcome are deliberately redacted |
| Released attachment/local blob | Exact fenced cleanup; ambiguity retains reference |
| Provider-file artifact | Exact profile-bound deletion/absence proof or retained blocked state |
| Protected message/proposal/tool/approval/context/checkpoint content | Bounded age/deleting-session dependency proof or retained blocked state |
| Usage, pricing, audit, egress, run-attempt/outcome facts | Append-only retained security/accounting evidence |
| Deleted session shell | Empty-title hidden tombstone with redacted lifecycle evidence |

`RecoveryRequired` is itself a truthful retained-with-reason outcome. It is not
a queue state. Current code provides no generic operator mutation that changes
it, because provider truth and application mutation truth require different
authoritative evidence.

## Privileged uncertain-effect recovery contract

A future privileged recovery service may perform only one of these bounded
actions:

1. attach authoritative provider usage and settle the exact uncertain
   reservation while leaving the run closed;
2. record authoritative proof that transport never began and release the exact
   unused reservation while terminally closing the attempt; or
3. record a host-owned application-effect reference and retain the run,
   approval, tool, and budget facts for manual/domain reconciliation.

It must never synthesize assistant output, reconstruct a tool result, mark an
application mutation successful, requeue a consequential effect, reuse an
approval, or clear a provider/object reference. A host policy must authorize
the exact recovery action under a freshly rehydrated operator with recent MFA.
The request must bind the run, source attempt, generation, reservation,
provider/profile/model or tool/approval identity, expected row versions,
evidence kind, safe external evidence reference, and intended terminal
classification.

One generated-ORM state-machine transaction must revalidate the complete
durable graph, settle any authoritative budget/usage exactly once, append a
redacted immutable audit and recovery epoch, and leave no live lease. Exact
replay may return the already recorded decision; conflicting evidence remains
closed. A generic free-form “operator override” is permanently unsupported.

This service is not implemented yet. The existing provider-neutral budget
reconciler deliberately requires a current running fence, while the specialized
background terminal transaction owns only exact OpenAI response evidence.
Generalizing that arithmetic and recovery evidence is downstream work and does
not require an ORM/auth change.

## Retention truth

The current retention service distinguishes:

- logical session deletion;
- protected payload tombstoning;
- exact local/provider object deletion;
- append-only checkpoint maintenance;
- retained redacted security/accounting facts; and
- blocked dependencies.

It never reports complete erasure merely because a row became invisible.
Accepted proposals without a recorded applied outcome, nonterminal or
recovery-required runs, active/uncertain tool authority, unconsumed approvals,
ambiguous artifacts, current coordinator checkpoints, and over-bound histories
remain blockers. Append-only usage, pricing, audit, egress, attempts, and
attempt outcomes are retained. The complete details and bounded transaction
order are in [session retention](session-retention.md).

No further retention mutation is admitted while applied restore remains
unavailable: changing what a backup can no longer recover would make the
restore claim weaker, not more complete.

## Backup and applied restore boundary

`AiSchemaModule` exposes all private entity metadata, backup descriptors, the
module fingerprint, and ordered preflight/reconcile/validate/readiness hook
declarations. `AiRestoreReconciler` already produces a side-effect-free plan:

- terminal runs stay terminal;
- safe pre-effect or exact adopted-batch states may receive a new attempt;
- waits and uncertain effects become `RecoveryRequired`;
- provider continuations/files require re-verification;
- pending approval/consent counts remain explicit; and
- invalid encryption, attachment, usage, budget, pricing, skill, rule,
  checkpoint, webhook, background, provider-session, UI-intent, retention, or stream facts keep
  readiness closed.

The collector foundation is now implemented. `OrmAiRestoreFactCollector` uses
generated ORM queries in one bounded serializable transaction to derive
conservative run classifications and approval/egress-consent revalidation
candidates. With `AiRestorePolicyAuditLimits`, it also revalidates complete
budget-policy and immutable pricing-catalog graphs against host-attested
deployment ceilings and exact pricing creation-audit linkage. Those inputs do
not prove equivalence to the live service configuration; the future applied
validator must bind that configuration epoch. With
`AiRestoreAttachmentMetadataAuditLimits`, the same transaction can also prove
the bounded stable attachment/artifact lifecycle, exact session/message owner
graph, provider tuple shape, and unique safe local-object references. Live
upload/cleanup states and backup-redacted transient/provider references remain
invalid until explicitly repaired. The restore target and every writer must
remain closed throughout that transaction. The collector uses its own trusted
unbounded pagination view, contained by deployment-owned hard limits, so host
GraphQL page caps cannot silently truncate an audit. It returns opaque
`AiCollectedRestoreFacts` with a deterministic digest and an explicit status
for every required audit. Reaching a limit returns no partial evidence or
candidate actions for that category; observing an invalid row/graph, missing
deployment inputs, or an audit not yet implemented produces a fatal collected
plan; a caller-supplied zero is not substituted.

Provider-session cursors have a deliberately closed portable-restore rule.
Their protected field is backup-redacted, and the required
`ProviderSessionBindings` audit is complete only when the bounded database
query proves that no binding row exists. Any active, claimed, cleanup,
quarantined, or otherwise retained binding is invalid for portable restore and
keeps readiness closed. A redaction marker, cursor loss, or provider expiry is
not provider-absence proof. Hosts must drain retained sessions through the
exact deletion service before a portable backup intended for ready restore;
raw restore never resumes an external provider thread.

Encryption and object recovery deliberately remain separate. A current
content-protection policy row cannot prove historic application-encrypted
envelopes are openable: the future key audit must scan every protected envelope
header and bind a prevalidated set of current and historic deployment keys.
Likewise, `AttachmentMetadataGraph::Complete` cannot prove restored object
bytes. `AttachmentObjectBytes` remains a separate fatal audit until a verified
backup manifest is bound to the database evidence and every target BlobStore
object is streamed and rechecked for its exact key, byte count, and SHA-256.
Optional object metadata and a successful restore-sink call are insufficient.

## Recovery epochs and enforced closure

`OrmAiRuntimeRecoveryService` owns the recovery epoch ledger in
`graphql_orm_ai_runtime_recovery`. The row with the highest `epoch_sequence`
is the current epoch. It is live only when its state is `live`, its start gate
flag is set, and its `live_at` marker is present. `live_at` is excluded from
portable backups, so every restored epoch row is not live.

The ledger is enforced in two places:

- `AiRuntimeStartGate::open` accepts only an `AiRuntimeReadinessProof`.
  `OrmAiRuntimeRecoveryService::readiness_proof` mints it from a live current
  epoch for the compiled module fingerprint. A host-built
  `AiRuntimeReadinessReport` cannot open the gate.
- `OrmAiRunService::claim_next`, `claim_next_approved` and approval
  consumption read the current epoch inside their own state-machine
  transactions and refuse with `RuntimeNotReady` when it is not live. This
  holds even for a worker that never consults the in-memory gate.

`readiness_proof` behaves as follows:

| Current epoch | Outcome |
|---|---|
| Live, same module fingerprint | Proof for that epoch; nothing is written |
| Live, earlier module fingerprint | Live successor epoch; no repairs |
| None (new store or first open after upgrading) | First live epoch, after expiring reusable approvals and expiring active leases |
| Not live (restored or blocked) | `RuntimeNotReady`; run `reconcile_restore` |

The first-epoch closure exists because an empty ledger can also come from a
portable backup taken before the deployment ever opened its runtime.

`OrmAiRuntimeRecoveryService::reconcile_restore` is the restore applier. It
collects facts from its own database with the supplied collector's audit
configuration, builds the collected plan, and then, in one generated-ORM
state-machine transaction:

1. expires every pending, approved and resume-claimed approval, with an
   `ai.approval.expired` audit event;
2. revokes every unrevoked egress consent, with an
   `ai.egress_consent.revoked` audit event;
3. returns an unstarted first lease to `RetryScheduled` only when the plan and
   the current row both classify it as effect-free, leaves pristine queued
   runs unchanged, and moves every other non-terminal run to
   `RecoveryRequired` with its attempt and lease cleared, its lease generation
   incremented, and its terminal run event appended;
4. verifies that no reusable approval, unrevoked consent or held lease
   remains; and
5. records a new epoch with the facts and plan digests and redacted closure
   counts. The epoch is live only when the plan has no fatal issue.

Exceeding a deployment bound aborts the transaction and changes nothing.
Closure never calls a provider or application tool.

## Remaining restore work

Opening a restored runtime is still closed. The collector reports the
encryption-key, attachment object-byte, usage/counter, rule, skill,
checkpoint, provider, UI-intent, retention, and stream audits as not
implemented, so every collected plan is fatal and `reconcile_restore` records
a `blocked` epoch. The restored store keeps its closure repairs and stays
closed. Still missing are:

1. a manifest-verified backup and exact referenced-object restore through
   `graphql-orm-backup` into a test-owned empty target;
2. the remaining collectors listed above;
3. a complete post-apply validator beyond runtime closure; and
4. an owned disposable PostgreSQL round trip of the full chain.

`graphql-orm-backup` 0.7.1, ORM 0.22.0, and storage 0.6.1 resolve through one
workspace and one database/metadata/storage type universe. AI schema module
0.58.0 also includes finalized local attachment and artifact object keys in
the confidential database export while continuing to redact quarantine,
upload-token, provider, credential, and secret references.

A file-level or physical database restore carries the `live_at` marker with
the rows, so the crate cannot detect it. Run `reconcile_restore` after any
such restore, before starting a worker. Consumer owners must still rehearse
their composed schema and object store separately.

The `graphql-orm-*` packages already share this workspace. Continue restore
work on the integration branch, keep temporary coordination only in ignored
root `.handoff/`, and run the complete dependency and backend matrix for every
cross-package change.
