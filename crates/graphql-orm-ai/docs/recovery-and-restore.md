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
runtime closure after restore are implemented. A restored runtime reopens only
after operator acknowledgement; complete restore audits and privileged
uncertain-effect resolution remain blocked.

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
| Restored run that was not terminal, including queued and unstarted runs | `RecoveryRequired` under a new lease generation; never requeued or live-resumed |
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
flag is set, and its `recorded_locally_at` marker is present. The marker is
set on every row this deployment records and is excluded from portable
backups, so every row restored from a portable backup is neither live nor an
acknowledgeable restore closure. An empty ledger is not live either.

The ledger is enforced in three places:

- `AiRuntimeStartGate::open` accepts only an `AiRuntimeReadinessProof`.
  `OrmAiRuntimeRecoveryService::readiness_proof` mints it from a live current
  epoch for the compiled module fingerprint. A host-built
  `AiRuntimeReadinessReport` cannot open the gate.
- Run claims, approved-wait claims, expired-lease recovery, and the
  subscription-wait, OpenAI background and session-title claim paths read the
  current epoch inside their own state-machine transactions and refuse with
  `RuntimeNotReady` when it is not live. A granted run lease records the
  epoch in `recovery_epoch_id`. Every lease-fenced write, approval
  consumption included, requires that epoch to be the store's current live
  epoch inside its transaction.
- Gate-guarded tool, provider and OpenAI background services require the
  lease's epoch, or for a background reconciliation claim the store's live
  epoch, to equal `AiRuntimeStartGate::epoch_id`. A gate opened with a proof
  from another store cannot drive this store's work.

`readiness_proof` behaves as follows:

| Current epoch | Outcome |
|---|---|
| Live, same module fingerprint | Proof for that epoch; nothing is written |
| Live, earlier module fingerprint | Live successor epoch; no repairs |
| None, and no runs, approvals or egress consents (a new store) | First live epoch |
| None, over existing runs, approvals or consents | `RuntimeNotReady`; the operator chooses in-place upgrade or restore |
| Not live (restored, or a restore closure awaiting acknowledgement) | `RuntimeNotReady` |

An empty ledger over existing state is either a deployment upgraded in place
from 0.127.0 or earlier, or a backup taken before the deployment ever recorded
an epoch, including a 0.127-era backup or a snapshot whose ledger was
stripped. The crate cannot tell them apart, so the operator chooses:

- **Upgraded in place:** call `acknowledge_in_place_upgrade` once. It
  succeeds only while the ledger is empty, records the first live epoch with
  the operator subject and reason plus an
  `ai.runtime.in_place_upgrade_acknowledged` audit event, and expires leases
  granted before epochs existed so their workers re-claim through ordinary
  lease recovery. Runs, approvals and egress consents are otherwise left as
  they are. A repeat call is refused with `Conflict`.
- **Restored from a backup:** use reconcile + acknowledge instead, as below.

Choosing the in-place path for a restored store re-opens consumed approvals,
because approval consumptions, consent revocations and run progress made
after the backup were rolled back with it.

Two further properties are deliberate:

- A module upgrade records a live successor epoch. Leases granted under the
  previous epoch stop validating, so in-flight runs fail their next fenced
  write and are reclassified by ordinary lease recovery once they expire.
- The current epoch is the row with the highest `epoch_sequence`. Recorded
  epochs start at 1 and the column is uniquely indexed. A row with a crafted
  sequence at or below 0, or at `i64::MAX`, cannot be followed by a valid
  successor, so the store stays closed rather than reopening.

## Reopening after a restore

After any restore, stop every worker and writer, then:

1. **Reconcile.** `reconcile_restore` collects facts from its own database
   with the supplied collector's audit configuration, builds the collected
   plan, and in one generated-ORM state-machine transaction:
   - expires every pending, approved and resume-claimed approval, with an
     `ai.approval.expired` audit event;
   - revokes every unrevoked egress consent, with an
     `ai.egress_consent.revoked` audit event;
   - moves every non-terminal run, queued and unstarted runs included, to
     `RecoveryRequired` with its attempt and lease cleared, its lease
     generation incremented, and its terminal run event appended. Nothing is
     requeued, so work that ran after the backup is not repeated
     automatically;
   - verifies that no reusable approval, unrevoked consent or held lease
     remains; and
   - records a `restore_closed` epoch with the facts and plan digests and
     redacted closure counts. It is never live.

   A repeat call that finds nothing to change returns the existing closure
   epoch instead of recording another. Exceeding a bound aborts the
   transaction and changes nothing. Closure never calls a provider or
   application tool.
2. **Review** the plan, including fatal issues for the audits listed below,
   and the runs moved to `RecoveryRequired`.
3. **Acknowledge.** `acknowledge_reviewed_restore` takes the closure epoch,
   the operator subject and a reason. It succeeds only when that epoch is the
   store's current epoch and was recorded by `reconcile_restore` on this
   deployment, so a closure row restored from a backup is refused. It
   re-checks that no reusable approval or held lease exists, records the
   subject and reason in the ledger with an `ai.runtime.restore_acknowledged`
   audit event, and records a new live epoch. `readiness_proof` then works
   again.
4. **Retry deliberately.** Closed runs stay `RecoveryRequired`; retry them
   through the run-disposition service only after deciding that repeating
   them is safe.

A file-level or physical database restore carries the `recorded_locally_at`
marker with the rows, so the crate cannot detect it. Run the same steps after
any such restore, before starting a worker. Use the same steps for a store
restored from a pre-epoch backup; never `acknowledge_in_place_upgrade`.

## Remaining restore work

The collector reports the encryption-key, attachment object-byte,
usage/counter, rule, skill, checkpoint, provider, UI-intent, retention, and
stream audits as not implemented, so every collected plan is fatal. The
acknowledgement is the operator's explicit acceptance of those unaudited
categories. Still missing are:

1. a manifest-verified backup and exact referenced-object restore through
   `graphql-orm-backup` into a test-owned empty target;
2. the remaining collectors listed above;
3. a complete post-apply validator beyond runtime closure; and
4. an owned disposable PostgreSQL round trip of the full chain.

`graphql-orm-backup` 0.7.1, ORM 0.22.0, and storage 0.6.1 resolve through one
workspace and one database/metadata/storage type universe. AI schema module
0.58.0 also includes finalized local attachment and artifact object keys in
the confidential database export while continuing to redact quarantine,
upload-token, provider, credential, and secret references. Consumer owners
must still rehearse their composed schema and object store separately.

The `graphql-orm-*` packages already share this workspace. Continue restore
work on the integration branch, keep temporary coordination only in ignored
root `.handoff/`, and run the complete dependency and backend matrix for every
cross-package change.
