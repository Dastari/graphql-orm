//! Explicit capture of sensitive complete schema evidence; pure, passive replay.
//! Each explicitly selected recipe requires its own exact build-qualified compiler
//! and x86_64-unknown-linux-gnu profile; other pairs return LimitExceeded before
//! those operations start. Ordinary planning is unaffected.
use super::*;
use serde::{Deserialize, Serialize};
mod accounting;
mod bounded_decode;
mod encoded_size;
mod wire;
use accounting::*;
use wire::{Decode, Enc, Wire};
type E = OwnedPlanningReplayError;
type R<T> = Result<T, E>;
const RECIPE: &str = "graphql-orm-owned-planning-8e1-v1";
const RUST1981_RECIPE: &str = "graphql-orm-owned-planning-8e1-rust1981-v1";
const FORMAT: u16 = 1;
/// Explicit compiler-scoped planning recipe. There is no implicit selection.
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub enum OwnedPlanningReplayRecipe {
    /// Original 8e1 recipe on exact qualified Rust 1.97.1.
    Historical8e1V1,
    /// Native 8e1 recipe on exact qualified Rust 1.98.1, for new evidence.
    Rust1981V1,
}
impl OwnedPlanningReplayRecipe {
    /// Exact wire identifier; this is distinct from producer identity and hashes.
    pub fn identifier(self) -> &'static str {
        match self {
            Self::Historical8e1V1 => RECIPE,
            Self::Rust1981V1 => RUST1981_RECIPE,
        }
    }
}
const MAX_ENCODED: usize = 67_108_864;
// A missing build-time attestation fails closed, before even allocating a ledger.
// No runtime environment/filesystem/process access participates in replay.
fn qualified_profile(recipe: OwnedPlanningReplayRecipe) -> R<()> {
    let admitted = match recipe {
        OwnedPlanningReplayRecipe::Historical8e1V1 => cfg!(graphql_orm_replay_qualified_profile),
        OwnedPlanningReplayRecipe::Rust1981V1 => cfg!(graphql_orm_replay_rust1981_profile),
    };
    if admitted {
        Ok(())
    } else {
        Err(E::LimitExceeded)
    }
}
// Frozen v1 ceilings do not follow a future change to ordinary ORM defaults.
const PINNED_SCHEMA_LIMITS: RuntimeMigrationLimits = RuntimeMigrationLimits {
    max_collections: 512,
    max_fields: 256,
    max_indexes: 128,
    max_relations: 128,
    max_key_members: 32,
    max_default_bytes: 65_536,
    max_target_bytes: 8_388_608,
    max_plan_steps: 16_384,
    max_plan_statements: 32_768,
    max_plan_bytes: 8_388_608,
};

/// Separate fixed replay errors. Diagnostics never contain retained metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OwnedPlanningReplayError {
    /// Unknown wire format.
    UnsupportedFormat,
    /// Unknown integrity/planning recipe.
    UnsupportedRecipe,
    /// Capture/replay supports only SQLite and PostgreSQL.
    UnsupportedBackend,
    /// A resource ceiling, qualified build profile or admission bound is unmet.
    LimitExceeded,
    /// The complete typed witness is invalid.
    InvalidWitness,
    /// Reconstructed complete facts differ from retained facts.
    ReplayMismatch,
}
impl fmt::Display for E {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "owned planning replay: {self:?}")
    }
}
impl std::error::Error for E {}
/// Live planning errors remain available to trusted callers. Outer formatting
/// deliberately omits their potentially sensitive underlying diagnostics.
#[non_exhaustive]
pub enum OwnedPlanningCaptureError {
    /// Original live planning failure.
    Planning(RuntimeMigrationError),
    /// Opt-in witness failure.
    Witness(OwnedPlanningReplayError),
}
impl fmt::Debug for OwnedPlanningCaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Planning(_) => "OwnedPlanningCaptureError::Planning",
            Self::Witness(_) => "OwnedPlanningCaptureError::Witness",
        })
    }
}
impl fmt::Display for OwnedPlanningCaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}
impl std::error::Error for OwnedPlanningCaptureError {}
impl From<E> for OwnedPlanningCaptureError {
    fn from(e: E) -> Self {
        Self::Witness(e)
    }
}
impl From<RuntimeMigrationError> for OwnedPlanningCaptureError {
    fn from(e: RuntimeMigrationError) -> Self {
        Self::Planning(e)
    }
}
impl From<RuntimeMigrationDiagnostics> for OwnedPlanningCaptureError {
    fn from(e: RuntimeMigrationDiagnostics) -> Self {
        Self::Planning(e.into())
    }
}

/// Narrow-only checked profile. Work/text/depth/node ceilings are fixed.
#[derive(Clone, Copy, Debug)]
pub struct OwnedPlanningReplayLimits {
    max_encoded_bytes: usize,
    schema: RuntimeMigrationLimits,
}
impl Default for OwnedPlanningReplayLimits {
    fn default() -> Self {
        Self {
            max_encoded_bytes: MAX_ENCODED,
            schema: PINNED_SCHEMA_LIMITS,
        }
    }
}
impl OwnedPlanningReplayLimits {
    /// Construct a positive profile no wider than the pinned hard ceilings.
    pub fn new(
        max_encoded_bytes: usize,
        schema: RuntimeMigrationLimits,
    ) -> Result<Self, OwnedPlanningReplayError> {
        if max_encoded_bytes == 0 || max_encoded_bytes > MAX_ENCODED {
            return Err(E::LimitExceeded);
        }
        check_limit_values(schema, PINNED_SCHEMA_LIMITS)?;
        Ok(Self {
            max_encoded_bytes,
            schema,
        })
    }
    /// Maximum accepted/emitted JSON bytes.
    pub fn max_encoded_bytes(&self) -> usize {
        self.max_encoded_bytes
    }
    /// Maximum complete model/plan sizes.
    pub fn schema(&self) -> RuntimeMigrationLimits {
        self.schema
    }
    fn intersect(self, b: Self) -> Self {
        let mut a = limit_values(self.schema);
        let bs = limit_values(b.schema);
        for (a, b) in a.iter_mut().zip(bs) {
            *a = (*a).min(b);
        }
        Self {
            max_encoded_bytes: self.max_encoded_bytes.min(b.max_encoded_bytes),
            schema: limits_from(a),
        }
    }
}
/// Explicit acknowledgement of sensitive schema literals and private custody.
/// This token grants no migration or application authority.
#[derive(Debug)]
pub struct OwnedPlanningReplayCaptureConsent {
    _private: (),
}
impl OwnedPlanningReplayCaptureConsent {
    /// Accept complete sensitive metadata retention and take responsibility for
    /// private custody. Capture cannot redact while promising exact replay.
    pub fn accept_sensitive_schema_metadata_and_private_custody() -> Self {
        Self { _private: () }
    }
}
/// Opaque complete evidence. Only explicit encoding discloses its schema/SQL.
pub struct OwnedPlanningReplayWitness {
    data: Data,
    limits: OwnedPlanningReplayLimits,
}
impl fmt::Debug for OwnedPlanningReplayWitness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OwnedPlanningReplayWitness { .. }")
    }
}
/// Passive verified facts, without an apply-capable plan or conversion to one.
pub struct VerifiedOwnedPlanningReplay {
    data: Facts,
    recipe: OwnedPlanningReplayRecipe,
    backend: DatabaseBackend,
    version: String,
    description: String,
    ownership: ManagedTableSet,
    options: PlanOptions,
}
impl fmt::Debug for VerifiedOwnedPlanningReplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VerifiedOwnedPlanningReplay { .. }")
    }
}
struct Facts {
    source_schema_hash: String,
    target_schema_hash: String,
    plan_hash: String,
    steps: Vec<OwnedPlannedMigrationStep>,
    statements: Vec<String>,
}
struct Data {
    recipe: OwnedPlanningReplayRecipe,
    backend: DatabaseBackend,
    version: String,
    description: String,
    ownership: ManagedTableSet,
    options: PlanOptions,
    source: OwnedSchemaModel,
    target: OwnedSchemaModel,
    expected: Facts,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Input {
    format_version: u16,
    recipe: String,
    backend: String,
    version: String,
    description: String,
    ownership: Vec<String>,
    options: wire::Options,
    source: wire::Schema,
    target: wire::Schema,
    expected: Expected,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Expected {
    source_schema_hash: String,
    target_schema_hash: String,
    plan_hash: String,
    steps: Vec<wire::Classified>,
    statements: Vec<String>,
}
impl Wire for Facts {
    fn emit<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut o = s.serialize_struct("Expected", 5)?;
        o.serialize_field("sourceSchemaHash", &self.source_schema_hash)?;
        o.serialize_field("targetSchemaHash", &self.target_schema_hash)?;
        o.serialize_field("planHash", &self.plan_hash)?;
        o.serialize_field("steps", &Enc(&self.steps))?;
        o.serialize_field("statements", &self.statements)?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        for (key, value) in [
            ("sourceSchemaHash", &self.source_schema_hash),
            ("targetSchemaHash", &self.target_schema_hash),
            ("planHash", &self.plan_hash),
        ] {
            i.member(key)?;
            value.inventory(i)?;
        }
        i.member("steps")?;
        self.steps.inventory(i)?;
        i.member("statements")?;
        self.statements.inventory(i)
    }
}
impl Wire for Data {
    fn emit<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeSeq, SerializeStruct};
        struct Owners<'a>(&'a ManagedTableSet);
        impl Serialize for Owners<'_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let mut q = s.serialize_seq(None)?;
                for x in self.0.tables() {
                    q.serialize_element(x)?;
                }
                q.end()
            }
        }
        let mut o = s.serialize_struct("Witness", 10)?;
        o.serialize_field("formatVersion", &FORMAT)?;
        o.serialize_field("recipe", self.recipe.identifier())?;
        o.serialize_field("backend", self.backend.name())?;
        o.serialize_field("version", &self.version)?;
        o.serialize_field("description", &self.description)?;
        o.serialize_field("ownership", &Owners(&self.ownership))?;
        o.serialize_field("options", &Enc(&self.options))?;
        o.serialize_field("source", &Enc(&self.source))?;
        o.serialize_field("target", &Enc(&self.target))?;
        o.serialize_field("expected", &Enc(&self.expected))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        for k in [
            "formatVersion",
            "recipe",
            "backend",
            "version",
            "description",
            "ownership",
            "options",
            "source",
            "target",
            "expected",
        ] {
            i.member(k)?;
        }
        i.string(self.recipe.identifier())?;
        i.string(self.backend.name())?;
        self.version.inventory(i)?;
        self.description.inventory(i)?;
        i.ownership(&self.ownership)?;
        self.options.inventory(i)?;
        self.source.inventory(i)?;
        self.target.inventory(i)?;
        self.expected.inventory(i)
    }
}
fn limit_values(x: RuntimeMigrationLimits) -> [usize; 10] {
    [
        x.max_collections,
        x.max_fields,
        x.max_indexes,
        x.max_relations,
        x.max_key_members,
        x.max_default_bytes,
        x.max_target_bytes,
        x.max_plan_steps,
        x.max_plan_statements,
        x.max_plan_bytes,
    ]
}
fn limits_from(x: [usize; 10]) -> RuntimeMigrationLimits {
    RuntimeMigrationLimits {
        max_collections: x[0],
        max_fields: x[1],
        max_indexes: x[2],
        max_relations: x[3],
        max_key_members: x[4],
        max_default_bytes: x[5],
        max_target_bytes: x[6],
        max_plan_steps: x[7],
        max_plan_statements: x[8],
        max_plan_bytes: x[9],
    }
}
fn check_limit_values(x: RuntimeMigrationLimits, ceiling: RuntimeMigrationLimits) -> R<()> {
    if limit_values(x)
        .into_iter()
        .zip(limit_values(ceiling))
        .any(|(x, m)| x == 0 || x > m)
    {
        return Err(E::LimitExceeded);
    }
    Ok(())
}
fn backend(b: DatabaseBackend) -> R<()> {
    match b {
        DatabaseBackend::Sqlite | DatabaseBackend::Postgres => Ok(()),
        _ => Err(E::UnsupportedBackend),
    }
}
fn identity(v: &str, d: &str) -> R<()> {
    if v.len() > 256 || d.len() > 4096 || v.trim().is_empty() {
        return Err(E::InvalidWitness);
    }
    Ok(())
}
fn plan_limits(steps: usize, sql: &[String], lim: RuntimeMigrationLimits) -> R<()> {
    let n = sql
        .iter()
        .try_fold(0usize, |a, s| a.checked_add(s.len()))
        .ok_or(E::LimitExceeded)?;
    if steps > lim.max_plan_steps || sql.len() > lim.max_plan_statements || n > lim.max_plan_bytes {
        Err(E::LimitExceeded)
    } else {
        Ok(())
    }
}
fn hashes_valid(f: &Facts) -> bool {
    [&f.source_schema_hash, &f.target_schema_hash, &f.plan_hash]
        .into_iter()
        .all(|s| {
            s.len() == 16
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}
fn profile_check_once(
    s: &OwnedSchemaModel,
    limits: OwnedPlanningReplayLimits,
    l: &mut Ledger,
) -> R<Size> {
    let z = size(s, l, limits.max_encoded_bytes)?;
    l.reserve(profile(z, l.recipe)?)?;
    check_limit_values(s.limits, PINNED_SCHEMA_LIMITS)?;
    // The structural/Debug check runs once with the intersection, preserving every
    // original embedded ceiling while permitting only a narrower witness profile.
    let profile = OwnedPlanningReplayLimits {
        max_encoded_bytes: limits.max_encoded_bytes,
        schema: s.limits,
    }
    .intersect(limits);
    s.check_limits_size(profile.schema)
        .map_err(|_| E::LimitExceeded)?;
    Ok(z)
}
fn compute(data: &Data, limits: OwnedPlanningReplayLimits, l: &mut Ledger) -> R<Facts> {
    backend(data.backend)?;
    identity(&data.version, &data.description)?;
    let a = profile_check_once(&data.source, limits, l)?;
    let b = profile_check_once(&data.target, limits, l)?;
    let c = accounting::target(&data.target, b, l, limits.max_encoded_bytes)?;
    l.reserve(c)?;
    data.target
        .validate_physical_contract()
        .map_err(|_| E::InvalidWitness)?;
    compute_after_target(
        data.backend,
        &data.version,
        &data.description,
        &data.source,
        &data.target,
        &data.ownership,
        data.options,
        limits,
        a,
        b,
        Some(&data.expected),
        l,
    )
    .map_err(|e| match e {
        OwnedPlanningCaptureError::Witness(e) => e,
        OwnedPlanningCaptureError::Planning(_) => E::InvalidWitness,
    })
}
#[allow(clippy::too_many_arguments)]
fn compute_after_target(
    backend: DatabaseBackend,
    version: &str,
    description: &str,
    current: &OwnedSchemaModel,
    target: &OwnedSchemaModel,
    ownership: &ManagedTableSet,
    options: PlanOptions,
    limits: OwnedPlanningReplayLimits,
    a: Size,
    b: Size,
    expected: Option<&Facts>,
    l: &mut Ledger,
) -> Result<Facts, OwnedPlanningCaptureError> {
    let cap = limits.max_encoded_bytes;
    let owners = ownership.0.len() as u64;
    let c = effects(current, target, a, b, owners, l, cap)?;
    l.reserve(c)?;
    validate_owned_effects(backend, current, target, ownership)?;
    let scoped = scoped_current(current, ownership, options, target);
    let c = diff(current, target, a, b, l, cap)?;
    l.reserve(c)?;
    let empty = Vec::new();
    let mut wanted = target.borrowed();
    if backend != DatabaseBackend::Postgres {
        wanted.extensions = &empty;
    }
    let steps = super::super::migrations::diff_schema_views(backend, &scoped.borrowed(), &wanted);
    // A decoded expected plan is untrusted. Before expansion, require the
    // actual typed diff to match it exactly. Length-first closed comparisons
    // visit at most the expected structure/payload, even on a mismatch. This
    // makes future admission based on retained steps sound without an extra
    // engine pass or speculative plan in the size calculator.
    if let Some(expected) = expected {
        let z = size(&expected.steps, l, cap)?;
        l.reserve(Cost::new(mul(4, z.m()?)?, 0))?;
        if steps.len() != expected.steps.len()
            || !steps
                .iter()
                .zip(&expected.steps)
                .all(|(a, b)| exact_step(a, &b.step))
        {
            return Err(E::ReplayMismatch.into());
        }
    }
    let c = render(&steps, &scoped, target, l, cap)?;
    l.reserve(c)?;
    let statements = super::super::migrations::render_owned_plan_steps(
        backend,
        &scoped.borrowed(),
        &wanted,
        &steps,
    );
    if let Some(expected) = expected {
        let z = size(&expected.statements, l, cap)?;
        l.reserve(Cost::new(mul(4, z.m()?)?, 0))?;
        if statements != expected.statements {
            return Err(E::ReplayMismatch.into());
        }
    }
    plan_limits(steps.len(), &statements, target.limits)?;
    plan_limits(steps.len(), &statements, limits.schema)?;
    let mut classification =
        Cost::new(mul(256, steps.len() as u64)?, mul(128, steps.len() as u64)?);
    for step in &steps {
        let z = size(step, l, cap)?;
        classification = classification.add(Cost::new(add(mul(64, z.n)?, mul(2, z.b)?)?, 0))?;
    }
    l.reserve(classification)?;
    let steps = steps
        .into_iter()
        .map(|step| {
            let (risk, reason) = classify_migration_step_view(&StepRef::from(&step));
            OwnedPlannedMigrationStep {
                step,
                risk,
                reason: reason.into(),
            }
        })
        .collect::<Vec<_>>();
    if let Some(expected) = expected {
        let z = size(&expected.steps, l, cap)?;
        l.reserve(Cost::new(mul(4, z.m()?)?, 0))?;
        if !steps
            .iter()
            .zip(&expected.steps)
            .all(|(a, b)| a.risk == b.risk && a.reason == b.reason)
        {
            return Err(E::ReplayMismatch.into());
        }
    }
    let c = schema_hash(current, a, l, cap)?;
    l.reserve(c)?;
    let source_schema_hash = current.stable_hash();
    let c = schema_hash(target, b, l, cap)?;
    l.reserve(c)?;
    let target_schema_hash = target.stable_hash();
    let id = Size {
        n: owners + 4,
        b: mul(add(MAX_PHYSICAL_IDENTIFIER_LEN as u64, 1)?, owners)?
            + version.len() as u64
            + description.len() as u64,
        longest: 0,
        non_ascii_bytes: 0,
    };
    let c = integrity(&steps, &statements, id, l, cap)?;
    l.reserve(c)?;
    let plan_hash = owned_integrity_hash(
        backend.name(),
        version,
        description,
        &source_schema_hash,
        &target_schema_hash,
        ownership,
        &steps,
        &statements,
    );
    Ok(Facts {
        source_schema_hash,
        target_schema_hash,
        plan_hash,
        steps,
        statements,
    })
}
// Exact comparison deliberately bypasses IndexModel's semantic PartialEq.
// Wire emits every raw private field, including empty versus explicit directions.
// Compare borrowed typed fields recursively; no serialization tree or Debug text.
fn exact_facts(a: &Facts, b: &Facts) -> bool {
    a.source_schema_hash == b.source_schema_hash
        && a.target_schema_hash == b.target_schema_hash
        && a.plan_hash == b.plan_hash
        && a.statements == b.statements
        && a.steps.len() == b.steps.len()
        && a.steps
            .iter()
            .zip(&b.steps)
            .all(|(a, b)| a.risk == b.risk && a.reason == b.reason && exact_step(&a.step, &b.step))
}
fn exact_index(a: &IndexModel, b: &IndexModel) -> bool {
    a.name == b.name
        && a.columns == b.columns
        && a.column_directions == b.column_directions
        && a.is_unique == b.is_unique
        && a.method == b.method
        && a.is_spatial == b.is_spatial
        && a.predicate == b.predicate
}
fn exact_table(a: &OwnedTableModel, b: &OwnedTableModel) -> bool {
    a.entity_name == b.entity_name
        && a.table_name == b.table_name
        && a.primary_key == b.primary_key
        && a.primary_keys == b.primary_keys
        && a.default_sort == b.default_sort
        && a.columns == b.columns
        && a.indexes.len() == b.indexes.len()
        && a.indexes
            .iter()
            .zip(&b.indexes)
            .all(|(a, b)| exact_index(a, b))
        && a.composite_unique_indexes == b.composite_unique_indexes
        && a.foreign_keys == b.foreign_keys
        && a.search_indexes == b.search_indexes
        && a.append_only == b.append_only
        && a.retention_purge == b.retention_purge
        && a.check_constraints == b.check_constraints
}
fn exact_step(a: &OwnedMigrationStep, b: &OwnedMigrationStep) -> bool {
    match (a, b) {
        (OwnedMigrationStep::CreateTable(a), OwnedMigrationStep::CreateTable(b)) => {
            exact_table(a, b)
        }
        (
            OwnedMigrationStep::CreateIndex {
                table_name: a,
                index: ai,
            },
            OwnedMigrationStep::CreateIndex {
                table_name: b,
                index: bi,
            },
        ) => a == b && exact_index(ai, bi),
        _ => a == b,
    }
}
fn canonical_size(data: &Data, limits: OwnedPlanningReplayLimits, l: &mut Ledger) -> R<usize> {
    let z = size(data, l, limits.max_encoded_bytes)?;
    let upper = add(mul(256, z.n)?, mul(6, z.b)?)?;
    l.reserve(
        Cost::new(add(mul(32, z.n)?, mul(16, z.b)?)?, mul(2, upper)?)
            .add(ownership_tree(data.ownership.0.len() as u64)?)?,
    )?;
    let mut count = encoded_size::Count {
        n: 0,
        maximum: limits.max_encoded_bytes,
    };
    Enc(data)
        .serialize(encoded_size::Layout { count: &mut count })
        .map_err(|_| E::LimitExceeded)?;
    Ok(count.n)
}
fn codec_admit(n: usize, l: &mut Ledger) -> R<()> {
    l.reserve(Cost::new(
        add(mul(32, n as u64)?, 256)?,
        add(mul(2, n as u64)?, 256)?,
    ))
}
fn certificate(data: &Data, limits: OwnedPlanningReplayLimits, l: &mut Ledger) -> R<()> {
    let calculator_start = l.used;
    let n = canonical_size(data, limits, l)?;
    let mut future = Ledger::for_recipe(data.recipe);
    codec_admit(n, &mut future)?;
    let z = size(data, l, limits.max_encoded_bytes)?;
    // Lexical member/value nodes are at most twice the wire inventory slots.
    // Includes the strict re-entry wrappers after tagged Content buffering.
    future.reserve(Cost::new(mul(128, z.n)?, 0))?;
    // Only admission calculations run here. No extra engine observation or replay.
    let mut verify = Ledger::for_recipe(data.recipe);
    let a = size(&data.source, l, limits.max_encoded_bytes)?;
    let b = size(&data.target, l, limits.max_encoded_bytes)?;
    verify.reserve(profile(a, data.recipe)?.add(profile(b, data.recipe)?)?)?;
    let c = accounting::target(&data.target, b, l, limits.max_encoded_bytes)?;
    verify.reserve(c)?;
    let c = effects(
        &data.source,
        &data.target,
        a,
        b,
        data.ownership.0.len() as u64,
        l,
        limits.max_encoded_bytes,
    )?;
    verify.reserve(c)?;
    let c = diff(
        &data.source,
        &data.target,
        a,
        b,
        l,
        limits.max_encoded_bytes,
    )?;
    verify.reserve(c)?;
    let c = render(&[], &data.source, &data.target, l, limits.max_encoded_bytes)?;
    verify.reserve(c)?;
    // Expected steps have exactly the captured structure; reserve their renderer
    // directly by borrowing, without allocating a speculative Vec of steps.
    verify.reserve(Cost::new(
        mul(256, data.expected.steps.len() as u64)?,
        mul(128, data.expected.steps.len() as u64)?,
    ))?;
    for step in &data.expected.steps {
        let step_size = size(&step.step, l, limits.max_encoded_bytes)?;
        verify.reserve(Cost::new(
            add(mul(64, step_size.n)?, mul(2, step_size.b)?)?,
            0,
        ))?;
        let c = render(
            std::slice::from_ref(&step.step),
            &data.source,
            &data.target,
            l,
            limits.max_encoded_bytes,
        )?;
        verify.reserve(c)?;
    }
    let c = schema_hash(&data.source, a, l, limits.max_encoded_bytes)?;
    verify.reserve(c)?;
    let c = schema_hash(&data.target, b, l, limits.max_encoded_bytes)?;
    verify.reserve(c)?;
    let c = integrity(
        &data.expected.steps,
        &data.expected.statements,
        z,
        l,
        limits.max_encoded_bytes,
    )?;
    verify.reserve(c)?;
    verify.reserve(z.copy()?.add(Cost::new(
        mul(512, z.m()?)?,
        mul(128, data.expected.steps.len() as u64)?,
    ))?)?;
    // Certificate calculation is charged to this operation before its visits.
    // Its full measured conservative reservation also prepays the same bounded
    // calculation on decode, and dominates verify's smaller descriptor walks.
    let calculator = Cost::new(l.used.w - calculator_start.w, l.used.t - calculator_start.t);
    verify.reserve(calculator)?;
    future.reserve(calculator)?;
    // Decode's own profile inventories and target-accountant invocation, plus
    // certificate tail bookkeeping, are additional to the measured certificate.
    // 2048N+32B dominates their <=192N+4B model walk, two <=448N+4B
    // target descriptor walks, conversion/field visits and fixed ledger work.
    future.reserve(Cost::new(add(mul(2048, z.n)?, mul(32, z.b)?)?, 0))?;
    let owners = data.ownership.0.len() as u64;
    let longest = MAX_PHYSICAL_IDENTIFIER_LEN as u64;
    let bytes = mul(longest, owners)?;
    future
        .reserve(ownership_cost(owners, longest, bytes)?.add(Cost::new(mul(128, owners)?, 0))?)?;
    // Decode additionally performs the once-only profile/target checks. This is
    // separate from certificate arithmetic and from a future verification.
    future.reserve(profile(a, data.recipe)?.add(profile(b, data.recipe)?)?)?;
    let c = accounting::target(&data.target, b, l, limits.max_encoded_bytes)?;
    future.reserve(c)?;
    let encode_upper = add(mul(256, z.n)?, mul(6, z.b)?)?;
    let mut encode = Ledger::for_recipe(data.recipe);
    encode.reserve(calculator)?;
    encode.reserve(profile(a, data.recipe)?.add(profile(b, data.recipe)?)?)?;
    encode.reserve(ownership_tree(owners)?)?;
    encode.reserve(Cost::new(
        add(mul(64, z.n)?, mul(32, z.b)?)?,
        mul(2, encode_upper)?,
    ))?;
    Ok(())
}
fn predecode(bytes: &[u8], limits: OwnedPlanningReplayLimits, l: &mut Ledger) -> R<()> {
    if bytes.len() > limits.max_encoded_bytes {
        return Err(E::LimitExceeded);
    }
    codec_admit(bytes.len(), l)?;
    let mut depth = 0usize;
    let mut nodes = 0u64;
    let mut pos = 0;
    while pos < bytes.len() {
        let b = bytes[pos];
        match b {
            b'{' | b'[' => {
                depth = depth.checked_add(1).ok_or(E::LimitExceeded)?;
                if depth > 64 {
                    return Err(E::LimitExceeded);
                }
                nodes = add(nodes, 1)?;
            }
            b'}' | b']' => {
                depth = depth.checked_sub(1).ok_or(E::InvalidWitness)?;
            }
            b'"' => {
                nodes = add(nodes, 1)?;
                let start = pos;
                pos += 1;
                loop {
                    if pos >= bytes.len() {
                        return Err(E::InvalidWitness);
                    }
                    if bytes[pos] == b'"' {
                        break;
                    }
                    if bytes[pos] == b'\\' {
                        pos = pos.checked_add(1).ok_or(E::LimitExceeded)?;
                    }
                    pos += 1;
                    if pos - start > 8_388_608 + 1 {
                        return Err(E::LimitExceeded);
                    }
                }
            }
            b't' | b'f' | b'n' | b'-' | b'0'..=b'9' => {
                nodes = add(nodes, 1)?;
                while pos + 1 < bytes.len()
                    && !matches!(
                        bytes[pos + 1],
                        b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t'
                    )
                {
                    pos += 1;
                }
            }
            _ => {}
        }
        if nodes > 2_000_000 {
            return Err(E::LimitExceeded);
        }
        pos += 1;
    }
    if depth != 0 {
        return Err(E::InvalidWitness);
    }
    l.reserve(Cost::new(mul(64, nodes)?, 0))
}
impl OwnedPlanningReplayWitness {
    /// Decode a complete v1 witness without connecting to a database. All object
    /// members are required, duplicates/unknowns rejected, and order preserved.
    pub fn from_json(
        bytes: &[u8],
        limits: OwnedPlanningReplayLimits,
    ) -> Result<Self, OwnedPlanningReplayError> {
        Self::from_json_for_recipe(bytes, limits, OwnedPlanningReplayRecipe::Historical8e1V1)
    }
    /// Decode only the explicitly expected recipe on its own qualified compiler.
    /// This never relabels evidence or tries a different recipe on failure.
    pub fn from_json_for_recipe(
        bytes: &[u8],
        limits: OwnedPlanningReplayLimits,
        expected_recipe: OwnedPlanningReplayRecipe,
    ) -> Result<Self, OwnedPlanningReplayError> {
        qualified_profile(expected_recipe)?;
        let mut l = Ledger::for_recipe(expected_recipe);
        predecode(bytes, limits, &mut l)?;
        let mut parser = serde_json::Deserializer::from_slice(bytes);
        let input = Input::deserialize(bounded_decode::Fixed(&mut parser))
            .map_err(|_| E::InvalidWitness)?;
        parser.end().map_err(|_| E::InvalidWitness)?;
        if input.format_version != FORMAT {
            return Err(E::UnsupportedFormat);
        }
        if input.recipe != expected_recipe.identifier() {
            return Err(E::UnsupportedRecipe);
        }
        let backend = match input.backend.as_str() {
            "sqlite" => DatabaseBackend::Sqlite,
            "postgres" => DatabaseBackend::Postgres,
            _ => return Err(E::UnsupportedBackend),
        };
        identity(&input.version, &input.description)?;
        if !input.ownership.windows(2).all(|p| p[0] < p[1]) {
            return Err(E::InvalidWitness);
        }
        admit_ownership(&input.ownership, &mut l)?;
        let ownership = ManagedTableSet::new(input.ownership).map_err(|_| E::InvalidWitness)?;
        let expected = Facts {
            source_schema_hash: input.expected.source_schema_hash,
            target_schema_hash: input.expected.target_schema_hash,
            plan_hash: input.expected.plan_hash,
            steps: input.expected.steps.finish(),
            statements: input.expected.statements,
        };
        if !hashes_valid(&expected) {
            return Err(E::InvalidWitness);
        }
        let data = Data {
            recipe: expected_recipe,
            backend,
            version: input.version,
            description: input.description,
            ownership,
            options: input.options.finish(),
            source: input.source.finish(),
            target: input.target.finish(),
            expected,
        };
        profile_check_once(&data.source, limits, &mut l)?;
        let z = profile_check_once(&data.target, limits, &mut l)?;
        let c = accounting::target(&data.target, z, &mut l, limits.max_encoded_bytes)?;
        l.reserve(c)?;
        data.target
            .validate_physical_contract()
            .map_err(|_| E::InvalidWitness)?;
        plan_limits(
            data.expected.steps.len(),
            &data.expected.statements,
            limits.schema,
        )?;
        certificate(&data, limits, &mut l)?;
        Ok(Self { data, limits })
    }
    /// Explicitly disclose complete sensitive evidence in canonical compact JSON.
    /// The effective profile can only become narrower.
    pub fn to_json(
        &self,
        limits: OwnedPlanningReplayLimits,
    ) -> Result<Vec<u8>, OwnedPlanningReplayError> {
        qualified_profile(self.data.recipe)?;
        let limits = self.limits.intersect(limits);
        let mut l = Ledger::for_recipe(self.data.recipe);
        profile_check_once(&self.data.source, limits, &mut l)?;
        profile_check_once(&self.data.target, limits, &mut l)?;
        plan_limits(
            self.data.expected.steps.len(),
            &self.data.expected.statements,
            limits.schema,
        )?;
        let n = canonical_size(&self.data, limits, &mut l)?;
        let z = size(&self.data, &mut l, limits.max_encoded_bytes)?;
        l.reserve(
            Cost::new(add(mul(32, z.n)?, mul(16, z.b)?)?, mul(2, n as u64)?)
                .add(ownership_tree(self.data.ownership.0.len() as u64)?)?,
        )?;
        let mut result = Vec::with_capacity(n);
        serde_json::to_writer(&mut result, &Enc(&self.data)).map_err(|_| E::InvalidWitness)?;
        Ok(result)
    }
    /// Reproduce exact engine facts purely in memory. This grants no authorization,
    /// present baseline, recovery or data-validity assurance.
    pub fn verify(&self) -> Result<VerifiedOwnedPlanningReplay, OwnedPlanningReplayError> {
        qualified_profile(self.data.recipe)?;
        let mut l = Ledger::for_recipe(self.data.recipe);
        let facts = compute(&self.data, self.limits, &mut l)?;
        let z = size(&self.data.expected, &mut l, self.limits.max_encoded_bytes)?;
        let actual = size(&facts, &mut l, self.limits.max_encoded_bytes)?;
        l.reserve(Cost::new(z.cmp(actual)?, 0))?;
        if !exact_facts(&facts, &self.data.expected) {
            return Err(E::ReplayMismatch);
        }
        let mut id = Inventory::new(&mut l, self.limits.max_encoded_bytes);
        id.string(&self.data.version)?;
        id.string(&self.data.description)?;
        id.ownership(&self.data.ownership)?;
        let z = id.size;
        l.reserve(
            z.copy()?
                .add(ownership_tree(self.data.ownership.0.len() as u64)?)?,
        )?;
        Ok(VerifiedOwnedPlanningReplay {
            data: facts,
            recipe: self.data.recipe,
            backend: self.data.backend,
            version: self.data.version.clone(),
            description: self.data.description.clone(),
            ownership: self.data.ownership.clone(),
            options: self.data.options,
        })
    }
}
impl VerifiedOwnedPlanningReplay {
    /// Exact supported wire version.
    pub fn format_version(&self) -> u16 {
        FORMAT
    }
    /// Source-qualified planning/integrity recipe.
    pub fn recipe(&self) -> &str {
        self.recipe.identifier()
    }
    /// Original planning backend.
    pub fn backend(&self) -> DatabaseBackend {
        self.backend
    }
    /// Original host version.
    pub fn version(&self) -> &str {
        &self.version
    }
    /// Original host description.
    pub fn description(&self) -> &str {
        &self.description
    }
    /// Complete observed source hash.
    pub fn source_schema_hash(&self) -> &str {
        &self.data.source_schema_hash
    }
    /// Desired target hash.
    pub fn target_schema_hash(&self) -> &str {
        &self.data.target_schema_hash
    }
    /// Exact owned integrity hash.
    pub fn plan_hash(&self) -> &str {
        &self.data.plan_hash
    }
    /// Explicit original table ownership.
    pub fn ownership(&self) -> &ManagedTableSet {
        &self.ownership
    }
    /// Original scoping options.
    pub fn options(&self) -> PlanOptions {
        self.options
    }
    /// Complete ordered passive steps with risk/reason.
    pub fn steps(&self) -> &[OwnedPlannedMigrationStep] {
        &self.data.steps
    }
    /// Exact ordered passive SQL; no execution capability.
    pub fn statements(&self) -> &[String] {
        &self.data.statements
    }
}
impl<B: RuntimeMigrationBackend> SchemaManager<'_, B> {
    /// Explicitly capture the actual full observation of a successful owned plan.
    /// Consent acknowledges sensitive metadata retention and private custody.
    /// Existing planning entry points perform no replay accounting or retention.
    #[allow(clippy::too_many_arguments)]
    pub async fn plan_owned_migration_with_replay_witness(
        &self,
        version: impl Into<String>,
        description: impl Into<String>,
        target: &OwnedSchemaModel,
        ownership: &ManagedTableSet,
        options: PlanOptions,
        limits: OwnedPlanningReplayLimits,
        _consent: OwnedPlanningReplayCaptureConsent,
    ) -> Result<(OwnedPlannedMigration, OwnedPlanningReplayWitness), OwnedPlanningCaptureError>
    {
        self.plan_owned_migration_with_replay_witness_for_recipe(
            version,
            description,
            target,
            ownership,
            options,
            OwnedPlanningReplayRecipe::Historical8e1V1,
            limits,
            _consent,
        )
        .await
    }
    /// Capture new complete evidence using an explicitly selected native recipe.
    /// Every recipe is admitted only on its own exact compiler/platform profile.
    /// This labels the unchanged ordinary plan; it does not rehash that plan.
    #[allow(clippy::too_many_arguments)]
    pub async fn plan_owned_migration_with_replay_witness_for_recipe(
        &self,
        version: impl Into<String>,
        description: impl Into<String>,
        target: &OwnedSchemaModel,
        ownership: &ManagedTableSet,
        options: PlanOptions,
        recipe: OwnedPlanningReplayRecipe,
        limits: OwnedPlanningReplayLimits,
        _consent: OwnedPlanningReplayCaptureConsent,
    ) -> Result<(OwnedPlannedMigration, OwnedPlanningReplayWitness), OwnedPlanningCaptureError>
    {
        qualified_profile(recipe)?;
        backend(B::DIALECT)?;
        supported::<B>()?;
        if !self.policy().allows_planning() {
            return Err(rejection(RuntimeMigrationDiagnosticCode::PolicyDenied).into());
        }
        let mut l = Ledger::for_recipe(recipe);
        let b = profile_check_once(target, limits, &mut l)?;
        let c = accounting::target(target, b, &mut l, limits.max_encoded_bytes)?;
        l.reserve(c)?;
        target
            .validate_physical_contract()
            .map_err(|_| rejection(RuntimeMigrationDiagnosticCode::InvalidPhysicalContract))?;
        let version = version.into();
        let description = description.into();
        if version.len() > 256 || description.len() > 4096 || version.trim().is_empty() {
            return Err(rejection(RuntimeMigrationDiagnosticCode::LimitExceeded).into());
        }
        B::validate_owned_dependencies(self.database.pool(), ownership).await?;
        let current = B::introspect_owned(self.database.pool()).await?;
        let a = profile_check_once(&current, limits, &mut l)?;
        let facts = compute_after_target(
            B::DIALECT,
            &version,
            &description,
            &current,
            target,
            ownership,
            options,
            limits,
            a,
            b,
            None,
            &mut l,
        )?;
        let id = Size {
            n: ownership.0.len() as u64 + 8,
            b: version.len() as u64
                + description.len() as u64
                + mul(MAX_PHYSICAL_IDENTIFIER_LEN as u64, ownership.0.len() as u64)?
                + 48,
            longest: 0,
            non_ascii_bytes: 0,
        };
        let f = size(&facts, &mut l, limits.max_encoded_bytes)?;
        l.reserve(
            b.copy()?
                .times(2)?
                .add(id.copy()?.times(2)?)?
                .add(f.copy()?)?,
        )?;
        l.reserve(ownership_tree(ownership.0.len() as u64)?.times(2)?)?;
        let plan = OwnedPlannedMigration {
            version: version.clone(),
            description: description.clone(),
            backend: B::DIALECT.name(),
            source_schema_hash: facts.source_schema_hash.clone(),
            target_schema_hash: facts.target_schema_hash.clone(),
            plan_hash: facts.plan_hash.clone(),
            ownership: ownership.clone(),
            steps: facts.steps.clone(),
            statements: facts.statements.clone(),
            target: target.clone(),
        };
        let data = Data {
            recipe,
            backend: B::DIALECT,
            version,
            description,
            ownership: ownership.clone(),
            options,
            source: current,
            target: target.clone(),
            expected: facts,
        };
        certificate(&data, limits, &mut l)?;
        B::validate_owned_plan(self.database.pool(), &plan).await?;
        Ok((plan, OwnedPlanningReplayWitness { data, limits }))
    }
}

#[cfg(test)]
mod positive;
