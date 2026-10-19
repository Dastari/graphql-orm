---
title: Owned planning replay accounting
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-19
review_by: 2027-02-01
supersedes: []
---

# Owned planning replay accounting

See the [public contract](owned-planning-replay.md) for scope, limits and custody.
The following source-qualified schedule is the approved admission contract.

## Per-recipe compiler accounting

The historical recipe retains its exact 1.97.1 admission and accounting schedule.
The separately selected `graphql-orm-owned-planning-8e1-rust1981-v1` admits only
stock Rust 1.98.1 commit `48a229ceaefd4985c50990b14116b6d856af0985`, with the
same exact GNU x86_64 host/target and wrapper exclusion. Its native Debug bytes
are deliberately different where that compiler changes escaping. Neither recipe
is a formatter emulator or a portable hash.

Installed 1.98.1 `library/core/src/slice/sort/{stable,shared}` sources are
byte-identical to 1.97.1. BTree search/node/set implementations are unchanged;
map's Default const annotation and entry's fallible wrappers do not add a tree
pass, comparison, allocation or string copy. Existing 64(n+1) operation envelopes
cover their fixed dispatch. String/str call-path changes include constant-time
range/boundary refactoring (`core/src/str/traits.rs:116–338`) and removal of
redundant two-way-search bounds checks (`core/src/str/pattern.rs:1496–1630`),
without extra scans/candidate comparisons or text copies. ASCII normalization
and string-copy paths retain their bounds. New casefold APIs are not called by
the planning engine; unused UTF-8 boundary convenience changes add no helper
invocation. Integer formatter changes are const/stability/NumBuffer plumbing,
with the existing closed scalar/output envelopes retained. The source review digest manifest is retained alongside
the [fresh producer provenance](../../../crates/graphql-orm/tests/fixtures/owned-planning-replay-rust1981/README.md).

Native Debug needs an additional bound. In the exact 1.98.1 source:

- `core/src/fmt/mod.rs:2920–2960` scans printable ASCII and calls
  `char::escape_debug_ext` for other scalars.
- `core/src/char/methods.rs:478–503,1084,1144,1176,1218,1260,1274`
  adds fixed range/property checks and up to four skip searches for Cf,
  Cn_planes_0_3, Default_Ignorable_Code_Point and Grapheme_Extend.
- `core/src/unicode/unicode_data.rs:94–146` performs a header binary search and
  a bounded offset-run scan. The respective header/run maxima are 11/8, 54/332,
  12/8 and 33/244. Thus there are at most 592 offset iterations and eight header
  comparisons per search. Charge eight logical visits/arithmetic/comparisons per
  offset iteration, eight per header comparison, plus 512 fixed scalar/dispatch/
  range/escape events: `8*592 + 8*32 + 512 = 5504 < 8192` extra W per non-ASCII
  scalar. Each such scalar occupies at least two non-ASCII UTF-8 bytes.

Only the new recipe inventories `U(X)`, the number of non-ASCII stored UTF-8
bytes, preserving every original byte. Its string inventory prepays the scan
within the existing `128+4n` work reservation before traversal (byte visit,
ASCII predicate, iterator and count); historical inventories do no new byte scan.
Every Debug definition/profile pass and every semantic-hash step pass adds
`4096*U(X)` W **before** the helper, including repeated source/target/step
occurrences and future decode/encode/verify admission certificates. ASCII scalar,
number and ordinary type/field formatting retain the original allowances.
The `10B+256N` text envelope still dominates both native escape spellings, so
there is no text expansion exception or increased ceiling. Inventory size/copy/
comparison accounting includes the extra private descriptor's fixed operations
within existing per-slot allowances. New recipe dispatch and identifier comparison
fit the 4096 entry allowance; wire inventory/layout uses the actual recipe string
length, including every subsequent materialization/copy.

W remains 16,777,216 and T remains 67,108,864 per operation. These intersecting
ceilings can refuse otherwise valid inputs; the new lane's Unicode work does not
change historical admission or any ordinary planning entry point. New compiler
and wrong-recipe refusal properties are inspection-only, not runtime tested.

## Implementation envelopes

`owned_planning_replay/accounting.rs` uses checked u64 composition and a cumulative
ledger. Inventories guard each visit/string inspection before traversal, count
wire member names as extra slots/bytes, and never decide ownership or a diff.
The implementation intentionally overbounds several schedule terms together:

- Each ledger starts with 4096 work and 256 text for fixed entry/error overhead.
  A constant-time library qualification guard precedes ledger creation and every
  public capture/codec/replay operation. Its successful fixed work is included in
  this entry allowance. Missing qualification returns LimitExceeded immediately,
  without inspecting data or accessing the database. The build-time compiler
  query is outside per-operation W/T and never occurs in runtime library code.
  Inventory slots reserve 64 work before visiting; strings additionally reserve
  128+4n before length aggregation. These include ledger/bookkeeping field visits,
  checked arithmetic and comparisons, not just the model's own fields.
- `Size::copy` charges 3N work (clone plus closed-data cleanup) and B text.
- Profile checks reserve 64N+20B work and D text; effective intersected structural
  and Debug limits run in one check, without a second Debug traversal.
- Table-pair diff bounds sum each side's descriptors for every opposite table,
  without allocating pairs or matching names. All hidden collection sorts, FK
  union/group sorts and canonical-check sorts are separate terms.
- Renderer frames reserve 4096N+512B text and twice that work per actual step.
  The closed 8e1 SQL templates, fixed fragments and scalar formatting fit within
  4096 bytes per slot; 512 bytes per stored byte dominates quote/path/hex/name
  copies, search config's eight materialization layers and subsequent SQL
  escaping and frames. Normalizers are additional terms, never absorbed into a
  claim of linear byte cost. Each step additionally prepays the same full maximum
  table rebuild frame, its Cartesian column comparisons and four normalizer
  envelopes, including on PostgreSQL and on non-rebuild variants. There is no
  deduplication credit. Schema-hash work/text additionally reserve repeated
  joins/check joins/canonical buffer/hex materializations.
- Verification compares the actual typed diff with retained raw steps before
  rendering, SQL immediately after rendering, and risks/reasons after
  classification. Length-first closed comparisons charge retained structure;
  unmatched data returns ReplayMismatch before the next expanding phase. This
  lets admission use retained actual steps without trusting a decoded claimed
  step list or running an extra engine pass. Separate actual/expected inventory,
  early/final comparison and passive-result retention allowance is 512M(data)
  work plus the full-data clone envelope. The detailed schedule's single final
  exact comparison remains, with these additional prepaid short-circuit passes.
- Ownership length uses the private constant-time tree length; every remaining
  ownership iterator and cloned tree separately prepays 64(n+1) structural work.
  Identity byte bounds use the constructor's fixed 63-byte identifier ceiling,
  without extra uncharged tree passes. String-copy text remains separately paid.
- Certificate calculations are charged to the current ledger. Their conservative
  reservation also prepays the same decoder calculation and dominates verify's
  smaller inventory walks. Future decode additionally pays byte/node admission,
  ownership insertion, model/profile and target checks. Decode additionally
  reserves 2048N+32B work for its own descriptor/conversion walks and certificate
  tail beyond the measured certificate prefix (model inventory <=192N+4B and
  two target descriptor walks <=448N+4B each, plus remaining bookkeeping).
  Future encode pays its
  own traversal/escaping/output. Certificate estimation performs no engine pass.
- Fixed decoder admission precedes serde allocation; a nonallocating lexical scan
  checks depth/node/string ceilings. Typed adapters reject duplicate, missing and
  unknown members; there is no `Value` tree. A private deserializer/visitor/seed
  adapter uses fixed errors before serde's default diagnostics can format input.
  Parser-boundary bridges have empty messages, without repeated diagnostic text
  materialization; size hints are not propagated into typed allocations.
  Locked Serde 1.0.228 `src/private/de.rs` TaggedContentVisitor::visit_map
  (835–886) buffers all non-tag members even when the tag appears last;
  ContentDeserializer::deserialize_struct (1404–1418) accepts sequences and
  deserialize_enum (1420–1461) accepts single-member objects. Therefore every one
  of the 31 StepObject payload fields uses bounded_decode::payload to wrap that
  ContentDeserializer in Fixed before typed decoding. FixedSeed/FixedVisitor
  maintain the wrapper through every nested map, sequence, option and enum;
  MapOnly and StringEnumOnly enforce the complete wire's shapes there as at the
  original JSON boundary. StringOnly additionally guards identifiers, including
  the tag, against derive's numeric variant-index alternative. Unknown/duplicate/
  missing member errors still use FixedError before formatting. There is no
  additional Content/Value tree or text copy from this re-entry.
  Decoder admission now reserves 64 rather than 32 work per lexical node:
  the additional 32 cover the re-entry, identifier gate and their visitor/seed
  dispatches (one re-entry per payload field; each subsequent nested value/member
  has at most one additional Fixed/visitor/seed layer, with <=32 logical visits/
  moves/comparisons per node). These wrappers do not scan stored string bytes;
  the original 32*encoded_bytes work still pays byte parsing/member comparisons.
  Nodes <=2N in the wire inventory, so the capture certificate prepays 128N
  rather than 64N for future decoder node work. This conservative correction
  retains the fixed W/T ceilings and can refuse additional near-ceiling inputs.
  Borrowed encoders preserve field and
  vector order; an admitted layout adapter adds exact quote/control escape
  lengths and decimal digit counts without emitting JSON, formatted integers or
  escape fragments. Capture performs this arithmetic, never automatic encoding.
  Layout arithmetic and actual encoding each conservatively reserve twice their
  output bound; actual encoding includes local escape/number fragments as well
  as writer emission. Classification separately reserves deep StepRef
  construction and possible AlterColumn type comparisons before its traversal.
  Actual encoding is separately prepaid.

The envelopes are conservative implementation corrections within the approved
contract. They can reject otherwise valid small-result plans and add no ceilings
or counters to ordinary production planning. The detailed helper coverage below
remains normative; a helper or formatter outside this closed schedule requires
review rather than an unaccounted fallback.

```text
SOURCE BASIS

ORM 0.50.2:
8e1e5698437d9ea5c0d6ff2c0f29cf7af58d45a0

All ORM file/line anchors below refer to:
crates/graphql-orm/src/graphql/orm/
at that exact revision, not the owner's older dirty working checkout.

Toolchain-library accounting is qualified to the reviewed Rust 1.97.1 standard
library, compiler commit 8bab26f4f68e0e26f0bb7960be334d5b520ea452, stock
x86_64-unknown-linux-gnu host/target, with no compiler wrappers. The std-only
crate build.rs checks the exact compiler release/commit/host and Cargo target;
only the historical match emits graphql_orm_replay_qualified_profile.
The additional 1.98.1 recipe emits its separate graphql_orm_replay_rust1981_profile;
each public path checks the explicitly requested or privately stored recipe. A missing match
fails closed in all four library entry points, rather than relying on the
standalone inspection script. Ordinary planning has no profile gate.
Its installed source was inspected, including stable sort and BTree
collection implementations. Additional toolchains need their library accounting
reviewed as well as recipe-conformance evidence; sharing a helper name proves
neither accounting nor Debug/rendering compatibility.

A. COUNTERS AND UNITS

Use one nonrefundable W/T ledger per public operation, shared across all phases
of that operation. A phase reservation uses checked arithmetic and is added to
the existing totals before the expanding phase starts. Reservations are not reset
between helpers, and unused branch allowance is not refunded.

W ceiling = 16,777,216.
T ceiling = 67,108,864.
All wire/model/plan ceilings still apply independently.

One W unit is one logical event:
- entering/visiting a scalar, field, vector element, collection entry or edge;
- moving/copying one collection element/handle;
- examining one UTF-8 byte or Unicode scalar in a comparison, parser,
  normalization, escaping or hash pass (count both passes when both occur);
- one scalar/length/tag comparison or one checked arithmetic operation.

Straight bulk copies/emissions of already-known text pay T by byte and W for
operation/element visits, not an additional per-byte W write charge. Data-
dependent byte inspection still pays W. This keeps work and text distinct.

A compound comparison is not one unit: include every potentially visited
nested member and compared byte. Loop bodies and repeated calls are charged
each time. Hashing charges each input byte-step, plus setup/hex formatting.

T counts cumulative UTF-8 payload materialization/emission, not peak live bytes:
- New String/Vec<u8> text, clone/to_owned/to_string, replacement, formatting,
  joining, escaped fragments, SQL, JSON, token strings, Debug text and hash
  canonical buffers all count.
- Appending n bytes charges n, and copying the same fragment into another
  logical buffer charges those n bytes again.
- Moving an already-created String into a Vec or returning it adds no T.
- Moving String/reference headers during sorting or Vec shifts adds W but no T.
- Vec<char> text is charged by its represented UTF-8 payload as well as its
  scalar visits; its machine storage size is not this metric.
- Allocator capacity slack, internal realloc relocation, reference/struct memory,
  allocator/runtime machinery and machine instructions are outside T. This is
  explicitly not a bound on total memory, physical memory traffic or latency.

Every captured/decoded occurrence counts independently, even if strings are
equal or are references to an already-seen value. No content-based dedup credit.

The inventory/admission calculator is itself charged: guard its visits, byte
scans, checked arithmetic and descriptor-array growth before each event/growth.
It uses only counts, lengths, enum tags, maxima, sums and raw parenthesis counts.
It emits no SQL, steps, canonicalized values or hashes, and makes no diff or
ownership decision. Products can be evaluated from aggregate/max descriptors;
do not allocate Cartesian products or speculative plans.

For a subtree X:
- N(X): field/scalar/enum/container/element slots, including nested metadata.
- B(X): sum of stored UTF-8 string bytes, with repeated occurrences counted.
- M(X) = N(X) + B(X).
- For a string x: n = byte length and
  r = min(raw '(' count, raw ')' count, floor(n/2)).
These raw counts upper-bound successful outer-parenthesis stripping without
parsing or normalizing SQL.

B. COMPOSITION RULES

The following are normative rules for a small private size-accounting adapter.
They describe costs; they do not reproduce planner decisions.

1. Sequential expressions: sum their W and T.
2. A loop/iterator/zip/map/filter/find/any: reserve every candidate visit and
   complete possible body; no early-exit credit. Nested loops multiply/sum the
   corresponding bounds. A filtered collect reserves every possible retained
   item. Shared name lookups do not make later invocations free.
3. A conditional: charge its predicate and either the maximum arm or, more
   conservatively, the sum of all feasible arms. Backend selection is fixed.
4. Deep clone of X: W += 2*N(X), T += B(X); count actual repeated clones.
   A borrowed view charges its vector/reference construction but not string
   copies. See owned_schema.rs:56,180,249,299,381 and 1331.
   Newly owning aggregates also reserve a closed-data cleanup walk over their
   owned slots (including discarded duplicate keys/token temporaries), unless
   that exact walk is already included in the stated envelope. Cleanup adds W,
   not T. There are no user-defined model destructors; allocator machinery is
   excluded separately.
5. Equality/ordering: recursively charge a tag/length comparison plus every
   potentially compared field, element and byte. Thus a full structural
   comparison is bounded by 1 + M(X) + M(Y). IndexRef equality additionally
   charges its effective-direction loop; see owned_schema.rs:321.
   String ASCII-insensitive comparisons charge full byte scans on both operands.
6. For format!(literal, args), let K be the UTF-8 length of the Rust format
   literal and L_i the output-length bound of each argument occurrence.
   Charge argument computation, then a new output of at most
       F = K + sum_i L_i.
   Repeated placeholders each contribute. K deliberately includes placeholder
   spelling, so it overbounds literal punctuation. Charge literal-format parsing,
   argument/scalar visits and any child escaping/inspection to W, and F to T.
   Bare string Display copies are not an invented rescan of their payload.
   Integer output is bounded by 64 bytes
   unless the source fixes a smaller width (e.g. 16-hex hashes).
7. join(k items, separator): output length
       J = sum_i L_i + max(k-1,0)*separator_bytes.
   Charge child creation first, then J to T plus 2k W length/element visits.
   A push_str charges one W copy operation and its appended bytes to T, not a
   free reference. A newly formatted prefix+suffix charges the prefix again.
8. ASCII case conversion/replacement: charge the input scan and output
   materialization. One-to-two quote replacement has output <= 2n.
   String Debug <= 10n+2 bytes; JSON string <= 6n+2 bytes.
   For a complete typed schema/step subtree, a safe Debug envelope is
       D(X) = 256*N(X) + 10*B(X).
   The 256 allowance covers the pinned fixed field/type/variant names,
   punctuation and scalar spellings per slot; there are no arbitrary Debug
   trait objects or callbacks in these model types. The corresponding Debug
   traversal reserves 32*N(X)+16*B(X) W for field/scalar visits, decimal
   formatting and string/Unicode escape inspection, separately from its emitted
   text. Include nested borrowed-view construction too. These envelopes are
   qualified to the pinned closed types and reviewed formatter implementation.
9. FNV input of length l: charge l byte-steps, setup and the 16-byte hex output.
   Hashing does not create another copy of its borrowed input.

These rules also cover fixed literal strings and error-formatting arms.
The existing target validator may allocate an input-bearing Protocol error
before the new wrapper maps it to a fixed code. Prepay the maximum single such
error frame and its arguments for that invocation; never log it.

C. SORTS AND COLLECTIONS: NO ASSUMED CONSTANT-TIME LOOKUPS

Let C_K be the recursive worst key-comparison charge above, including nested
FK pairs, optional constraint names or search/path keys as appropriate.

For a stable sort of n elements on the reviewed standard library:
- n < 2: comparison bound c=0, handle/setup allowance m=1.
- 2 <= n <= 20:
      c = n*(n-1)/2;
      m = 2*n*n + 2*n.
- n > 20, q=ceil(log2(n+1)):
      c = 4*n*n + 8*n*q + 64*n;
      m = 4*c.

Reserve W_sort = m + c*(C_K+2), T_sort=0, plus any actual allocating key
extraction. The ORM's existing sorting closures borrow their keys; canonical
constraint/token construction is charged before sorting, not hidden in C_K.

This intentionally loose bound follows the inspected implementation:
- insertion sorting for <=20;
- at most n run starts and n-1 outer physical merges, each scanning <=n;
- disjoint outer unsorted runs enter quicksort;
- quicksort limits each path to 2*floor(log2(n)) partitions, uses at most two
  linear stable partitions per node, and pivot sampling is bounded by n;
- terminal small sorts have threshold <=32;
- eager fallback has no further nonterminal quicksort, and at most n-1 merges.
The comparison terms cover both outer and fallback run/merge work; m covers
handle moves, scans and scratch copying. In particular, the two n-squared
run/merge allowances for the outer sort and two for fallback produce 4*n*n;
partition depth contributes 8*n*q; terminal small sorts contribute at most
64*n. The small-sort threshold is <=32, independent of record payload size. No sort is rerun to estimate its cost.

Source: installed library/core/src/slice/sort/stable/mod.rs:29,94;
stable/drift.rs:20,191,226,262; stable/quicksort.rs:16,89;
stable/merge.rs:8; shared/pivot.rs:13,55;
shared/smallsort.rs:174,220,536,574.

For a BTree get/contains/index/entry/insert/remove at size <=n:
      Op(n,K) = 64*(n+1) + (n+1)*C_K.
This allows a whole-tree-sized comparison path and conservative node/slot
movement, rather than claiming O(1). The inspected node capacity is 11 (B=6).

For collect::<BTreeMap/BTreeSet>:
      Build(n,K) = Sort(n,K) + sum_{j=0..n-1} Op(j,K).
Add key construction/clones separately. This covers the actual stable sort,
deduplication and sorted bulk build conservatively.

IMPORTANT: the actual FromIterator implementation sorts first. It must not be
charged merely as n insertions or as n visits.

For tree iteration, charge 64*(n+1) structural visits plus body costs.
For BTree clone, charge node/entry visits and deep key/value clones; cloning
String keys adds T, cloning &str keys does not.

Source: installed library/alloc/collections/btree/map.rs:2539;
set.rs:1470; search.rs:49,195; node.rs:43.
The bounds are tied to the reviewed implementation/profile, not an assumed
guarantee about all future Rust implementations.

D. DEFAULT AND CHECK NORMALIZATION: EXPLICIT REPEATED-PASS BOUNDS

NORM(x): dialect.rs:53,79.
With n,r as above, reserve:
      W_NORM = 512 + 64*(n+1)*(r+1);
      T_NORM = 64 + 8*(n+1)*(r+1);
      output length <= max(n,32).

The r+1 term covers repeated chars-vector creation, scanning and trimming/
copying; the linear tail covers case conversion and fixed keyword comparisons.
Raw parentheses in quoted text only overestimate r. No claim of linear cost.

TOK(x): migrations.rs:336.
Reserve:
      W_TOK = 256 + 64*(n+1)*(r+1);
      T_TOK = 64*(n+1);
      token count <= n+1;
      aggregate token text <= 64*(n+1).
A later token join has length <=65*(n+1) and is charged again.

This covers quoted values/identifiers, byte-to-character expansion, numeric/
operator tokens, repeated outer-token scans and remove(0) element shifts.
Header shifts are W; they do not clone String payloads.

check_constraints_equivalent (migrations.rs:485) charges TOK for every expression
on both sides, both token-vector sorts, full comparison, and the raw fallback
comparison. Hashing invokes TOK separately and adds token joins and a separate
sort of canonical constraint strings. These calls are not shared or cached away.

column_changed_for_backend (migrations.rs:776) charges:
- both initial full ColumnModel clones, even when comparison exits early;
- names/types/flags/spatial comparisons;
- defaults_equivalent: NORM on both present defaults and output comparison;
- the PostgreSQL TEXT fallback: postgres_text_default_literal (line 53) on both
  present defaults, EACH including another NORM plus its literal decode/copy;
- the final decoded-literal comparison.

Thus reserve up to TWO NORM invocations per present default per column-change
call. Other phases' normalizers remain additional calls.

postgres_alter_metadata_supported (line 749) charges type/alias scans and flags,
without inventing default normalization there.

E. VALIDATION, EFFECTS AND SCOPING

Before these helpers, inventory and reserve their full bounds.

Target check_limits: runtime_migration.rs:557.
Charge field/index/FK/key/default visits, borrowed views and D(target) Debug
emission to the existing bounded writer. A counter-only writer still consumes
work/generated-text allowance. Preserve the original target check once; expose
its counted definition length through a PRIVATE factoring if needed so local
profile checking does not require a hidden second Debug traversal.

Observed source bounds: use the same definition-counting mechanism once, plus
the structural/local profile checks. Never call target physical validation on
source. Preserve all embedded limits.

Target physical validation: owned_schema.rs:459.
Charge:
- one Build(target table-name map);
- for every index member: member-set insertion plus a full column-name search;
- for every FK: semantic-key clone/insertion, target-table lookup, two member
  sets, full source/target-column searches per ordered pair and type comparison;
- all primary-key, single-column uniqueness, composite-unique and unique-index
  alternatives, including complete nested key comparisons/temporary ref vectors;
- one maximum possible error-format frame.
Do not hide these searches inside a single “FK visit” charge.

Owned effects: runtime_migration.rs:1168.
Charge target ownership lookups; every source-to-target name-search candidate;
the full source-column x target-column name search and metadata checks for each
possible matched table; primary-key comparisons; and all source ownership/FK
visits. For EACH possible incoming unmanaged FK, charge BOTH full source/target
table searches plus a complete possible referenced PK/column comparison,
including column_changed_for_backend for each zipped column. Repeated incoming
references repeat those costs.

Scoping: runtime_migration.rs:1144.
Charge every source ownership lookup, the optional full target-name search,
extension clone and every potentially selected table clone. Using the full
source as the clone bound is safe; retain all full-source metadata separately.
Do not infer ownership or actually choose/filter tables in the accountant.

F. DIFF: MAPS, ORDERING AND PAYLOAD CLONES

Anchor: migrations.rs:890.

Reserve before diff_schema_views. Use counts/maxima, not speculative steps.

- Build source and target table maps, including name-key clones and the hidden
  FromIterator sorts.
- Build lowercase current-extension set; lowercase each target extension and
  charge its lookup and possible EnableExtension payload clone.
- Charge order_tables_by_foreign_keys separately for source AND target.

DFS ordering (migrations.rs:825), for t tables and f FK edges:
- at most t+f visit invocations;
- up to three entry lookups per visit;
- one contains_key per edge;
- one visiting insertion/removal and visited insertion per first table visit;
- two table-name clones per expanded table and t ordered-handle pushes.
Charge all nested FK visits, including calls that immediately find visited/
visiting state. There is no assumed free dependency/topological order.

There are at most min(source-table-count,target-table-count) common table
iterations. Bound their body by that count times the maximum table-pair descriptor
cost (or a tighter sum derived only from size descriptors). Do NOT instantiate
all pairs or decide which steps exist.

For each possible common table iteration:
- Column maps: both Builds and name clones; target-column lookups; up to two
  target-map lookups per source column; every possible column-change call;
  possible add/drop/alter payload clones.
- Ordinary indexes: both Builds/name clones; target lookups and full IndexRef
  comparison including predicate values and direction loop; source lookups;
  possible before/after/drop payload copies.
- Search indexes: both Builds/name clones; full derived equality including every
  field/alias/policy/path/relation/member; possible full search payload clones.
- FKs: both grouping folds, semantic-key clones and entry operations; Build of
  the union of keys (another hidden sort); two group lookups per union key;
  reference-vector copies; constraint-name sorts for BOTH sides of each group;
  complete possible dropped/added FK payloads.
  Sum group sort bounds conservatively using each side's full FK count.
- Append-only/retention comparisons and possible step.
- Both check-tokenization/sort passes and possible full before/after constraint
  payload clone.

Also reserve possible CreateTable clones for each target table, and removed-table
DropSearchIndex/DropTable name clones for each source table. Summing mutually
exclusive payload possibilities is conservative; account all nested strings.

A useful count-only step envelope for a common pair A/B is:
      columns_B + 2*columns_A
    + indexes_A + 2*indexes_B
    + searches_A + searches_B
    + FKs_A + FKs_B + 2.
Add target extension/table creates and source table/search drops.
Use it to bound step visits/allocations/payload copies before diff. Original
actual step ceilings still apply; conservative admission may refuse a valid
small-result plan when its safe envelope does not fit.

G. RENDERING: PRIVATE PHASE BOUNDARY AND COMPLETE FRAGMENTS

Factor the EXISTING private build_migration_plan_views body at migrations.rs:2378
into its existing diff phase and an unchanged render-from-steps phase. Existing
entry points keep their wrapper/order and receive no accountant. Capture/replay
reserve diff first, then reserve rendering from ACTUAL typed diff steps before
calling the factored renderer. This is private factoring, not another planner.

Every renderer estimate is composition of the literal/format/join/clone rules,
including every temporary, rather than just the final SQL length.

Required helper schedule:

- dialect.rs:515 quote_identifier:
  output <=2n+2; replacement temporary <=2n; total T <=4n+2.
- dialect.rs:542 quote_identifier_path:
  reserve split/filter visits, EVERY quoted component and the final dot join.
  Output <=5n+2 and total T <=11n+4 are safe envelopes.
- migrations.rs:1238 sql_string_literal:
  replacement plus outer quotes: output <=2n+2, total T <=4n+2.
- migrations.rs:503 render_default_clause:
  PG raw copy; SQLite NORM, another uppercase copy, numeric/string scans and a
  possible separate parentheses frame.
- migrations.rs:532 column definition:
  initial name/type frame, each constant clause, default helper AND DEFAULT
  frame, then parts join. Do not charge only the joined result.
- migrations.rs:560 create table:
  each column helper; PK and every composite-unique join/frame; every FK name,
  quoted name, source/target-key join and constraint frame; every check's quoted
  identifier/expression frame; parts join AND enclosing CREATE frame.
- migrations.rs:1136 FK naming:
  existing-name clone or both underscore joins/full generated-name frame;
  possible full-name FNV, hex string and shortened frame; then caller quoting.
  A shortened returned name does not erase the unshortened intermediate cost.
- migrations.rs:604,627 append-only:
  sanitize/name/full-name/hash path; table-literal replacement computed before
  dispatch; EACH selected SQL template with all repeated placeholder occurrences.
- migrations.rs:698 constraint_comment:
  for expression n, per-byte hex strings 2n, collected hex buffer 2n, tagged
  buffer 2n+12: total T=6n+12; output=2n+12. Its SQL comment frame is additional.
- migrations.rs:720 constraint comments:
  repeat the preceding helper and COMMENT frame for every check.
- migrations.rs:1188 ordinary index:
  quote each column, optional direction frame, column join; every predicate
  SQL literal, value join and predicate frame; quoted names/path; final CREATE.
- search.rs:120,125,130,143 search names:
  each sanitize pass and prefixed name is a distinct allocation/copy.
- migrations.rs:1560,1564,1570 search config:
  every quote replacement/JSON string, optional alias/policy frame, relation
  field frame/join, field/path/relation record, each of the three array joins,
  and final config object. Nested policy/alias/path/field strings all count.
  A safe final config envelope is J=256*N(index)+6*B(index).
  At most eight nested materialization layers on a leaf path give T_config<=8J;
  derive W from the explicit scans/format/join rules for those same layers.
- migrations.rs:1651 search metadata:
  metadata CREATE, full config helper, all SQL literal temporaries (including
  escaping the COMPLETE config), then INSERT/upsert. JSON escaping followed by
  SQL escaping is charged twice, with its multiplicative length bound.
- migrations.rs:1690 and 1738 create/drop search:
  all THREE search-table-name helpers are called before backend dispatch, even
  when some generated names are unused. Charge those plus all selected SQL
  frames; create includes metadata-upsert, drop includes DELETE literal/frame.
- migrations.rs:1984 step dispatch:
  charge the preceding children per actual variant.
  AlterSearchIndex = drop(before)+create(after).
  PG AlterColumn additionally clones before into default_target, clones/replaces
  its default, calls column_changed_for_backend AGAIN, and emits its possible
  DROP DEFAULT/TYPE/nullability/SET DEFAULT frames.
  AddForeignKey computes both key joins and quoted name before dispatch.
  SetCheckConstraints has separate before-drop, after-add and after-comment
  passes; comments regenerate their hex temporaries.

SQLite rebuild, migrations.rs:1932:
- temporary table-name frame;
- create-table-for-temporary-name with all its children;
- full target-column x source-column name search;
- every possible common-column name clone;
- common-column join;
- INSERT frame containing that column join TWICE;
- DROP/RENAME frames;
- every target ordinary-index renderer;
- append-only renderer when applicable.
Do not invent search-index recreation or other repair work not in this helper.

Root SQLite renderer, migrations.rs:2378:
- source/target lookup-map Builds, rebuilt-table sets and their operations;
- up to four StepRef conversions per step across filter/map/table-name/render
  passes (including nested borrowed index/predicate vectors);
- prepay each standalone step renderer, PLUS a full rebuild envelope for EACH
  rebuild-requiring step, using the maximum possible source/target table-size
  descriptors; duplicate-table rebuild estimates are deliberately not discounted;
- both PRAGMA strings.
Actual engine deduplication may do less work; it never widens the prepaid bound.
Non-SQLite charges its actual step traversal/render children.

After rendering, apply original actual plan-count/SQL limits. T never refunds
the fragments that were discarded or copied into the returned statements.

H. CLASSIFICATION, SCHEMA/PLAN HASHES AND RESULT RETENTION

Classification: migrations.rs:2275.
One StepRef conversion/match per step; include AlterColumn type-string equality.
The pinned reason literal is <=128 bytes. Charge its owned String copy per step.

Schema hash: owned_schema.rs:600.
For EACH invocation:
- borrowed views and full member traversal;
- root extension/table sorts;
- per-table canonical-check TOK/join/fallback copies and constraint-string sort;
- column-name, index-name, search-name and FK-key sorts;
- per-search field, JSON-path (field+path key) and relation sorts;
- NORM for EVERY present column default;
- every primary-key/index-column/predicate-value/relation-field join;
- numeric text and every append into the final canonical buffer;
- full FNV byte scan and final hex result.

Let X be that schema and e each check expression:
      H(X) = 256*N(X) + 2*B(X)
           + 65*sum_e (bytes(e)+1) + 32*present_default_count.
H bounds the final schema canonical buffer only.
Add all NORM/TOK temporaries, check joins/fallbacks, other joins, numeric strings
and the final 16-hex String separately. Do not present H alone as total T.
All sort comparators use actual key descriptors or safe recursive maxima.

Semantic plan hash: schema_manager.rs:886.
For step s, let D_s be its StepRef Debug envelope and a_s its reason bytes:
      line_s <= 256 + D_s + a_s.
Let
      P = backend_bytes + 1
        + sum_s line_s
        + sum_sql (sql_bytes + 1).
Reserve every StepRef construction, Debug traversal, risk/reason formatting,
line temporary, canonical append, full FNV scan and hex result.
Its text is bounded by
      T_semantic = sum_s line_s + P + 16.
Debug is written into the line temporary, not into an invented third standalone
Debug String; the line's subsequent copy into P is nevertheless charged again.

Owned wrapper: runtime_migration.rs:670.
The full semantic helper is a child call, not a free 16-byte argument.
Charge ownership iteration/ref collection/comma join J_o, wrapper formatting,
full FNV scan and final hex result.
The wrapper buffer length is
      L_wrapper = 72 + version_bytes + description_bytes + J_o.
Its additional text is J_o + L_wrapper + 16.
No JSON substitutes for these original bytes.

Additional naming hashes in rendering remain separate invocations. Source and
target hashes are computed once each in the coordinator and reused as values;
do not hash them again merely to construct the integrity wrapper.

Capture retention charges, BEFORE the copies:
- original plan target clone and ownership clone;
- classified reason strings;
- additional witness target/ownership/version/description/hash copies;
- complete expected-step and SQL copies into the independent witness;
- any explicit clone of source, if an implementation chooses one.
Prefer moving the actual original current into the witness; that move has no T.
The original scoped-source clone remains charged independently.

Verification exact comparison uses borrowed private-field traversal, including
raw index directions; no serialization or speculative cloned wire tree solely
for comparison. Returning verified facts moves reconstructed steps/SQL and
charges any new identity/ownership string copies.

I. PUBLIC-OPERATION MULTIPLICITY AND ADMISSION POINTS

Capture:
1. Validate consent/limit settings and inventory/admit target before original
   target check_limits/physical validation. Original target checks run once.
2. Preserve original version/description conversion and bounds, and original
   live dependency validation/introspection order.
3. Immediately after the actual introspected current is returned, inventory/
   admit source definition/profile checks, model effects and scoped cloning.
4. Reserve diff before diff allocation; reserve render from actual diff steps
   before rendering; classify; reserve full source hash, target hash and owned
   integrity before invoking them.
5. Reserve all witness copies before retention/publication.
6. Preserve original live plan validation before returning a successful pair.
   Do not add introspection, automatic apply, encoding or an automatic replay.

Verify:
one source/target profile check each; target physical validation once; model
effects once; scope once; diff once; render once; classification once; source
hash once; target hash once; semantic+owned wrapper once; exact comparison once;
and passive-result retention. Each has its own pre-admission, with a shared
cumulative ledger. No live backend methods.

Bounded from_json/to_json:
their typed visitor/encoder also uses the primitive visit/compare/byte/copy
rules, in addition to existing 64 MiB/depth/node/string limits.
The inventory records byte counts for quotes, backslashes and controls. Exact
canonical JSON string length is
      2+n+quotes+backslashes+short_controls+5*other_controls.
Here short_controls are backspace/form-feed/newline/carriage-return/tab; the
other controls are the remaining U+0000-U+001F bytes. Combine those lengths,
fixed member/tag spellings, separators and integer decimal digit lengths to
compute canonical wire size without producing bytes or deciding a plan.
Before decoding allocation, reserve a conservative base of
32*encoded_bytes+256 work, then 64 work per admitted value/member (including
32 for strict re-entry after tagged buffering, as detailed above), and at most
2*encoded_bytes+256 text for decode scratch, owned strings and a fixed error
frame. Member matching is against the closed
<=15-field shapes; the base includes full key scans/comparisons. New codec
errors use fixed messages rather than interpolating raw unexpected values.
Any engine-based target/profile validation is separately prepaid above.
For encoding, charge the closed field/variant traversal, JSON escaping and
bounded writer copies with the F/join rules; do not build a Value tree first.
All new codec accounting is prospective implementation structure, not a claim
that an unmodified generic serde call already enforces these counters.

Before publishing a witness, also establish that its complete verify and
canonical encode/decode schedules fit the SAME effective profile, using exact
canonical wire-size and structural descriptors only, not extra engine passes. Evaluating those admission
certificates itself consumes the current operation's inventory work; future
operations have fresh ledgers. A narrower caller profile may subsequently
refuse. This avoids successful capture producing an artifact that cannot
round-trip under its own profile.

Repeated public operations receive new ledgers. Repeated passes WITHIN an
operation always add charges, including from_json validation followed by its
own checks; a later verify separately pays for its own work. There is no
process-wide amortization or “already hashed” exemption.

Conservative upper bounds can refuse artifacts well below individual wire,
schema or plan maxima. The maxima are intersecting ceilings, not guaranteed
accepted sizes. No valid old plan or metadata is truncated to make it fit.

J. SCOPE, MAINTENANCE AND REVIEW REQUEST

The W/T promise covers the new accountant, codec/retention work and pure model
validation/planning/rendering/hashing done by the capture/replay path.

It does NOT bound the existing database introspection or live backend checks:
RuntimeMigrationBackend::validate_owned_dependencies, introspect_owned and
validate_owned_plan (runtime_migration.rs:801,866,910), including additional
catalog observations/parsers those methods already perform. Those retain their
original costs/order. Existing caller-supplied Into<String> implementations,
allocator/runtime machinery and host operations are also outside the bound;
their resulting owned strings are checked before subsequent engine work.

Therefore this is not a total wall-clock/CPU/peak-memory/network or whole-live-
capture limit. No process memory, latency or new transactional snapshot
guarantee is made.

Existing planning entry points use their unaccounted original private wrapper:
no inventory, counters, new ceilings, captured clones or codec work, even with
the feature enabled. Private factoring must preserve the exact algorithms,
ordering, output and original allocations/queries; authentic 8e1 fixtures and
normal compatibility evidence remain required.

The helper schedule is a closed whitelist for the named source/toolchain.
Unknown helper/formatting behavior, uncovered repeated call, unchecked
arithmetic or missing conservative descriptor means LimitExceeded BEFORE that
affected computation. No fallback to an unaccounted helper. A source/toolchain
change requires schedule review and recipe-conformance evidence.

All failure/refusal, malformed-input, disclosure, authority, concurrency and
cancellation properties remain inspection-only and runtime-unverified.


```
