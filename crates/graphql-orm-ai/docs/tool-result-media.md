---
title: "Protected tool-result images and source disclosure"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-10-13
review_by: 2027-02-01
supersedes: []
---

# Protected tool-result images and source disclosure

A registered tool can return an opaque reference to an image held by its owning
application. Reference JSON is descriptive metadata. It does not provide image
content or prove authorization. Provider delivery uses verified JPEG/PNG bytes
as typed image content, separately from the tool's JSON text.

## Static contract

Attach `AiToolResultMediaPolicy` to a compiled `AiGraphqlToolProfile` or registered
`AiToolDescriptor`. Its logical resolver ID, exact result-object paths, selected
metadata fields, classification and limits enter the immutable tool fingerprint.
Paths have no URL, filesystem path, wildcard or list traversal. Metadata fields
must be selected exportable scalars; a Secret or NeverExport ancestor cannot
be made exportable by declaring media.

`AiToolImageFieldBindings` names the opaque UUID, SHA-256, MIME type, width,
height, capture time and original retention deadline. An optional action deadline
is a historical label. Retention and action freshness are independent: reopening
an image never refreshes action evidence. No image requires a null reference and
null digest/MIME/dimensions; mixed absent-reference/non-null-image metadata fails.

Policy limits bound one through four images, at most 16 MiB total raw bytes,
8192 pixels per edge, 16 million aggregate decoded pixels per complete result,
original retention at most one
day, and one resolution pass at most five seconds. Hosts should choose narrower
limits. Actual digest, codec and decoded dimensions are checked. Asynchronous
decode uses a finite two-worker pool, refuses a busy queue and discards timed-out
results while retaining the permit until bounded work ends.

## Trusted owner and current source policy

Register `AiRuntimeBuilder::tool_result_media_resolver` with an
`AiToolResultMediaResolver`. It receives the freshly resolved principal, exact
registered descriptor, SDK-derived `AiToolResultDisclosureContext` and exact
retained image reference. The host opens its own protected resource and returns
`AiResolvedToolImage::decode_bounded`. It must independently compare its actual
owning records with the original invocation, principal/tenant, session/run/call,
provider profile/model/registration, argument and tool fingerprints, original
effect attempt/generation, digest and resource identity. Caller JSON, provider
URLs and path names cannot select resource authority.

The context has no public constructor or deserializer. Original effect identity
and current disclosure attempt/generation are separate. It contains safe
principal references, never bearer credentials. A retained provider cursor digest
is captured from the owning admitted binding before effect admission, rather than
being relabelled from a later rebind. Images and original output are reopened
under current protection, access and uncancelled run fences; no effect is rerun.

`AiToolExecutionProvenance::attempt_id` and `lease_generation` name the original
provider admission. Optional `effect_attempt_id` and `effect_lease_generation`
are minted only by the owning SDK from the actual executing lease immediately
before invocation, after approval consumption where required. Retention also keeps
exact authorization policy/digest returned by the completed trusted runtime, so
its provenance matches the actual admitted executor binding. They remain fixed
in the captured and protected result, and must match the separate retained effect
fence when reopened. Missing or deserialized claims grant no effect authority.
Old absent-field serialization remains unchanged.

Override `AiEgressPolicy::authorize_tool_sources` to check current grants and
resource policy for ToolResult, ImageAnalysis and later ModelInference/history.
Its default denies images and Restricted sources. A narrow
`AiToolResultEgressBoundary` may admit one exact static tool fingerprint, selected
provider profile and destination. General boundaries still govern ordinary,
unknown and mixed sources. Secret remains denied; scoped policies cannot multiply
aggregate request bytes or image counts. The current source hook intersects the
static boundary on every disclosure.

## Durable results, provider delivery and budgets

The application service protects and saves the actual bounded completed-effect
result before fresh scope/session disclosure authorization, parsing descriptive image
metadata or opening resources. Losing disclosure access preserves the original
completed output, while the exact live executing owner lease remains required
for custody; cancelled or obsolete leases cannot revive a result. Invalid
metadata preserves the saved output for owning recovery and denies disclosure; it does
not replace the completed output or authorize effect replay. Source metadata is
then admitted and protected before disclosure. Owner cancellation also closes
source-retained unfinished calls while preserving their original saved output.
A small internal source-address index bounds retained
native-history lookup to 64 exact cursor sources; it grants no authorization and
is never exposed as public CRUD. Owner loaders reopen the protected call,
arguments and result, compare full provenance and canonical checksums, and
require the current registered disclosure schema. Source-retained results remain
available for checked reopening even when first disclosure fails. No fallback
re-executes an effect.

`AiProviderCallExecutor::with_tool_result_source_service` binds the owning source
loader to the same runtime. Dynamic execution supplies the same owning service.
Media-required tools fail adapter/policy/capacity preflight before effect
admission. Adapters also default-deny source delivery for declared
Restricted/media effects unless they implement the final current source guard;
Codex requires both an explicit factory `supports_tool_result_source_delivery`
attestation and `AiCodexAppServerRegistration::with_guarded_tool_result_source_delivery`.
The enabled-only registration marker invalidates old capability-session bindings;
derive and admit the exact new binding afterward. Legacy text/dynamic-tool factories
remain compatible but cannot admit protected effects. The factory must preserve
both final input and responder guards through every queue.
No-effect native approval/control receipts contain no images and release their
unused media allocation while retaining ordinary protected receipt admission.
The existing atomic provider-turn reservation allocates image units
before callbacks. Callbacks share one count and 16 MiB raw-byte ceiling, including
ordinary attachment capacity already consumed in that provider turn. Reopened
history and ordinary attachments are checked as one complete request against
existing attachment limits and the image quote. ModelInference admission includes
the exact protected image sources, combined attachment count and conservative
base64/frame byte overhead; no separate image proof can multiply those limits. Unused
static capacity is released only after a definite completed result; ambiguous
provider delivery keeps the reservation uncertain. Settlement must include
`AiProviderUsageObservation::tool_result_image_units()`; provider-reported zero
cannot erase SDK-derived image usage.

Adapters await `ProviderRequestContext::authorize_tool_source_delivery` after
preparation/queueing and immediately before model/history transport. Its owning
callback is privately installed after actual source/body admission and reopens
current grants, resource bytes and run fences. The privately derived
`retained_tool_result_images()` getter contains actual reopened original pixels
for typed continuation input, rather than assuming the provider retained a
previous successful image delivery. Codex turn input carries the same delivery
check through the process queue; hosts await its `authorize_tool_source_delivery`
immediately before `turn/start`. A general/ordinary denial remains
closed rather than being reclassified as unavailable history.

Adapters await `ProviderDynamicToolResponder::authorize_delivery` immediately
before encoding/sending the exact result. Its compatibility default refuses
images. The owning implementation freshly reopens the retained output, source,
resource and bytes and checks current consent/cancellation. The experimental
Codex transport emits real `inputImage` items from verified inline bytes plus
capture/deadline labels; base64 is never inserted into result JSON/text and no
source URL is fetched. Other adapters fail closed until they explicitly implement
reviewed image-policy and typed-content support.

Retained history preserves original source classification and current-grant
checks, including after remote thread reuse. Current proof and image reopening
have a finite whole-pass deadline. If historical content expires or loses
admission before dispatch, the unstarted reservation is released, the old cursor
requires cleanup, and a fresh tool-free turn can contain independently reopened
current human input, ordinary owned attachments and a truthful withholding
notice. Prior model text, tree JSON and tool output are removed; saved effect
status remains durable. Restoring or historical reopening does not renew grants,
action freshness or media retention.

## Compatibility and schema

Use AI 0.107.0 and tool profiles 0.16.0 from one reviewed workspace release.
Absent media policy serializes identically to the previous descriptor format;
media declarations change its fingerprint. Schema module 0.72.0 adds protected
source metadata and the internal source-address table. Existing results without
source metadata retain the general disclosure path and gain no scoped allowance.
Apply the owned AI schema migration and restore reconciliation before opening the
runtime. See the [migration guide](../MIGRATION.md), [attachments](attachments.md)
and [recovery contract](recovery-and-restore.md).
