//! Checked original filter intent and conservative offline SQL selection semantics.
use super::*;
use std::cmp::Ordering;
use std::sync::Arc;

/// Maximum UTF-8 literal bytes accepted by the core checked filter builder.
pub const RUNTIME_FILTER_MAX_TEXT_BYTES: usize = 64 * 1024;
/// Maximum binary literal bytes accepted by the core checked filter builder.
pub const RUNTIME_FILTER_MAX_BINARY_BYTES: usize = 32 * 1024;

/// Safe checked-filter failure category; contains no literals or record values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeFilterErrorCode {
    /// Invalid filter/operator/literal combination.
    InvalidInput,
    /// A handle is invalid for the builder's schema or collection.
    InvalidHandle,
    /// Original tree, literal or emitted bind bounds exceeded.
    LimitExceeded,
    /// A dependency was not loaded, including one removed by constant lowering.
    MissingLoadedField,
    /// A loaded value has an invalid logical kind or nullability.
    WrongValueKind,
    /// The requested profile/type/storage semantics are not supported.
    UnsupportedSemantics,
    /// A node or record belongs to another schema or collection.
    SchemaMismatch,
}
/// Safe checked-filter failure; source category retained privately for canonical GraphQL compatibility.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RuntimeFilterError {
    code: RuntimeFilterErrorCode,
    query_code: Option<RuntimeQueryErrorCode>,
}
impl RuntimeFilterError {
    pub(crate) const fn new(code: RuntimeFilterErrorCode) -> Self {
        Self {
            code,
            query_code: None,
        }
    }
    /// Value-free failure category.
    pub const fn code(self) -> RuntimeFilterErrorCode {
        self.code
    }
    #[cfg(feature = "runtime-graphql")]
    pub(crate) fn graphql_code(self) -> &'static str {
        if let Some(code) = self.query_code {
            return code.as_str();
        }
        if self.code == RuntimeFilterErrorCode::LimitExceeded {
            "cost_exceeded"
        } else {
            "invalid_graphql_input"
        }
    }
}
impl fmt::Debug for RuntimeFilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RuntimeFilterError")
            .field(&self.code)
            .finish()
    }
}
impl fmt::Display for RuntimeFilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.code {
            RuntimeFilterErrorCode::InvalidInput => "invalid_input",
            RuntimeFilterErrorCode::InvalidHandle => "invalid_handle",
            RuntimeFilterErrorCode::LimitExceeded => "limit_exceeded",
            RuntimeFilterErrorCode::MissingLoadedField => "missing_loaded_field",
            RuntimeFilterErrorCode::WrongValueKind => "wrong_value_kind",
            RuntimeFilterErrorCode::UnsupportedSemantics => "unsupported_semantics",
            RuntimeFilterErrorCode::SchemaMismatch => "schema_mismatch",
        })
    }
}
impl StdError for RuntimeFilterError {}
impl From<RuntimeQueryError> for RuntimeFilterError {
    fn from(e: RuntimeQueryError) -> Self {
        let query_code = e.code();
        let code = match query_code {
            RuntimeQueryErrorCode::InvalidHandle => RuntimeFilterErrorCode::InvalidHandle,
            RuntimeQueryErrorCode::ResourceLimit => RuntimeFilterErrorCode::LimitExceeded,
            RuntimeQueryErrorCode::UnsupportedOperator => {
                RuntimeFilterErrorCode::UnsupportedSemantics
            }
            _ => RuntimeFilterErrorCode::InvalidInput,
        };
        Self {
            code,
            query_code: Some(query_code),
        }
    }
}
/// Boolean intent node, including empty nodes and repeated occurrences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeFilterBoolean {
    /// Conjunction.
    And,
    /// Implicit conjunction of fields/operators in one input object.
    ImplicitAnd,
    /// Disjunction.
    Or,
    /// Negation.
    Not,
}
/// Original operator occurrence. Names never carry caller-controlled values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeFilterOperator {
    /// Scalar comparison or literal string operation.
    Scalar(RuntimeScalarOperator),
    /// Membership, including an empty list.
    List(RuntimeListOperator),
    /// Null test; false is IS NOT NULL.
    IsNull(bool),
    /// Inclusive range.
    Between,
    /// A whole field entry supplied as null (ignored by GraphQL semantics).
    Field,
}
/// Value-free traversal of the complete original tree before constant lowering.
/// Hosts must authorize all occurrences before protected SQL or record disclosure.
pub trait RuntimeFilterIntentVisitor {
    /// Enter a Boolean node. `ignored` identifies an explicit null input entry.
    fn enter_boolean(&mut self, kind: RuntimeFilterBoolean, ignored: bool);
    /// Leave the corresponding Boolean node.
    fn leave_boolean(&mut self, kind: RuntimeFilterBoolean, ignored: bool);
    /// Visit each original field/operator occurrence, including ignored nulls.
    fn field(&mut self, field: &RuntimeFieldHandle, operator: RuntimeFilterOperator, ignored: bool);
}
#[derive(Clone)]
enum Intent {
    #[allow(dead_code)] // Constructed by the optional canonical GraphQL adapter.
    Sequence(Vec<Intent>),
    Boolean(RuntimeFilterBoolean, bool, Vec<Intent>),
    Field(RuntimeFieldHandle, RuntimeFilterOperator, bool),
}
impl Intent {
    fn visit(&self, v: &mut dyn RuntimeFilterIntentVisitor) {
        match self {
            Self::Sequence(children) => {
                v.enter_boolean(RuntimeFilterBoolean::ImplicitAnd, false);
                for c in children {
                    c.visit(v)
                }
                v.leave_boolean(RuntimeFilterBoolean::ImplicitAnd, false)
            }
            Self::Boolean(k, ignored, children) => {
                v.enter_boolean(*k, *ignored);
                for c in children {
                    c.visit(v)
                }
                v.leave_boolean(*k, *ignored);
            }
            Self::Field(f, op, ignored) => v.field(f, *op, *ignored),
        }
    }
    fn validate_profile(&self) -> Result<(), RuntimeFilterError> {
        match self {
            Self::Sequence(v) | Self::Boolean(_, _, v) => {
                for c in v {
                    c.validate_profile()?
                }
            }
            Self::Field(f, op, _) => {
                let supported = f.value_kind() != RuntimeValueKind::Json
                    && match op {
                        RuntimeFilterOperator::Scalar(op) => compare_supported(f.value_kind(), *op),
                        RuntimeFilterOperator::Between => matches!(
                            f.value_kind(),
                            RuntimeValueKind::Integer
                                | RuntimeValueKind::Float
                                | RuntimeValueKind::String
                                | RuntimeValueKind::DateTime
                        ),
                        _ => true,
                    };
                if !supported {
                    return Err(RuntimeFilterError::new(
                        RuntimeFilterErrorCode::UnsupportedSemantics,
                    ));
                }
            }
        }
        Ok(())
    }
    fn fields(&self, out: &mut Vec<RuntimeFieldHandle>) {
        match self {
            Self::Sequence(children) | Self::Boolean(_, _, children) => {
                for c in children {
                    c.fields(out)
                }
            }
            Self::Field(f, _, _) => {
                if !out.contains(f) {
                    out.push(f.clone())
                }
            }
        }
    }
}
/// An owned checked construction node. It has no public unchecked constructor.
#[derive(Clone)]
pub struct RuntimeFilterNode {
    predicate: RuntimePredicate,
    intent: Intent,
}
impl fmt::Debug for RuntimeFilterNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RuntimeFilterNode([redacted])")
    }
}
/// Owned schema-bound builder. All construction validates original tree limits.
#[derive(Clone)]
pub struct RuntimeFilterBuilder {
    schema: Arc<ValidatedRuntimeSchema>,
    collection: RuntimeCollectionHandle,
    limits: RuntimeQueryLimits,
    max_text_bytes: usize,
    max_binary_bytes: usize,
    sqlite_suffix_binds: bool,
    #[cfg(feature = "runtime-graphql")]
    legacy_nulls: bool,
}
impl fmt::Debug for RuntimeFilterBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RuntimeFilterBuilder([redacted])")
    }
}
impl ValidatedRuntimeSchema {
    /// Create a checked, owned filter builder without requiring GraphQL.
    ///
    /// Literal text/binary caps are [`RUNTIME_FILTER_MAX_TEXT_BYTES`] and
    /// [`RUNTIME_FILTER_MAX_BINARY_BYTES`]. The SQL bind budget accounts for
    /// SQLite's duplicated suffix operand as well as logical operands.
    /// # Errors
    /// Rejects a foreign collection or invalid limits.
    pub fn runtime_filter_builder(
        &self,
        collection: &RuntimeCollectionHandle,
        limits: RuntimeQueryLimits,
    ) -> Result<RuntimeFilterBuilder, RuntimeFilterError> {
        check_collection(self, collection)?;
        if limits.max_predicate_nodes == 0 || limits.max_predicate_depth == 0 {
            return Err(limit_error(collection.id()).into());
        }
        Ok(RuntimeFilterBuilder {
            schema: Arc::new(self.clone()),
            collection: collection.clone(),
            limits,
            max_text_bytes: RUNTIME_FILTER_MAX_TEXT_BYTES,
            max_binary_bytes: RUNTIME_FILTER_MAX_BINARY_BYTES,
            sqlite_suffix_binds: true,
            #[cfg(feature = "runtime-graphql")]
            legacy_nulls: false,
        })
    }
}
fn literal(
    value: &RuntimeValue,
    max_text: usize,
    max_binary: usize,
) -> Result<(), RuntimeFilterError> {
    let valid = match value {
        RuntimeValue::String(v) => v.len() <= max_text,
        RuntimeValue::Bytes(v) => v.len() <= max_binary,
        RuntimeValue::Json(_) => {
            return Err(RuntimeFilterError::new(
                RuntimeFilterErrorCode::UnsupportedSemantics,
            ));
        }
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(RuntimeFilterError::new(
            RuntimeFilterErrorCode::LimitExceeded,
        ))
    }
}
fn check_literals(e: &PredicateExpr, text: usize, binary: usize) -> Result<(), RuntimeFilterError> {
    match e {
        PredicateExpr::Compare { value, .. } => literal(value, text, binary),
        PredicateExpr::List { values, .. } => {
            for v in values {
                literal(v, text, binary)?
            }
            Ok(())
        }
        PredicateExpr::Between { low, high, .. } => {
            literal(low, text, binary)?;
            literal(high, text, binary)
        }
        PredicateExpr::And(v) | PredicateExpr::Or(v) => {
            for e in v {
                check_literals(e, text, binary)?
            }
            Ok(())
        }
        PredicateExpr::Not(e) => check_literals(e, text, binary),
        _ => Ok(()),
    }
}
fn within_lists(e: &PredicateExpr, max: usize) -> bool {
    match e {
        PredicateExpr::List { values, .. } => values.len() <= max,
        PredicateExpr::And(v) | PredicateExpr::Or(v) => v.iter().all(|e| within_lists(e, max)),
        PredicateExpr::Not(e) => within_lists(e, max),
        _ => true,
    }
}
fn actual_binds(e: &PredicateExpr, duplicate_suffix: bool) -> Option<usize> {
    match e {
        PredicateExpr::Constant(_) | PredicateExpr::IsNull { .. } => Some(0),
        PredicateExpr::Compare { op, .. } => Some(
            if duplicate_suffix && matches!(op, RuntimeScalarOperator::EndsWith) {
                2
            } else {
                1
            },
        ),
        PredicateExpr::List { values, .. } => Some(values.len()),
        PredicateExpr::Between { .. } => Some(2),
        PredicateExpr::And(v) | PredicateExpr::Or(v) => v.iter().try_fold(0usize, |n, e| {
            n.checked_add(actual_binds(e, duplicate_suffix)?)
        }),
        PredicateExpr::Not(e) => actual_binds(e, duplicate_suffix),
    }
}
impl RuntimeFilterBuilder {
    #[cfg(feature = "runtime-graphql")]
    pub(crate) fn for_canonical_execution(mut self) -> Self {
        self.sqlite_suffix_binds = false;
        self.legacy_nulls = true;
        self
    }
    #[cfg(feature = "runtime-graphql")]
    pub(crate) fn with_graphql_literal_limits(mut self, text: usize, binary: usize) -> Self {
        self.max_text_bytes = text;
        self.max_binary_bytes = binary;
        self
    }
    #[cfg(feature = "runtime-graphql")]
    pub(crate) fn resolve_graphql_field(
        &self,
        id: &FieldId,
    ) -> Result<RuntimeFieldHandle, RuntimeFilterError> {
        self.schema
            .resolve_field(&self.collection, id)
            .map_err(|_| RuntimeFilterError::new(RuntimeFilterErrorCode::InvalidHandle))
    }
    fn node(
        &self,
        predicate: RuntimePredicate,
        intent: Intent,
    ) -> Result<RuntimeFilterNode, RuntimeFilterError> {
        check_literals(&predicate.expr, self.max_text_bytes, self.max_binary_bytes)?;
        if !within_lists(&predicate.expr, self.limits.max_values_per_list)
            || actual_binds(&predicate.expr, self.sqlite_suffix_binds)
                .is_none_or(|n| n > self.limits.max_bind_parameters)
        {
            return Err(RuntimeFilterError::new(
                RuntimeFilterErrorCode::LimitExceeded,
            ));
        }
        Ok(RuntimeFilterNode { predicate, intent })
    }
    fn leaf(
        &self,
        field: &RuntimeFieldHandle,
        op: RuntimeFilterOperator,
        p: RuntimePredicate,
    ) -> Result<RuntimeFilterNode, RuntimeFilterError> {
        self.node(p, Intent::Field(field.clone(), op, false))
    }
    /// Construct a validated scalar comparison.
    /// # Errors
    /// Rejects invalid handles, operators, kinds, literals or bounds.
    pub fn compare(
        &self,
        field: &RuntimeFieldHandle,
        op: RuntimeScalarOperator,
        value: RuntimeValue,
    ) -> Result<RuntimeFilterNode, RuntimeFilterError> {
        literal(&value, self.max_text_bytes, self.max_binary_bytes)?;
        self.leaf(
            field,
            RuntimeFilterOperator::Scalar(op),
            self.schema
                .runtime_compare(&self.collection, field, op, value, self.limits)?,
        )
    }
    /// Construct membership while retaining intent even for an empty list.
    /// # Errors
    /// Rejects invalid handles, values, unsupported kinds or bounds.
    pub fn membership(
        &self,
        field: &RuntimeFieldHandle,
        op: RuntimeListOperator,
        values: Vec<RuntimeValue>,
    ) -> Result<RuntimeFilterNode, RuntimeFilterError> {
        if values.len() > self.limits.max_values_per_list {
            return Err(RuntimeFilterError::new(
                RuntimeFilterErrorCode::LimitExceeded,
            ));
        }
        for v in &values {
            literal(v, self.max_text_bytes, self.max_binary_bytes)?
        }
        self.leaf(
            field,
            RuntimeFilterOperator::List(op),
            self.schema
                .runtime_list(&self.collection, field, op, values, self.limits)?,
        )
    }
    /// Construct a null test.
    /// # Errors
    /// Rejects invalid/unfilterable handles or bounds.
    pub fn is_null(
        &self,
        field: &RuntimeFieldHandle,
        is_null: bool,
    ) -> Result<RuntimeFilterNode, RuntimeFilterError> {
        self.leaf(
            field,
            RuntimeFilterOperator::IsNull(is_null),
            self.schema
                .runtime_is_null(&self.collection, field, is_null, self.limits)?,
        )
    }
    /// Construct an inclusive range.
    /// # Errors
    /// Rejects invalid handles, kinds, literals or bounds.
    pub fn between(
        &self,
        field: &RuntimeFieldHandle,
        low: RuntimeValue,
        high: RuntimeValue,
    ) -> Result<RuntimeFilterNode, RuntimeFilterError> {
        literal(&low, self.max_text_bytes, self.max_binary_bytes)?;
        literal(&high, self.max_text_bytes, self.max_binary_bytes)?;
        self.leaf(
            field,
            RuntimeFilterOperator::Between,
            self.schema
                .runtime_between(&self.collection, field, low, high, self.limits)?,
        )
    }
    fn combine(
        &self,
        children: Vec<RuntimeFilterNode>,
        kind: RuntimeFilterBoolean,
    ) -> Result<RuntimeFilterNode, RuntimeFilterError> {
        if children.len() > self.limits.max_predicate_nodes {
            return Err(RuntimeFilterError::new(
                RuntimeFilterErrorCode::LimitExceeded,
            ));
        }
        let (predicates, intents): (Vec<_>, Vec<_>) = children
            .into_iter()
            .map(|n| (n.predicate, n.intent))
            .unzip();
        let p = if kind == RuntimeFilterBoolean::And {
            self.schema
                .runtime_and(&self.collection, predicates, self.limits)?
        } else {
            self.schema
                .runtime_or(&self.collection, predicates, self.limits)?
        };
        self.node(p, Intent::Boolean(kind, false, intents))
    }
    /// Conjoin children; empty AND is true, with an original empty intent node.
    /// # Errors
    /// Rejects foreign nodes or original tree/bind limits.
    pub fn and(
        &self,
        children: Vec<RuntimeFilterNode>,
    ) -> Result<RuntimeFilterNode, RuntimeFilterError> {
        self.combine(children, RuntimeFilterBoolean::And)
    }
    /// Disjoin children; empty OR is false.
    /// # Errors
    /// Rejects foreign nodes or original tree/bind limits.
    pub fn or(
        &self,
        children: Vec<RuntimeFilterNode>,
    ) -> Result<RuntimeFilterNode, RuntimeFilterError> {
        self.combine(children, RuntimeFilterBoolean::Or)
    }
    /// Negate a node using SQL three-valued semantics.
    /// # Errors
    /// Rejects foreign nodes or original tree/bind limits.
    pub fn not(&self, child: RuntimeFilterNode) -> Result<RuntimeFilterNode, RuntimeFilterError> {
        self.node(
            self.schema
                .runtime_not(&self.collection, child.predicate, self.limits)?,
            Intent::Boolean(RuntimeFilterBoolean::Not, false, vec![child.intent]),
        )
    }
    /// Finalize original intent separately from the optimized SQL predicate.
    /// # Errors
    /// Rejects a node from another schema/collection or stricter bounds.
    pub fn finish(
        self,
        root: RuntimeFilterNode,
    ) -> Result<CheckedRuntimeFilter, RuntimeFilterError> {
        if root.predicate.schema != self.schema.fingerprint()
            || root.predicate.collection != *self.collection.id()
        {
            return Err(RuntimeFilterError::new(
                RuntimeFilterErrorCode::SchemaMismatch,
            ));
        }
        check_literals(
            &root.predicate.expr,
            self.max_text_bytes,
            self.max_binary_bytes,
        )?;
        if !within_lists(&root.predicate.expr, self.limits.max_values_per_list)
            || root.predicate.nodes > self.limits.max_predicate_nodes
            || root.predicate.depth > self.limits.max_predicate_depth
            || actual_binds(&root.predicate.expr, self.sqlite_suffix_binds)
                .is_none_or(|n| n > self.limits.max_bind_parameters)
        {
            return Err(RuntimeFilterError::new(
                RuntimeFilterErrorCode::LimitExceeded,
            ));
        }
        Ok(CheckedRuntimeFilter { root })
    }
    #[cfg(feature = "runtime-graphql")]
    pub(crate) fn implicit_and(
        &self,
        children: Vec<RuntimeFilterNode>,
    ) -> Result<RuntimeFilterNode, RuntimeFilterError> {
        let (predicates, intents): (Vec<_>, Vec<_>) = children
            .into_iter()
            .map(|n| (n.predicate, n.intent))
            .unzip();
        self.node(
            self.schema
                .runtime_and(&self.collection, predicates, self.limits)?,
            Intent::Sequence(intents),
        )
    }
    #[cfg(feature = "runtime-graphql")]
    pub(crate) fn ignored_field(
        &self,
        field: &RuntimeFieldHandle,
        op: RuntimeFilterOperator,
    ) -> Result<RuntimeFilterNode, RuntimeFilterError> {
        check_handle(&self.schema, &self.collection, field)?;
        if !self.legacy_nulls {
            if !field.filterable() {
                return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidFilter).into());
            }
            let supported = match op {
                RuntimeFilterOperator::Scalar(op) => compare_supported(field.value_kind(), op),
                RuntimeFilterOperator::List(_) => field.value_kind() != RuntimeValueKind::Json,
                RuntimeFilterOperator::Between => matches!(
                    field.value_kind(),
                    RuntimeValueKind::Integer
                        | RuntimeValueKind::Float
                        | RuntimeValueKind::String
                        | RuntimeValueKind::DateTime
                ),
                _ => true,
            };
            if !supported {
                return Err(
                    RuntimeQueryError::new(RuntimeQueryErrorCode::UnsupportedOperator).into(),
                );
            }
        }
        self.node(
            self.ignored_predicate()?,
            Intent::Field(field.clone(), op, true),
        )
    }
    #[cfg(feature = "runtime-graphql")]
    fn ignored_predicate(&self) -> Result<RuntimePredicate, RuntimeFilterError> {
        let mut p = self
            .schema
            .runtime_and(&self.collection, vec![], self.limits)?;
        // Legacy canonical execution already bounds raw input separately and
        // historically omitted these entries from the SQL predicate budget.
        // Public checked construction never uses this private budget policy.
        if self.legacy_nulls {
            p.nodes = 0;
            p.depth = 0;
        }
        Ok(p)
    }
    #[cfg(feature = "runtime-graphql")]
    pub(crate) fn ignored_boolean(
        &self,
        kind: RuntimeFilterBoolean,
    ) -> Result<RuntimeFilterNode, RuntimeFilterError> {
        self.node(
            self.ignored_predicate()?,
            Intent::Boolean(kind, true, vec![]),
        )
    }
}
/// Immutable filter plus complete original intent. No authorization is implied.
#[derive(Clone)]
pub struct CheckedRuntimeFilter {
    root: RuntimeFilterNode,
}
impl fmt::Debug for CheckedRuntimeFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CheckedRuntimeFilter([redacted])")
    }
}
impl CheckedRuntimeFilter {
    /// Optimized structural predicate. Referenced fields alone are not full intent.
    pub fn predicate(&self) -> &RuntimePredicate {
        &self.root.predicate
    }
    /// Visit every original Boolean/field/operator occurrence, including ignored nulls.
    pub fn visit_intent(&self, visitor: &mut dyn RuntimeFilterIntentVisitor) {
        self.root.intent.visit(visitor)
    }
    /// Prepare a conservative matcher under the host's explicit storage assertion.
    /// # Errors
    /// Rejects unsupported profile/type semantics anywhere, before evaluation.
    pub fn prepare_match(
        &self,
        profile: RuntimeMatchProfile,
    ) -> Result<RuntimeRecordMatcher, RuntimeFilterError> {
        if profile == RuntimeMatchProfile::MssqlUnsupported {
            return Err(RuntimeFilterError::new(
                RuntimeFilterErrorCode::UnsupportedSemantics,
            ));
        }
        self.root.intent.validate_profile()?;
        let mut fields = Vec::new();
        self.root.intent.fields(&mut fields);
        if fields
            .iter()
            .any(|f| f.value_kind() == RuntimeValueKind::Json)
        {
            return Err(RuntimeFilterError::new(
                RuntimeFilterErrorCode::UnsupportedSemantics,
            ));
        }
        validate_expr(&self.root.predicate.expr)?;
        Ok(RuntimeRecordMatcher {
            filter: self.clone(),
            fields,
            profile,
        })
    }
}
/// Explicit host assertion about the actual database storage semantics.
/// Neither backend naming nor a record codec establishes this assertion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeMatchProfile {
    /// SQLite UTF-8 String TEXT with BINARY collation and canonical physical storage: booleans INTEGER0/1,
    /// exact i64 INTEGER, finite REAL, byte BLOB, canonical lowercase UUID TEXT
    /// and canonical microsecond UTC date-time TEXT (years0001–9999, no leap second).
    /// The host asserts these invariants; decoding cannot recover prior storage
    /// spelling/class or detect externally written noncanonical values.
    SqliteBinaryCanonical,
    /// PostgreSQL UTF-8 C collation with String physically TEXT (or proven
    /// equivalent VARCHAR, never fixed CHAR/bpchar), native typed values and
    /// finite numeric values; microsecond UTC years0001–9999 without leap seconds.
    /// Logical kind/decoding alone cannot establish physical storage semantics.
    PostgresCUtf8,
    /// Reserved unsupported profile; always rejects before matching.
    MssqlUnsupported,
}
/// Offline matcher. No provenance, current authority, policy or native RLS proof.
#[derive(Clone)]
pub struct RuntimeRecordMatcher {
    filter: CheckedRuntimeFilter,
    fields: Vec<RuntimeFieldHandle>,
    profile: RuntimeMatchProfile,
}
impl fmt::Debug for RuntimeRecordMatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeRecordMatcher")
            .field("profile", &self.profile)
            .finish_non_exhaustive()
    }
}
fn valid_value(value: &RuntimeValue) -> Result<(), RuntimeFilterError> {
    match value {
        RuntimeValue::String(s) if s.chars().any(char::is_control) => Err(RuntimeFilterError::new(
            RuntimeFilterErrorCode::UnsupportedSemantics,
        )),
        RuntimeValue::Json(_) => Err(RuntimeFilterError::new(
            RuntimeFilterErrorCode::UnsupportedSemantics,
        )),
        RuntimeValue::DateTime(t)
            if t.as_str().len() != 27
                || t.as_str().starts_with("0000")
                || t.as_str().get(17..19) == Some("60") =>
        {
            Err(RuntimeFilterError::new(
                RuntimeFilterErrorCode::UnsupportedSemantics,
            ))
        }
        _ => Ok(()),
    }
}
fn validate_expr(e: &PredicateExpr) -> Result<(), RuntimeFilterError> {
    match e {
        PredicateExpr::Compare { value, .. } => valid_value(value),
        PredicateExpr::List { values, .. } => {
            for v in values {
                valid_value(v)?
            }
            Ok(())
        }
        PredicateExpr::Between { low, high, .. } => {
            valid_value(low)?;
            valid_value(high)
        }
        PredicateExpr::And(v) | PredicateExpr::Or(v) => {
            for e in v {
                validate_expr(e)?
            }
            Ok(())
        }
        PredicateExpr::Not(e) => validate_expr(e),
        _ => Ok(()),
    }
}
impl RuntimeRecordMatcher {
    /// Evaluate an authoritative immutable record; only SQL TRUE selects it.
    /// All original dependencies are checked before Boolean short-circuiting.
    /// Offline matching cannot prove a deleted row's current SELECT/RLS policy.
    /// # Errors
    /// Rejects schema/collection drift, unloaded fields, wrong kinds and unsupported values.
    pub fn matches(&self, record: &RuntimeRecord) -> Result<bool, RuntimeFilterError> {
        let p = &self.filter.root.predicate;
        if record.schema_fingerprint() != &p.schema || record.collection_id() != &p.collection {
            return Err(RuntimeFilterError::new(
                RuntimeFilterErrorCode::SchemaMismatch,
            ));
        }
        for f in &self.fields {
            let v = record_value(record, f)?;
            if (matches!(v, RuntimeValue::Null) && !f.nullable())
                || v.kind().is_some_and(|k| k != f.value_kind())
            {
                return Err(RuntimeFilterError::new(
                    RuntimeFilterErrorCode::WrongValueKind,
                ));
            }
            valid_value(v)?;
        }
        Ok(eval(&p.expr, record)? == Some(true))
    }
}
fn record_value<'a>(
    record: &'a RuntimeRecord,
    field: &RuntimeFieldHandle,
) -> Result<&'a RuntimeValue, RuntimeFilterError> {
    match record
        .state(field)
        .map_err(|_| RuntimeFilterError::new(RuntimeFilterErrorCode::WrongValueKind))?
    {
        super::super::RuntimeFieldState::Unloaded => Err(RuntimeFilterError::new(
            RuntimeFilterErrorCode::MissingLoadedField,
        )),
        super::super::RuntimeFieldState::Null => Ok(&RuntimeValue::Null),
        super::super::RuntimeFieldState::Value(v) => Ok(v),
    }
}
fn compare_values(a: &RuntimeValue, b: &RuntimeValue) -> Result<Ordering, RuntimeFilterError> {
    let result = match (a, b) {
        (RuntimeValue::Boolean(a), RuntimeValue::Boolean(b)) => Some(a.cmp(b)),
        (RuntimeValue::Integer(a), RuntimeValue::Integer(b)) => Some(a.cmp(b)),
        (RuntimeValue::Float(a), RuntimeValue::Float(b)) => a.get().partial_cmp(&b.get()),
        (RuntimeValue::String(a), RuntimeValue::String(b)) => Some(a.as_bytes().cmp(b.as_bytes())),
        (RuntimeValue::Uuid(a), RuntimeValue::Uuid(b)) => Some(a.as_bytes().cmp(b.as_bytes())),
        (RuntimeValue::Bytes(a), RuntimeValue::Bytes(b)) => Some(a.cmp(b)),
        (RuntimeValue::DateTime(a), RuntimeValue::DateTime(b)) => Some(a.as_str().cmp(b.as_str())),
        _ => None,
    };
    result.ok_or(RuntimeFilterError::new(
        RuntimeFilterErrorCode::WrongValueKind,
    ))
}
fn eval(e: &PredicateExpr, r: &RuntimeRecord) -> Result<Option<bool>, RuntimeFilterError> {
    let value = |f: &RuntimeFieldHandle| record_value(r, f);
    Ok(match e {
        PredicateExpr::Constant(v) => Some(*v),
        PredicateExpr::IsNull { field, is_null } => {
            Some(matches!(value(field)?, RuntimeValue::Null) == *is_null)
        }
        PredicateExpr::Compare {
            field,
            op,
            value: operand,
        } => {
            let actual = value(field)?;
            if matches!(actual, RuntimeValue::Null) {
                None
            } else {
                use RuntimeScalarOperator as O;
                Some(match op {
                    O::Contains | O::StartsWith | O::EndsWith => {
                        let (RuntimeValue::String(a), RuntimeValue::String(b)) = (actual, operand)
                        else {
                            return Err(RuntimeFilterError::new(
                                RuntimeFilterErrorCode::WrongValueKind,
                            ));
                        };
                        match op {
                            O::Contains => a.contains(b),
                            O::StartsWith => a.starts_with(b),
                            _ => a.ends_with(b),
                        }
                    }
                    _ => {
                        let c = compare_values(actual, operand)?;
                        match op {
                            O::Eq => c == Ordering::Equal,
                            O::Ne => c != Ordering::Equal,
                            O::Lt => c == Ordering::Less,
                            O::Lte => c != Ordering::Greater,
                            O::Gt => c == Ordering::Greater,
                            O::Gte => c != Ordering::Less,
                            _ => unreachable!(),
                        }
                    }
                })
            }
        }
        PredicateExpr::List { field, op, values } => {
            let actual = value(field)?;
            if matches!(actual, RuntimeValue::Null) {
                None
            } else {
                let mut found = false;
                for v in values {
                    found |= compare_values(actual, v)? == Ordering::Equal
                }
                Some(found == matches!(op, RuntimeListOperator::In))
            }
        }
        PredicateExpr::Between { field, low, high } => {
            let actual = value(field)?;
            if matches!(actual, RuntimeValue::Null) {
                None
            } else {
                Some(
                    compare_values(actual, low)? != Ordering::Less
                        && compare_values(actual, high)? != Ordering::Greater,
                )
            }
        }
        PredicateExpr::And(v) => {
            let mut result = Some(true);
            for e in v {
                match eval(e, r)? {
                    Some(false) => result = Some(false),
                    None if result != Some(false) => result = None,
                    _ => {}
                }
            }
            result
        }
        PredicateExpr::Or(v) => {
            let mut result = Some(false);
            for e in v {
                match eval(e, r)? {
                    Some(true) => result = Some(true),
                    None if result != Some(true) => result = None,
                    _ => {}
                }
            }
            result
        }
        PredicateExpr::Not(e) => eval(e, r)?.map(|v| !v),
    })
}
