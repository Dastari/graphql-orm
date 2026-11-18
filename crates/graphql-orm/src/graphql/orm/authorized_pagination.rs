//! Request-local database visibility and bounded residual authorization scans.
use super::*;
use crate::db::Database;
use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
use crate::graphql::pagination::*;
use chacha20poly1305::{
    XChaCha20Poly1305,
    aead::{Aead, AeadCore, KeyInit, OsRng},
};
use std::any::TypeId;

fn invalid() -> async_graphql::Error {
    OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured).into_graphql_error()
}
fn input_error() -> async_graphql::Error {
    OrmPublicError::new(OrmErrorCode::InvalidInput).into_graphql_error()
}
fn query_error(error: sqlx::Error) -> async_graphql::Error {
    OrmPublicError::from(error).into_graphql_error()
}

/// Associates a typed generated filter with its entity. Implementors must render
/// parameterized expressions for that entity and accurately report residual work.
pub trait ReadVisibilityFilter: DatabaseFilter {
    fn entity_type_id() -> TypeId;
    fn backend() -> DatabaseBackend;
}

/// Closed set of scalar comparisons for trusted read policy columns.
/// Operands use SQL three-valued logic, including when the value is NULL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadPredicateOp {
    /// Equal (`=`).
    Eq,
    /// Not equal (`<>`).
    Ne,
    /// Less than (`<`).
    Lt,
    /// Less than or equal (`<=`).
    Lte,
    /// Greater than (`>`).
    Gt,
    /// Greater than or equal (`>=`).
    Gte,
}

/// Entity- and backend-bound authorization predicate, built from the same typed
/// filters used by ordinary queries (including supported relation predicates).
/// An empty filter is rejected: use `ReadVisibility::Unrestricted` deliberately.
///
/// Composition is budgeted when a predicate is built. A constructor,
/// [`Self::not`], `and` or `or` that would exceed a limit returns
/// `AUTHORIZATION_MISCONFIGURED` before rendering:
///
/// - on every backend, at most [`READ_PREDICATE_MAX_RENDERED_BYTES`] of
///   rendered SQL, [`READ_PREDICATE_MAX_EXPRESSION_DEPTH`] levels of
///   expression nesting and [`READ_PREDICATE_CLOSURE_MAX_NESTING`] closures
///   nested inside one another;
/// - on SQL Server, also at most [`READ_PREDICATE_MSSQL_MAX_NESTING`] nested
///   subqueries and [`READ_PREDICATE_MSSQL_MAX_NESTING_UNITS`] weighted
///   levels.
///
/// `and` and `or` flatten a child of the same kind, so a fold of any length
/// is one level deep, and long lists render in balanced groups. Within these
/// limits a predicate renders, formats and drops without deep recursion, and
/// the server accepts its nesting. The limits do not cover a statement's
/// bind-parameter count, which each server caps itself (SQL Server at 2,100
/// for the whole statement).
#[derive(Clone, Debug)]
pub struct ReadPredicate {
    entity: TypeId,
    backend: DatabaseBackend,
    expression: FilterExpression,
    cost: Cost,
}
fn validate_column<T: Entity>(column: &str) -> async_graphql::Result<()> {
    if T::columns()
        .iter()
        .any(|definition| definition.name == column)
    {
        Ok(())
    } else {
        Err(invalid())
    }
}

/// Every fragment is non-empty and every conjunction has a child. An empty
/// disjunction is accepted here and refused as a match-nothing term.
fn negatable(expression: &FilterExpression) -> bool {
    match expression {
        FilterExpression::Raw { clause, .. } | FilterExpression::TrustedFragment { clause, .. } => {
            !clause.trim().is_empty()
        }
        FilterExpression::And(children) => !children.is_empty() && children.iter().all(negatable),
        FilterExpression::Or(children) => children.iter().all(negatable),
    }
}

/// Whether rendered policy SQL contains the ORM's never-true term `1 = 0`
/// (an empty `in_list` or `or` list, or another generated match-nothing
/// fallback) as a standalone comparison. Bound values are placeholders, so a
/// clause never contains caller text.
fn contains_match_nothing(clause: &str) -> bool {
    fn word(c: char) -> bool {
        c.is_alphanumeric()
            || matches!(
                c,
                '_' | '"' | '\'' | '`' | '[' | ']' | '.' | '$' | '@' | '?'
            )
    }
    let chars = clause.chars().collect::<Vec<_>>();
    let skip = |mut index: usize| {
        while chars.get(index).is_some_and(|c| c.is_whitespace()) {
            index += 1;
        }
        index
    };
    (0..chars.len()).any(|start| {
        if chars[start] != '1' || (start > 0 && word(chars[start - 1])) {
            return false;
        }
        let equals = skip(start + 1);
        if chars.get(equals) != Some(&'=') {
            return false;
        }
        let zero = skip(equals + 1);
        chars.get(zero) == Some(&'0') && !chars.get(zero + 1).is_some_and(|c| word(*c))
    })
}

/// Hops a closure predicate follows when the caller gives no bound: see
/// [`ReadPredicate::related_closure`].
pub const READ_PREDICATE_CLOSURE_DEFAULT_DEPTH: u32 = 64;

/// The largest hop bound [`ReadPredicate::related_closure_within`] accepts.
/// A larger bound is refused with `AUTHORIZATION_MISCONFIGURED`.
pub const READ_PREDICATE_CLOSURE_MAX_DEPTH: u32 = 1024;

/// How a closure predicate quantifies over the rows it reaches: see
/// [`ReadPredicate::related_closure_within`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadPredicateQuantifier {
    /// Some reached row matches (existential).
    Any,
    /// Every reached row matches, and the walk is complete (universal).
    All,
}

/// The nesting level for a new closure CTE: one above any closure already
/// rendered inside `predicate`, so nested closures never shadow each other.
fn closure_level(predicate: &str) -> usize {
    predicate
        .match_indices("__gom_closure_")
        .filter_map(|(start, marker)| {
            predicate[start + marker.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<usize>()
                .ok()
        })
        .max()
        .unwrap_or(0)
        + 1
}

/// The largest hop bound [`ReadPredicate::related_closure_within`] accepts on
/// SQL Server, where the walk is unrolled into one nested derived table per
/// hop. SQL Server refuses statements nested much deeper ("nested too
/// deeply", error 191), so a larger bound is refused with
/// `AUTHORIZATION_MISCONFIGURED`. It equals
/// [`READ_PREDICATE_CLOSURE_DEFAULT_DEPTH`], so `related_closure` and
/// `related_closure_all` behave the same on every backend.
pub const READ_PREDICATE_CLOSURE_MSSQL_MAX_DEPTH: u32 = 64;

/// The largest rendered SQL, in bytes, that one [`ReadPredicate`] may hold on
/// any backend. Relations, negations and closures inline their target (a
/// closure four times), so composition can otherwise grow the statement
/// geometrically. A predicate that would exceed it is refused with
/// `AUTHORIZATION_MISCONFIGURED` before it is rendered.
pub const READ_PREDICATE_MAX_RENDERED_BYTES: usize = 1 << 20;

/// How many closures may nest inside one another, through any number of
/// relations, negations, `and` and `or`, on any backend: a closure whose
/// target holds a closure is allowed, a third level is refused with
/// `AUTHORIZATION_MISCONFIGURED`. Each level evaluates the inner walk for
/// every row the outer walk reaches, so the cost multiplies per level.
pub const READ_PREDICATE_CLOSURE_MAX_NESTING: u32 = 2;

/// The deepest subquery nesting one [`ReadPredicate`] may render on SQL
/// Server, counted as nested `(SELECT ...)` levels. SQL Server refuses a
/// statement nested about 100 levels deep ("nested too deeply", error 191);
/// the rest is left for the host query, such as a page, count or relation
/// filter around the policy. A deeper predicate is refused with
/// `AUTHORIZATION_MISCONFIGURED` when it is built. A closure at
/// [`READ_PREDICATE_CLOSURE_MSSQL_MAX_DEPTH`] uses about 66 levels.
pub const READ_PREDICATE_MSSQL_MAX_NESTING: u32 = MSSQL_MAX_NESTING;
const MSSQL_MAX_NESTING: u32 = 80;

/// The deepest expression nesting one [`ReadPredicate`] may render on any
/// backend, counted as nested parentheses: each `not()`, each `and`/`or`
/// level, each relation and each subquery adds at least one. Rendering and
/// dropping a predicate recurse once per level, so this keeps that recursion
/// far inside a 2 MiB thread stack, even in debug builds. A deeper predicate
/// is refused with `AUTHORIZATION_MISCONFIGURED` when it is built. `and` and
/// `or` folds are flattened, so folding any number of terms adds one level.
pub const READ_PREDICATE_MAX_EXPRESSION_DEPTH: u32 = MAX_EXPRESSION_DEPTH;
const MAX_EXPRESSION_DEPTH: u32 = 256;

/// The deepest weighted nesting one [`ReadPredicate`] may render on SQL
/// Server. SQL Server's "nested too deeply" (error 191) counts subqueries,
/// negations and plain parentheses together, at different weights. On the
/// owned server, a statement failed from about 96 nested subqueries alone,
/// from about 525 nested `NOT (...)` alone, and in between on mixtures, in
/// proportion. So each level along the deepest path is weighted: a subquery
/// 11, a `NOT (` 2 and any other parenthesis 1. Failures start at about
/// 1,050 units; this limit leaves about 15% under that. It applies together
/// with [`READ_PREDICATE_MSSQL_MAX_NESTING`] and
/// [`READ_PREDICATE_MAX_EXPRESSION_DEPTH`].
pub const READ_PREDICATE_MSSQL_MAX_NESTING_UNITS: u32 = MSSQL_MAX_NESTING_UNITS;
const MSSQL_MAX_NESTING_UNITS: u32 = 900;
const SUBQUERY_UNITS: u32 = 11;
const NEGATION_UNITS: u32 = 2;

/// What a predicate costs to render and run: its rendered size, its deepest
/// parenthesis and subquery nesting, and its deepest chain of closures inside
/// closures. `units` is the SQL Server weighted nesting. `inner` and
/// `inner_units` are those of the deepest child of a flattened `and`/`or`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Cost {
    bytes: usize,
    depth: u32,
    inner: u32,
    units: u32,
    inner_units: u32,
    nesting: u32,
    closures: u32,
}
impl Cost {
    fn of_expression(expression: &FilterExpression) -> Self {
        let clause =
            render_filter_expression(DatabaseBackend::Sqlite, expression, &mut 1, &mut Vec::new());
        Self::of(&clause)
    }
    fn of(clause: &str) -> Self {
        // Placeholders render as `$n` or `@Pn` on the server: count each as
        // up to seven bytes rather than the anonymous `?`.
        let placeholders = clause.bytes().filter(|byte| *byte == b'?').count();
        let Nesting {
            depth,
            units,
            subqueries: nesting,
        } = nesting(clause);
        Self {
            bytes: clause.len().saturating_add(placeholders.saturating_mul(6)),
            depth,
            inner: depth,
            units,
            inner_units: units,
            nesting,
            closures: closure_level(clause) as u32 - 1,
        }
    }
    fn check(self, backend: DatabaseBackend) -> async_graphql::Result<Self> {
        if self.bytes > READ_PREDICATE_MAX_RENDERED_BYTES
            || self.closures > READ_PREDICATE_CLOSURE_MAX_NESTING
            || self.depth > MAX_EXPRESSION_DEPTH
            || (backend == DatabaseBackend::Mssql
                && (self.nesting > MSSQL_MAX_NESTING || self.units > MSSQL_MAX_NESTING_UNITS))
        {
            return Err(invalid());
        }
        Ok(self)
    }
}

/// Refuse before rendering when `copies` of a `bytes`-long target plus
/// `overhead` would exceed the rendered-size budget.
fn reserve(bytes: usize, copies: usize, overhead: usize) -> async_graphql::Result<()> {
    if bytes.saturating_mul(copies).saturating_add(overhead) > READ_PREDICATE_MAX_RENDERED_BYTES {
        return Err(invalid());
    }
    Ok(())
}

/// How deeply rendered SQL nests along its deepest paths.
struct Nesting {
    /// Parentheses of any kind.
    depth: u32,
    /// SQL Server weighted levels: see [`READ_PREDICATE_MSSQL_MAX_NESTING_UNITS`].
    units: u32,
    /// `(SELECT ...)` and `(WITH ...)` subqueries.
    subqueries: u32,
}

/// Measure `sql` in one pass, skipping quoted strings and identifiers.
fn nesting(sql: &str) -> Nesting {
    let bytes = sql.as_bytes();
    // Per open parenthesis: (is a subquery, weight).
    let mut open: Vec<(bool, u32)> = Vec::new();
    let (mut subqueries, mut units) = (0_u32, 0_u32);
    let mut deepest = Nesting {
        depth: 0,
        units: 0,
        subqueries: 0,
    };
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            quote @ (b'\'' | b'"' | b'[') => {
                let close = if quote == b'[' { b']' } else { quote };
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == close {
                        if bytes.get(index + 1) == Some(&close) {
                            index += 1;
                        } else {
                            break;
                        }
                    }
                    index += 1;
                }
            }
            b'(' => {
                let rest = sql[index + 1..].trim_start().as_bytes();
                let starts = |word: &[u8]| {
                    rest.get(..word.len())
                        .is_some_and(|head| head.eq_ignore_ascii_case(word))
                };
                let before = sql[..index].trim_end().as_bytes();
                let negation = before.len() >= 3
                    && before[before.len() - 3..].eq_ignore_ascii_case(b"NOT")
                    && !before
                        .get(before.len().wrapping_sub(4))
                        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_');
                let subquery = starts(b"SELECT") || starts(b"WITH");
                let weight = if subquery {
                    SUBQUERY_UNITS
                } else if negation {
                    NEGATION_UNITS
                } else {
                    1
                };
                subqueries += u32::from(subquery);
                units += weight;
                open.push((subquery, weight));
                deepest.depth = deepest.depth.max(open.len() as u32);
                deepest.units = deepest.units.max(units);
                deepest.subqueries = deepest.subqueries.max(subqueries);
            }
            b')' => {
                if let Some((subquery, weight)) = open.pop() {
                    subqueries -= u32::from(subquery);
                    units -= weight;
                }
            }
            _ => {}
        }
        index += 1;
    }
    deepest
}

/// The SQL Server closure. SQL Server does not allow a common table
/// expression inside a subquery, so the bounded walk is unrolled into a
/// breadth-first search of nested derived tables, one per hop:
///
/// 1. Level 0 holds the authorized row's pointer at depth 0. Level `k` holds
///    every pointer reached within `k` hops with the hop it was first reached
///    at: level `k - 1`, plus the distinct pointers of the rows named by its
///    frontier (the pointers first reached at hop `k - 1`), grouped by pointer
///    keeping the minimum depth. Each level is deduplicated, so its size is
///    the number of distinct pointers reached, whatever the fan-out. Only
///    pointer columns are carried; the target is not evaluated in the walk.
/// 2. Each pointer below the bound is followed once more to evaluate the
///    target over the rows it names, which are the rows the recursive CTE's
///    step reaches, giving their MAX/MIN truth and whether any exist.
/// 3. A pointer makes the walk incomplete when it is partially NULL, when it
///    is not NULL and names no row, or when it is first reached at the bound
///    (the CTE's "at the bound and not seen before"). An all-NULL pointer
///    names no row and is complete: a root.
/// 4. The authorized row's own truth is evaluated first, in a derived table
///    with no FROM, so the target's columns bind to the authorized row.
///
/// The CTE form expands a pointer each time it is reached, but a later
/// expansion only reaches pointers already reached earlier, so both forms see
/// the same pointers, first depths and reached rows, and the verdict below is
/// the CTE's. Each hop's `OUTER APPLY` carries `TOP (2147483647)`, which is
/// no limit but keeps the hop a correlated lookup so the plan does not grow
/// with join reordering. The target appears four times (twice per truth), as
/// in the CTE form.
fn render_mssql_closure(
    dialect: DatabaseBackend,
    table: &str,
    prefix: &str,
    keys: &[(&str, &str)],
    quantifier: ReadPredicateQuantifier,
    predicate: &str,
    max_depth: u32,
) -> String {
    let columns = |render: &dyn Fn(usize, &str, &str) -> String, separator: &str| {
        keys.iter()
            .enumerate()
            .map(|(index, &(pointer, key))| render(index, pointer, key))
            .collect::<Vec<_>>()
            .join(separator)
    };
    let of = |alias: &str| columns(&|index, _, _| format!("{alias}.__gom_p{index}"), ", ");
    let names = columns(&|index, _, _| format!("__gom_p{index}"), ", ");
    let pointers = columns(
        &|index, pointer, _| {
            format!(
                "{table}.{} AS __gom_p{index}",
                dialect.quote_identifier(pointer)
            )
        },
        ", ",
    );
    // The rows of `table` whose key equals the pointer carried by `alias`.
    let named_by = |alias: &str| {
        columns(
            &|index, _, key| {
                format!(
                    "{table}.{} = {alias}.__gom_p{index}",
                    dialect.quote_identifier(key)
                )
            },
            " AND ",
        )
    };
    let group = format!("{prefix}_g");
    let level = |depth: u32| {
        if depth == max_depth {
            group.clone()
        } else {
            format!("{prefix}_l{depth}")
        }
    };
    let mut walk = format!("(SELECT {pointers}, 0 AS __gom_depth) AS {}", level(0));
    for depth in 1..=max_depth {
        let previous = level(depth - 1);
        let child = format!("{prefix}_c{depth}");
        let merged = format!("{prefix}_v{depth}");
        walk = format!(
            "(SELECT {merged_names}, MIN({merged}.__gom_depth) AS __gom_depth FROM {walk} \
             OUTER APPLY (SELECT DISTINCT TOP (2147483647) 1 AS __gom_r, {pointers} FROM {table} \
             WHERE {previous}.__gom_depth = {frontier} AND {step}) AS {child} \
             CROSS APPLY (VALUES ({previous_names}, {previous}.__gom_depth), \
             ({child_names}, CASE WHEN {child}.__gom_r IS NOT NULL THEN {depth} END)) \
             AS {merged} ({names}, __gom_depth) WHERE {merged}.__gom_depth IS NOT NULL \
             GROUP BY {merged_names}) AS {current}",
            merged_names = of(&merged),
            frontier = depth - 1,
            step = named_by(&previous),
            previous_names = of(&previous),
            child_names = of(&child),
            current = level(depth),
        );
    }
    let followed = columns(
        &|index, _, _| format!("{group}.__gom_p{index} IS NOT NULL"),
        " AND ",
    );
    let ambiguous = if keys.len() > 1 {
        format!(
            "(NOT ({followed}) AND ({})) OR ",
            columns(
                &|index, _, _| format!("{group}.__gom_p{index} IS NOT NULL"),
                " OR "
            )
        )
    } else {
        String::new()
    };
    // 2 = TRUE, 1 = UNKNOWN, 0 = FALSE for the row in scope as `table`.
    let truth = format!("CASE WHEN ({predicate}) THEN 2 WHEN NOT ({predicate}) THEN 0 ELSE 1 END");
    let reached = format!("{prefix}_r");
    let truths = format!("{prefix}_x");
    let summary = format!("{prefix}_s");
    let own = format!("{prefix}_a");
    let verdict = match quantifier {
        ReadPredicateQuantifier::Any => format!(
            "CASE WHEN {own}.__gom_truth = 2 OR {summary}.__gom_max = 2 THEN 1 \
             WHEN {own}.__gom_truth = 1 OR {summary}.__gom_max = 1 OR {summary}.__gom_incomplete = 1 \
             THEN NULL ELSE 0 END"
        ),
        ReadPredicateQuantifier::All => format!(
            "CASE WHEN {own}.__gom_truth = 0 OR {summary}.__gom_min = 0 THEN 0 \
             WHEN {own}.__gom_truth = 1 OR {summary}.__gom_min = 1 OR {summary}.__gom_incomplete = 1 \
             THEN NULL ELSE 1 END"
        ),
    };
    // `table` is unaliased where the target is evaluated. In `truths` it is
    // the reached row. `own` has no FROM and comes first, so it sees no
    // `__gom_` column of this subquery: the target's unqualified columns
    // bind to the authorized row, never to the summary.
    format!(
        "(SELECT {verdict} FROM (SELECT {truth} AS __gom_truth) AS {own} CROSS APPLY (\
         SELECT MAX({reached}.__gom_max) AS __gom_max, MIN({reached}.__gom_min) AS __gom_min, \
         MAX(CASE WHEN {ambiguous}({followed} AND ({group}.__gom_depth = {max_depth} \
         OR {reached}.__gom_rows = 0)) THEN 1 ELSE 0 END) AS __gom_incomplete FROM {walk} \
         OUTER APPLY (SELECT MAX({truths}.__gom_truth) AS __gom_max, \
         MIN({truths}.__gom_truth) AS __gom_min, COUNT(*) AS __gom_rows \
         FROM (SELECT {truth} AS __gom_truth FROM {table} \
         WHERE {step} AND {group}.__gom_depth < {max_depth}) AS {truths}) AS {reached}) AS {summary}) = 1",
        step = named_by(&group),
    )
}

/// A validated key relation from `S` to `T` with its rendered target predicate
/// (anonymous placeholders) and bind values.
struct Relation {
    table: String,
    joins: String,
    predicate: String,
    values: Vec<SqlValue>,
}
impl Relation {
    fn new<B, S, T>(keys: &[(&str, &str)], target: &ReadPredicate) -> async_graphql::Result<Self>
    where
        B: OrmBackend,
        S: Entity + 'static,
        T: Entity + 'static,
    {
        if keys.is_empty() || target.entity != TypeId::of::<T>() || target.backend != B::DIALECT {
            return Err(invalid());
        }
        let mut joins = Vec::new();
        for (source, destination) in keys {
            if !S::columns().iter().any(|c| c.name == *source)
                || !T::columns().iter().any(|c| c.name == *destination)
            {
                return Err(invalid());
            }
            joins.push(format!(
                "__gom_visibility.{} = {}.{}",
                B::DIALECT.quote_identifier(destination),
                B::DIALECT.render_table_identifier_path(S::TABLE_NAME),
                B::DIALECT.quote_identifier(source)
            ));
        }
        let mut values = Vec::new();
        let predicate = render_filter_expression(
            DatabaseBackend::Sqlite,
            &target.expression,
            &mut 1,
            &mut values,
        );
        Ok(Self {
            table: B::DIALECT.render_table_identifier_path(T::TABLE_NAME),
            joins: joins.join(" AND "),
            predicate,
            values,
        })
    }

    /// Correlated EXISTS over the related rows where `predicate` is TRUE.
    fn exists(&self, predicate: &str) -> String {
        format!(
            "EXISTS (SELECT 1 FROM (SELECT * FROM {} WHERE {predicate}) AS __gom_visibility WHERE {})",
            self.table, self.joins
        )
    }
}

impl ReadPredicate {
    /// A predicate over `expression`, measured and checked against the
    /// composition budget.
    fn measured(
        entity: TypeId,
        backend: DatabaseBackend,
        expression: FilterExpression,
    ) -> async_graphql::Result<Self> {
        let cost = Cost::of_expression(&expression).check(backend)?;
        Ok(Self {
            entity,
            backend,
            expression,
            cost,
        })
    }

    /// A predicate over a rendered trusted `clause`, measured and checked.
    fn fragment(
        entity: TypeId,
        backend: DatabaseBackend,
        clause: String,
        values: Vec<SqlValue>,
    ) -> async_graphql::Result<Self> {
        let cost = Cost::of(&clause).check(backend)?;
        Ok(Self {
            entity,
            backend,
            expression: FilterExpression::trusted_fragment(clause, values),
            cost,
        })
    }

    /// Compare a validated persisted physical column with a bound value.
    /// Private and non-filterable columns are accepted; this does not expose
    /// them in GraphQL inputs or grant any field access. Unknown names fail closed.
    pub fn column<B: OrmBackend, T: Entity + 'static>(
        column: &str,
        op: ReadPredicateOp,
        value: SqlValue,
    ) -> async_graphql::Result<Self> {
        validate_column::<T>(column)?;
        let operator = match op {
            ReadPredicateOp::Eq => "=",
            ReadPredicateOp::Ne => "<>",
            ReadPredicateOp::Lt => "<",
            ReadPredicateOp::Lte => "<=",
            ReadPredicateOp::Gt => ">",
            ReadPredicateOp::Gte => ">=",
        };
        Self::fragment(
            TypeId::of::<T>(),
            B::DIALECT,
            format!("{} {operator} ?", B::DIALECT.quote_identifier(column)),
            vec![value],
        )
    }

    /// `column IS NULL` on a validated persisted physical column.
    ///
    /// This is the explicit way to admit (or, with [`Self::not`], to exclude)
    /// rows whose column is NULL. It is two-valued: never UNKNOWN. Private and
    /// non-filterable columns are accepted; unknown names fail closed.
    pub fn is_null<B: OrmBackend, T: Entity + 'static>(
        column: &str,
    ) -> async_graphql::Result<Self> {
        Self::null_test::<B, T>(column, "IS NULL")
    }

    /// `column IS NOT NULL` on a validated persisted physical column.
    ///
    /// Two-valued, like [`Self::is_null`]. Use it where a negated deny list
    /// would be empty: [`Self::not`] refuses to negate an empty `in_list`, and
    /// `is_not_null` states "every row with a value" explicitly.
    pub fn is_not_null<B: OrmBackend, T: Entity + 'static>(
        column: &str,
    ) -> async_graphql::Result<Self> {
        Self::null_test::<B, T>(column, "IS NOT NULL")
    }

    fn null_test<B: OrmBackend, T: Entity + 'static>(
        column: &str,
        test: &str,
    ) -> async_graphql::Result<Self> {
        validate_column::<T>(column)?;
        Self::fragment(
            TypeId::of::<T>(),
            B::DIALECT,
            format!("{} {test}", B::DIALECT.quote_identifier(column)),
            Vec::new(),
        )
    }

    /// SQL negation, rendered as `NOT (...)` with the inner bind values in
    /// their original order.
    ///
    /// Predicates use SQL three-valued logic and only TRUE admits a row.
    /// `not(p)` is TRUE exactly when `p` is FALSE; when `p` is UNKNOWN (for
    /// example `column = ?` on a NULL column) the negation stays UNKNOWN and
    /// does not admit the row. A comparison is FALSE only when it would be
    /// FALSE whatever value a NULL column held, so negation never admits a row
    /// on the strength of a NULL. Double negation is the identity and the rule
    /// holds inside any `and`/`or` tree. Only SQL evaluates policies: reads,
    /// counts, keyset and connection pages, authorized scans, search and
    /// subscription delivery re-reads all render the same clause. To admit
    /// NULL rows, say so explicitly:
    /// `p.not()?.or(ReadPredicate::is_null::<B, T>("column")?)?`.
    ///
    /// Refused with `AUTHORIZATION_MISCONFIGURED`:
    /// - empty inner fragments or conjunctions;
    /// - a predicate that renders the deny fallback `1 = 0` anywhere,
    ///   including inside a relation: an empty `in_list` or `or` list, a
    ///   generated filter that fails validation (for example a list over
    ///   [`crate::graphql::filters::MAX_FILTER_LIST_OPERANDS`] operands), or
    ///   an invalid filter value. Its negation would be a blanket grant. Omit
    ///   the term, or use [`Self::is_not_null`] or
    ///   [`ReadVisibility::Unrestricted`] deliberately.
    ///
    /// Relation EXISTS is two-valued, so `related(..).not()` is plain
    /// `NOT EXISTS`: a source row with a NULL key, or whose related rows all
    /// have an UNKNOWN target predicate, is admitted. For the fail-closed
    /// absence test use [`Self::not_related`].
    #[allow(clippy::should_implement_trait)] // Fallible policy construction.
    pub fn not(self) -> async_graphql::Result<Self> {
        if !negatable(&self.expression) {
            return Err(invalid());
        }
        reserve(self.cost.bytes, 1, 16)?;
        let mut values = Vec::new();
        let clause = render_filter_expression(
            DatabaseBackend::Sqlite,
            &self.expression,
            &mut 1,
            &mut values,
        );
        if contains_match_nothing(&clause) {
            return Err(invalid());
        }
        Self::fragment(self.entity, self.backend, format!("NOT ({clause})"), values)
    }

    /// Reject filters requiring residual Rust matching on this backend. A partial
    /// policy must supply a SQL-only prefilter and retain its row callback.
    pub fn from_filter<B: OrmBackend, F: ReadVisibilityFilter>(
        filter: &F,
    ) -> async_graphql::Result<Self> {
        if F::backend() != B::DIALECT || filter.requires_in_memory_filtering(B::DIALECT) {
            return Err(invalid());
        }
        let expression = filter.to_filter_expression().ok_or_else(invalid)?;
        Self::measured(F::entity_type_id(), B::DIALECT, expression)
    }

    /// A parameterized EXISTS join over validated persisted columns. `keys`
    /// maps source columns to target columns; composite keys are supported.
    /// Target restrictions come from a typed target predicate. This is an
    /// explicit equality relation, not a named relation resolver: include any
    /// discriminator constraints in source/target predicates yourself. Target
    /// entity/row policies are not implicit grants; the policy owns this join.
    pub fn related<B, S, T>(keys: &[(&str, &str)], target: Self) -> async_graphql::Result<Self>
    where
        B: OrmBackend,
        S: Entity + 'static,
        T: Entity + 'static,
    {
        reserve(target.cost.bytes, 1, 1024)?;
        let relation = Relation::new::<B, S, T>(keys, &target)?;
        let clause = relation.exists(&relation.predicate);
        Self::fragment(TypeId::of::<S>(), B::DIALECT, clause, relation.values)
    }

    /// Fail-closed absence: "no related row can match `target`".
    ///
    /// `keys` and `target` are validated exactly as for [`Self::related`], and
    /// as there the policy owns the join: target row policies are not
    /// imported, and a hidden target row counts like any other. The outcome
    /// reveals what `related(keys, target)` reveals plus whether some related
    /// row has `target` UNKNOWN (a hidden one included), and that case fails
    /// closed: it can deny a row, never admit one. The result is
    /// three-valued, so it composes with [`Self::not`], `and` and `or` under
    /// the same rule as every other predicate. For each source row it is:
    ///
    /// - UNKNOWN when any source key column is NULL: the relation cannot be
    ///   evaluated, so the row is not admitted;
    /// - FALSE when a related row has `target` TRUE;
    /// - UNKNOWN when no related row has `target` TRUE but one has it UNKNOWN
    ///   (it might match), so the row is not admitted;
    /// - TRUE otherwise, including when there is no related row at all.
    ///
    /// `not_related(keys, p).not()?` therefore admits the same rows as
    /// `related(keys, p)`. To admit rows with a NULL key, add
    /// `.or(ReadPredicate::is_null::<B, S>("key")?)?` explicitly.
    ///
    /// It renders as `(CASE WHEN <key IS NULL> THEN NULL WHEN EXISTS (<related
    /// rows where target>) THEN 0 WHEN EXISTS (<related rows where target is
    /// not FALSE>) THEN NULL ELSE 1 END) = 1` on SQLite, PostgreSQL and SQL
    /// Server. The target's bind values appear twice, in that order. A target
    /// that renders the deny fallback `1 = 0` (such as an empty `in_list`) is
    /// refused with `AUTHORIZATION_MISCONFIGURED`, as for [`Self::not`].
    pub fn not_related<B, S, T>(keys: &[(&str, &str)], target: Self) -> async_graphql::Result<Self>
    where
        B: OrmBackend,
        S: Entity + 'static,
        T: Entity + 'static,
    {
        if !negatable(&target.expression) {
            return Err(invalid());
        }
        reserve(target.cost.bytes, 2, 2048)?;
        let relation = Relation::new::<B, S, T>(keys, &target)?;
        if relation.predicate.trim().is_empty() || contains_match_nothing(&relation.predicate) {
            return Err(invalid());
        }
        let source = B::DIALECT.render_table_identifier_path(S::TABLE_NAME);
        let null_key = keys
            .iter()
            .map(|(column, _)| format!("{source}.{} IS NULL", B::DIALECT.quote_identifier(column)))
            .collect::<Vec<_>>()
            .join(" OR ");
        let matching = relation.exists(&relation.predicate);
        let possible = relation.exists(&format!(
            "CASE WHEN NOT ({}) THEN 0 ELSE 1 END = 1",
            relation.predicate
        ));
        let mut values = relation.values.clone();
        values.extend(relation.values);
        Self::fragment(
            TypeId::of::<S>(),
            B::DIALECT,
            format!(
                "(CASE WHEN {null_key} THEN NULL WHEN {matching} THEN 0 WHEN {possible} THEN NULL ELSE 1 END) = 1"
            ),
            values,
        )
    }

    /// Existential closure: "some row reachable from this one matches
    /// `target`", following at most [`READ_PREDICATE_CLOSURE_DEFAULT_DEPTH`]
    /// hops.
    ///
    /// The same as [`Self::related_closure_within`] with
    /// [`ReadPredicateQuantifier::Any`] and the default bound. See there for
    /// the walk, the NULL, cycle and depth rules, and visibility.
    pub fn related_closure<B, T>(keys: &[(&str, &str)], target: Self) -> async_graphql::Result<Self>
    where
        B: OrmBackend,
        T: Entity + 'static,
    {
        Self::related_closure_within::<B, T>(
            ReadPredicateQuantifier::Any,
            keys,
            target,
            READ_PREDICATE_CLOSURE_DEFAULT_DEPTH,
        )
    }

    /// Universal closure: "every row reachable from this one matches
    /// `target`", that is "no reachable row fails `target`", following at
    /// most [`READ_PREDICATE_CLOSURE_DEFAULT_DEPTH`] hops.
    ///
    /// The same as [`Self::related_closure_within`] with
    /// [`ReadPredicateQuantifier::All`] and the default bound. It is
    /// fail-closed: an UNKNOWN `target` anywhere in the closure, or a walk
    /// that cannot be completed, never admits the row.
    pub fn related_closure_all<B, T>(
        keys: &[(&str, &str)],
        target: Self,
    ) -> async_graphql::Result<Self>
    where
        B: OrmBackend,
        T: Entity + 'static,
    {
        Self::related_closure_within::<B, T>(
            ReadPredicateQuantifier::All,
            keys,
            target,
            READ_PREDICATE_CLOSURE_DEFAULT_DEPTH,
        )
    }

    /// A transitive-closure relation over one entity `T`, quantified over
    /// the rows it reaches and bounded to `max_depth` hops.
    ///
    /// **The walk.** `keys` maps pointer columns to key columns, in the
    /// `(source, target)` order of [`Self::related`]: one hop goes from a
    /// row to every row whose key columns equal this row's pointer columns.
    /// A parent chain is `&[("parent_id", "id")]`; swapping the pair walks
    /// to descendants instead. Composite keys are supported, and columns are
    /// validated as for `related`. The closure is reflexive: it holds the
    /// row itself (depth 0) and every row reached in 1 to `max_depth` hops.
    /// A pointer reaches every row with an equal key, so with a non-unique
    /// key the walk fans out exactly as `related` does for one hop. Use a
    /// unique key, normally the primary key, for a parent chain.
    ///
    /// **Three-valued result.** Per row, with "complete" defined below:
    ///
    /// - [`ReadPredicateQuantifier::Any`] is TRUE when some reached row has
    ///   `target` TRUE; FALSE when the walk is complete and every reached row
    ///   has `target` FALSE; UNKNOWN otherwise.
    /// - [`ReadPredicateQuantifier::All`] is FALSE when some reached row has
    ///   `target` FALSE; TRUE when the walk is complete and every reached row
    ///   has `target` TRUE; UNKNOWN otherwise.
    ///
    /// Only TRUE admits a row, so an UNKNOWN `target` anywhere in the
    /// closure, or an incomplete walk, never admits under `All` and never
    /// lets a negation admit. The forms are duals: `All(p)` admits exactly
    /// the rows `Any(p.not()?).not()?` admits. Both compose with
    /// [`Self::not`], `and` and `or` under the same rule as every other
    /// predicate. "No reachable row fails `p`" is `All(p)`; `Any(p).not()?`
    /// admits a row only when the walk is complete and no reached row has
    /// `p` TRUE or UNKNOWN.
    ///
    /// **Complete walk.** The walk is complete when every pointer it found
    /// was followed to an existing row. It is incomplete, and the result is
    /// therefore never FALSE for `Any` or TRUE for `All`, when a reached row
    /// has:
    ///
    /// - a partially NULL composite pointer (an ambiguous step);
    /// - a non-NULL pointer that no row's key matches (an unreachable step);
    /// - a non-NULL pointer at depth `max_depth` that leads outside the rows
    ///   already reached (the depth bound cut the walk short).
    ///
    /// A pointer whose columns are all NULL ends the walk at that row: this
    /// is how a root is represented, and it is complete.
    ///
    /// **Cycles** terminate: the walk is bounded by `max_depth`, and it
    /// discards duplicate rows. A cycle, a self-loop included, is complete
    /// once `max_depth` is at least its length, because every pointer on it
    /// then leads back to a reached row.
    ///
    /// **Depth bound.** `max_depth` must be between 1 and
    /// [`READ_PREDICATE_CLOSURE_MAX_DEPTH`], or
    /// [`READ_PREDICATE_CLOSURE_MSSQL_MAX_DEPTH`] on SQL Server; the shorthand
    /// constructors use [`READ_PREDICATE_CLOSURE_DEFAULT_DEPTH`] on every
    /// backend. The work per evaluated row is at most `max_depth` times the
    /// rows the walk can reach. Hitting the
    /// bound makes the walk incomplete: `All` is then never TRUE (it fails
    /// closed), and `Any` is TRUE only if a matching row was found within
    /// the bound, otherwise UNKNOWN, so the row is not admitted and a
    /// negation does not admit it either.
    ///
    /// **Visibility.** As with `related`, the policy owns the walk: no row
    /// policy is imported, and a row hidden by its own policy is reached,
    /// tested and followed like any other. Hop by hop the result reveals
    /// what `related` and [`Self::not_related`] reveal: whether a reached row
    /// matches `target`, and whether one might (UNKNOWN). It also reveals
    /// whether the walk was complete. Those extra facts only ever make the
    /// result UNKNOWN, so they can deny a row, never admit one. Neither form
    /// returns columns of reached rows.
    ///
    /// **Rendering.** A correlated scalar subquery, used wherever the policy
    /// clause is applied: reads, counts, aggregates, keyset and connection
    /// pages, search and subscription delivery re-reads. `target`'s bind
    /// values appear four times, in order. SQLite and PostgreSQL render a
    /// `WITH RECURSIVE` common table expression. SQL Server does not allow a
    /// common table expression inside a subquery, so there the walk is
    /// unrolled into a breadth-first search with one nested derived table and
    /// one correlated `OUTER APPLY` lookup per hop. Each level keeps only the
    /// distinct pointers reached so far, so the work per row stays bounded by
    /// `max_depth` times the distinct pointers reached, whatever the fan-out.
    /// Both forms admit the same rows. No backend evaluates the closure in
    /// memory or for the whole table. Other backends are refused.
    ///
    /// **Composition limits.** A closure inside a closure evaluates the inner
    /// walk for every row the outer walk reaches, and the target is inlined
    /// four times, so cost multiplies with each level. At most
    /// [`READ_PREDICATE_CLOSURE_MAX_NESTING`] closures may nest, through any
    /// relations, negations, `and` and `or`; the rendered SQL may not exceed
    /// [`READ_PREDICATE_MAX_RENDERED_BYTES`]. On SQL Server, the policy's
    /// subqueries may nest at most [`READ_PREDICATE_MSSQL_MAX_NESTING`]
    /// levels. A closure at the default bound uses about 66 levels and each
    /// `related` around it adds two, leaving room for six such relations, or
    /// three around a closure inside a closure.
    ///
    /// Refused with `AUTHORIZATION_MISCONFIGURED`: unsupported backends,
    /// `max_depth` outside the range above, a composition over the limits
    /// above, empty or
    /// unknown key columns, a target for another entity or backend, and an
    /// empty target. A target that renders the deny fallback `1 = 0` is
    /// accepted, as for `related`, because the positive forms only deny with
    /// it; [`Self::not`] refuses to negate such a closure.
    pub fn related_closure_within<B, T>(
        quantifier: ReadPredicateQuantifier,
        keys: &[(&str, &str)],
        target: Self,
        max_depth: u32,
    ) -> async_graphql::Result<Self>
    where
        B: OrmBackend,
        T: Entity + 'static,
    {
        let limit = match B::DIALECT {
            DatabaseBackend::Sqlite | DatabaseBackend::Postgres => READ_PREDICATE_CLOSURE_MAX_DEPTH,
            DatabaseBackend::Mssql => READ_PREDICATE_CLOSURE_MSSQL_MAX_DEPTH,
            _ => 0,
        };
        if !(1..=limit).contains(&max_depth) || !negatable(&target.expression) {
            return Err(invalid());
        }
        // A closure inside closures multiplies the walk per level, and the
        // target is inlined four times; refuse both before rendering.
        if target.cost.closures >= READ_PREDICATE_CLOSURE_MAX_NESTING {
            return Err(invalid());
        }
        let walk = if B::DIALECT == DatabaseBackend::Mssql {
            max_depth as usize * (1024 + 256 * keys.len())
        } else {
            0
        };
        reserve(target.cost.bytes, 4, 4096 + walk)?;
        let relation = Relation::new::<B, T, T>(keys, &target)?;
        if relation.predicate.trim().is_empty() {
            return Err(invalid());
        }
        let dialect = B::DIALECT;
        let table = dialect.render_table_identifier_path(T::TABLE_NAME);
        let cte = format!("__gom_closure_{}", closure_level(&relation.predicate));
        let mut values = Vec::with_capacity(relation.values.len() * 4);
        for _ in 0..4 {
            values.extend(relation.values.iter().cloned());
        }
        if dialect == DatabaseBackend::Mssql {
            let clause = render_mssql_closure(
                dialect,
                &table,
                &cte,
                keys,
                quantifier,
                &relation.predicate,
                max_depth,
            );
            return Self::fragment(TypeId::of::<T>(), B::DIALECT, clause, values);
        }
        let join = |items: Vec<String>, separator: &str| items.join(separator);
        let indexed = |render: &dyn Fn(usize, &str, &str) -> String| {
            keys.iter()
                .enumerate()
                .map(|(index, &(pointer, key))| render(index, pointer, key))
                .collect::<Vec<_>>()
        };
        let pointers = join(
            indexed(&|_, pointer, _| format!("{table}.{}", dialect.quote_identifier(pointer))),
            ", ",
        );
        let columns = join(indexed(&|index, _, _| format!("__gom_p{index}")), ", ");
        let predicate = &relation.predicate;
        // 2 = TRUE, 1 = UNKNOWN, 0 = FALSE for the row in scope as `table`.
        let truth =
            format!("CASE WHEN ({predicate}) THEN 2 WHEN NOT ({predicate}) THEN 0 ELSE 1 END");
        // The anchor has no FROM, so `table` is the row being authorized.
        // In the step, the unaliased join makes `table` the reached row, so
        // the target (including any nested relation) is evaluated on it.
        let step = join(
            indexed(&|index, _, key| {
                format!(
                    "{table}.{} = {cte}.__gom_p{index}",
                    dialect.quote_identifier(key)
                )
            }),
            " AND ",
        );
        let followed = join(
            indexed(&|index, _, _| format!("__gom_node.__gom_p{index} IS NOT NULL")),
            " AND ",
        );
        let ambiguous = if keys.len() > 1 {
            format!(
                "(NOT ({followed}) AND ({})) OR ",
                join(
                    indexed(&|index, _, _| format!("__gom_node.__gom_p{index} IS NOT NULL")),
                    " OR ",
                )
            )
        } else {
            String::new()
        };
        let resolves = join(
            indexed(&|index, _, key| {
                format!(
                    "__gom_target.{} = __gom_node.__gom_p{index}",
                    dialect.quote_identifier(key)
                )
            }),
            " AND ",
        );
        let seen = join(
            indexed(&|index, _, _| {
                format!("__gom_seen.__gom_p{index} = __gom_node.__gom_p{index}")
            }),
            " AND ",
        );
        let incomplete = format!(
            "EXISTS (SELECT 1 FROM {cte} AS __gom_node WHERE {ambiguous}({followed} AND (\
             NOT EXISTS (SELECT 1 FROM {table} AS __gom_target WHERE {resolves}) \
             OR (__gom_node.__gom_depth = {max_depth} AND NOT EXISTS (SELECT 1 FROM {cte} AS __gom_seen \
             WHERE {seen} AND __gom_seen.__gom_depth < {max_depth})))))"
        );
        let verdict = match quantifier {
            ReadPredicateQuantifier::Any => format!(
                "CASE WHEN MAX(__gom_truth) = 2 THEN 1 WHEN MAX(__gom_truth) = 1 OR {incomplete} THEN NULL ELSE 0 END"
            ),
            ReadPredicateQuantifier::All => format!(
                "CASE WHEN MIN(__gom_truth) = 0 THEN 0 WHEN MIN(__gom_truth) = 1 OR {incomplete} THEN NULL ELSE 1 END"
            ),
        };
        let clause = format!(
            "(WITH RECURSIVE {cte} ({columns}, __gom_depth, __gom_truth) AS (\
             SELECT {pointers}, 0, {truth} \
             UNION SELECT {pointers}, {cte}.__gom_depth + 1, {truth} FROM {cte} JOIN {table} ON {step} \
             WHERE {cte}.__gom_depth < {max_depth}) \
             SELECT {verdict} FROM {cte}) = 1"
        );
        Self::fragment(TypeId::of::<T>(), B::DIALECT, clause, values)
    }

    pub fn and(self, other: Self) -> async_graphql::Result<Self> {
        self.combine(other, true)
    }
    pub fn or(self, other: Self) -> async_graphql::Result<Self> {
        self.combine(other, false)
    }
    fn combine(self, other: Self, and: bool) -> async_graphql::Result<Self> {
        if self.entity != other.entity || self.backend != other.backend {
            return Err(invalid());
        }
        // A child of the same kind is flattened into this node, so a fold of
        // any length stays one level deep: `(a) OR (b) OR (c)`. Its rendered
        // depth is its deepest child plus one, plus the grouping levels a long
        // chain is rendered with.
        let mut children = Vec::new();
        let (mut inner, mut inner_units) = (0, 0);
        for (expression, cost) in [(self.expression, self.cost), (other.expression, other.cost)] {
            match expression {
                FilterExpression::And(nested) if and && !nested.is_empty() => {
                    inner = inner.max(cost.inner);
                    inner_units = inner_units.max(cost.inner_units);
                    children.extend(nested);
                }
                FilterExpression::Or(nested) if !and && !nested.is_empty() => {
                    inner = inner.max(cost.inner);
                    inner_units = inner_units.max(cost.inner_units);
                    children.extend(nested);
                }
                expression => {
                    inner = inner.max(cost.depth);
                    inner_units = inner_units.max(cost.units);
                    children.push(expression);
                }
            }
        }
        // Each child is wrapped in plain parentheses, one unit each.
        let levels = 1 + super::query::filter_chain_levels(children.len());
        let depth = inner.saturating_add(levels);
        let units = inner_units.saturating_add(levels);
        let cost = Cost {
            bytes: self
                .cost
                .bytes
                .saturating_add(other.cost.bytes)
                // `(` `)` and ` AND `, plus amortised chunk grouping.
                .saturating_add(8),
            depth,
            inner,
            units,
            inner_units,
            nesting: self.cost.nesting.max(other.cost.nesting),
            closures: self.cost.closures.max(other.cost.closures),
        }
        .check(self.backend)?;
        Ok(Self {
            entity: self.entity,
            backend: self.backend,
            expression: if and {
                FilterExpression::And(children)
            } else {
                FilterExpression::Or(children)
            },
            cost,
        })
    }
}

/// The most rendered policy SQL, in bytes, that the generated relation
/// filters and relation-count orderings of one statement may add together.
/// Each fragment is also checked like a [`ReadPredicate`]; this caps their
/// sum, so a request with many where-input nodes cannot multiply a large
/// policy into one oversized statement. Exceeding it is refused with
/// `AUTHORIZATION_MISCONFIGURED` before the statement is rendered.
pub const READ_PREDICATE_MAX_STATEMENT_RELATION_BYTES: usize = 1 << 20;

/// Per-statement budget threaded through generated where-input and ordering
/// preparation: where-input nodes left, and relation policy SQL added so far.
#[doc(hidden)]
#[derive(Debug)]
pub struct RelationFilterBudget {
    nodes: usize,
    bytes: usize,
}
impl Default for RelationFilterBudget {
    fn default() -> Self {
        Self::new()
    }
}
impl RelationFilterBudget {
    /// The budget for one statement: 128 where-input nodes.
    pub fn new() -> Self {
        Self {
            nodes: 128,
            bytes: 0,
        }
    }
    /// Spend one where-input node; false once none are left.
    pub fn take_node(&mut self) -> bool {
        if self.nodes == 0 {
            return false;
        }
        self.nodes -= 1;
        true
    }
    /// Check a rendered relation fragment and add it to this statement.
    fn charge<B: OrmBackend>(&mut self, fragment: &str) -> async_graphql::Result<()> {
        let cost = Cost::of(fragment).check(B::DIALECT)?;
        self.bytes = self.bytes.saturating_add(cost.bytes);
        if self.bytes > READ_PREDICATE_MAX_STATEMENT_RELATION_BYTES {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Internal authorization bridge for generated static collection filters.
/// Only complete SQL visibility is accepted; callback policies fail closed.
#[doc(hidden)]
pub async fn prepare_static_relation_filter<B, S, T, F>(
    ctx: &async_graphql::Context<'_>,
    keys: &[(&str, &str)],
    alternatives: &[F],
    budget: &mut RelationFilterBudget,
) -> async_graphql::Result<(Vec<String>, Vec<SqlValue>)>
where
    B: OrmBackend,
    S: Entity + 'static,
    T: Entity + super::RelationTraversalAuthorization + 'static,
    F: ReadVisibilityFilter,
{
    if F::entity_type_id() != TypeId::of::<T>() || F::backend() != B::DIALECT {
        return Err(invalid());
    }
    // Related-row predicates and counts traverse the relation: the target's
    // own auth mode and `list` operation scopes apply.
    super::authorize_relation_target::<T>(ctx, true)?;
    let db = ctx.data::<Database<B>>()?;
    let surface = EntityAccessSurface::GraphqlRelation;
    db.ensure_entity_access(
        Some(ctx),
        T::entity_name(),
        T::metadata().read_policy,
        EntityAccessKind::Read,
        surface,
    )
    .await
    .map_err(crate::graphql::errors::graphql_error_from_callback)?;
    let visibility = db
        .read_visibility::<T>(Some(ctx), surface)
        .await
        .map_err(crate::graphql::errors::graphql_error_from_callback)?;
    if visibility.requires_residual_checks() {
        return Err(invalid());
    }
    let mut expressions = Vec::new();
    for filter in alternatives {
        filter.validate().map_err(query_error)?;
        if filter.requires_in_memory_filtering(B::DIALECT) {
            return Err(input_error());
        }
        super::authorize_read_fields::<T, B>(db, ctx, filter.referenced_fields()).await?;
        expressions.push(
            filter.to_filter_expression().unwrap_or_else(|| {
                FilterExpression::trusted_fragment("1 = 1".to_owned(), Vec::new())
            }),
        );
    }
    let mut expression = if expressions.is_empty() {
        FilterExpression::trusted_fragment("1 = 0".to_owned(), Vec::new())
    } else {
        FilterExpression::Or(expressions)
    };
    if let Some(policy) = visibility.predicate_expression() {
        expression = FilterExpression::And(vec![expression, policy.clone()]);
    }
    let predicate = ReadPredicate::related::<B, S, T>(
        keys,
        ReadPredicate::measured(TypeId::of::<T>(), B::DIALECT, expression)?,
    )?;
    let mut values = Vec::new();
    // Keep anonymous placeholders in this composable fragment. EntityQuery
    // rebases them after preceding scalar/policy binds using B::DIALECT.
    // Identifiers inside the EXISTS are already quoted by ReadPredicate::related.
    let clause = render_filter_expression(
        DatabaseBackend::Sqlite,
        &predicate.expression,
        &mut 1,
        &mut values,
    );
    if values.len() > 512 {
        return Err(input_error());
    }
    budget.charge::<B>(&clause)?;
    Ok((vec![clause], values))
}

/// Internal authorization bridge for generated `order_aggregate` count terms.
///
/// Returns the correlated `COUNT(*)` expression (without a direction) over the
/// target rows the request may read, plus its anonymous bind values. The
/// target's entity access is checked first, and its complete SQL row
/// visibility is ANDed into the counted rows. Callback-only and prefilter
/// (residual) policies cannot be expressed in one SQL count and fail closed
/// with the sanitized authorization error, as relation filters do.
///
/// `join` is generated, identifier-quoted SQL that correlates `alias` with the
/// source table; it must not contain bind placeholders.
#[doc(hidden)]
pub async fn prepare_relation_count_order<B, T>(
    ctx: &async_graphql::Context<'_>,
    alias: &str,
    join: &str,
    budget: &mut RelationFilterBudget,
) -> async_graphql::Result<(String, Vec<SqlValue>)>
where
    B: OrmBackend,
    T: Entity + super::RelationTraversalAuthorization + 'static,
{
    // Related-row predicates and counts traverse the relation: the target's
    // own auth mode and `list` operation scopes apply.
    super::authorize_relation_target::<T>(ctx, true)?;
    let db = ctx.data::<Database<B>>()?;
    let surface = EntityAccessSurface::GraphqlRelation;
    db.ensure_entity_access(
        Some(ctx),
        T::entity_name(),
        T::metadata().read_policy,
        EntityAccessKind::Read,
        surface,
    )
    .await
    .map_err(crate::graphql::errors::graphql_error_from_callback)?;
    let visibility = db
        .read_visibility::<T>(Some(ctx), surface)
        .await
        .map_err(crate::graphql::errors::graphql_error_from_callback)?;
    if visibility.requires_residual_checks() {
        return Err(invalid());
    }
    let table = B::DIALECT.render_identifier_path(T::TABLE_NAME);
    let Some(policy) = visibility.predicate_expression() else {
        return Ok((
            format!("(SELECT COUNT(*) FROM {table} AS {alias} WHERE {join})"),
            Vec::new(),
        ));
    };
    let mut values = Vec::new();
    // Anonymous placeholders: the sort renderer renumbers them per dialect
    // after the WHERE binds, as for contextual order expressions.
    let predicate = render_filter_expression(DatabaseBackend::Sqlite, policy, &mut 1, &mut values);
    if predicate.trim().is_empty() {
        return Err(invalid());
    }
    if values.len() > 512 {
        return Err(input_error());
    }
    // The count wraps the policy in two more subquery levels: check the
    // wrapped expression, as for a relation filter.
    reserve(predicate.len(), 1, 1024)?;
    let count = format!(
        "(SELECT COUNT(*) FROM (SELECT * FROM {table} WHERE {predicate}) AS {alias} WHERE {join})"
    );
    budget.charge::<B>(&count)?;
    Ok((count, values))
}

/// The policy's explicit read decision for one request/entity/surface.
#[derive(Clone, Debug)]
pub enum ReadVisibility {
    /// No SQL authorization is available. Every candidate needs a row callback.
    CallbackOnly,
    /// SQL narrows candidates; every remaining candidate still needs a callback.
    Prefilter(ReadPredicate),
    /// SQL is sufficient authorization; row callbacks cannot additionally veto.
    Complete(ReadPredicate),
    /// Explicitly authorize every matching row for this request and entity.
    Unrestricted,
}
impl ReadVisibility {
    pub(crate) fn predicate_expression(&self) -> Option<&FilterExpression> {
        match self {
            Self::Prefilter(predicate) | Self::Complete(predicate) => Some(&predicate.expression),
            _ => None,
        }
    }
    pub fn requires_residual_checks(&self) -> bool {
        matches!(self, Self::CallbackOnly | Self::Prefilter(_))
    }
    pub(crate) fn validate<T: Entity + 'static, B: OrmBackend>(&self) -> async_graphql::Result<()> {
        self.validate_identity(Some(TypeId::of::<T>()), B::DIALECT)
    }
    pub(crate) fn validate_identity(
        &self,
        entity: Option<TypeId>,
        backend: DatabaseBackend,
    ) -> async_graphql::Result<()> {
        if let Self::Prefilter(p) | Self::Complete(p) = self
            && (Some(p.entity) != entity || p.backend != backend)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// What a [`RowPolicy`] declares `read_visibility` can return for one entity.
///
/// Schema registration reads this static declaration because `read_visibility`
/// is asynchronous and request-dependent. It describes the policy; it never
/// grants access or replaces runtime checks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReadVisibilityCapability {
    /// Not declared. This is the default, and keeps generated Keyset roots
    /// registered. [`crate::db::Database::keyset_read_policy_diagnostics`]
    /// reports these roots.
    #[default]
    Unknown,
    /// Every GraphQL request gets `CallbackOnly` or `Prefilter`. Generated
    /// schema builders refuse an exposed Keyset root for such an entity.
    ResidualOnly,
    /// At least some GraphQL requests get `Complete` or `Unrestricted`.
    Complete,
}

/// Why a generated Keyset root was reported by the read-policy check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KeysetReadPolicyReason {
    /// The row policy declares [`ReadVisibilityCapability::ResidualOnly`] for
    /// an entity whose Keyset root is exposed. Refused at registration.
    DeclaredResidualOnly,
    /// The row policy leaves the entity [`ReadVisibilityCapability::Unknown`].
    /// Reported as a diagnostic only.
    UndeclaredCapability,
    /// The entity metadata passed to the check has no entry for the root's
    /// entity. Refused, since the check cannot read its policy key.
    MissingEntityMetadata,
    /// The entity declares a read policy key, no row policy is installed and
    /// the authorization mode is not `LegacyPermissive`, so every read of the
    /// entity defers to `can_read_row`. Refused at registration.
    MissingRowPolicy,
}

/// A generated Keyset root reported by the Keyset read-policy check.
///
/// A GraphQL Keyset request is served only when `RowPolicy::read_visibility`
/// returns `Complete` or `Unrestricted`. See
/// [`crate::db::Database::validate_keyset_read_policies`] and
/// [`crate::db::Database::keyset_read_policy_diagnostics`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeysetReadPolicyError {
    field_name: &'static str,
    entity_name: &'static str,
    policy_key: Option<&'static str>,
    reason: KeysetReadPolicyReason,
}

impl KeysetReadPolicyError {
    /// The reported GraphQL root field.
    pub const fn field_name(&self) -> &'static str {
        self.field_name
    }
    /// The entity the root pages through.
    pub const fn entity_name(&self) -> &'static str {
        self.entity_name
    }
    /// The entity's read policy key, if it declares one.
    pub const fn policy_key(&self) -> Option<&'static str> {
        self.policy_key
    }
    /// Why the root was reported.
    pub const fn reason(&self) -> KeysetReadPolicyReason {
        self.reason
    }
    /// Whether generated schema builders refuse the root for this reason.
    pub const fn refuses_registration(&self) -> bool {
        !matches!(self.reason, KeysetReadPolicyReason::UndeclaredCapability)
    }
}

impl std::fmt::Display for KeysetReadPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let policy = match self.policy_key {
            Some(key) => format!("read policy `{key}`"),
            None => "no read policy key".to_string(),
        };
        let (root, entity) = (self.field_name, self.entity_name);
        const FIXES: &str = "Leave `keyset` out of the entity's `query_shapes` (for example \
             `query_shapes = \"single,list,scan\"` with finite scan bounds) and page with the \
             bounded scan root, or return `ReadVisibilityCapability::Complete` from \
             `RowPolicy::read_visibility_capability` if `read_visibility` returns `Complete` or \
             `Unrestricted` for this entity on some GraphQL requests";
        match self.reason {
            KeysetReadPolicyReason::DeclaredResidualOnly => write!(
                f,
                "generated Keyset root `{root}` for entity `{entity}` ({policy}) is refused: the \
                 installed RowPolicy declares `ReadVisibilityCapability::ResidualOnly` for this \
                 entity, and Keyset pagination serves GraphQL requests only with `Complete` or \
                 `Unrestricted` read visibility. {FIXES}"
            ),
            KeysetReadPolicyReason::UndeclaredCapability => write!(
                f,
                "generated Keyset root `{root}` for entity `{entity}` ({policy}) may be unusable: \
                 the installed RowPolicy does not declare `read_visibility_capability` for this \
                 entity, and Keyset pagination refuses GraphQL requests that get `CallbackOnly` \
                 or `Prefilter` read visibility. Declare `Complete` or `ResidualOnly`. {FIXES}"
            ),
            KeysetReadPolicyReason::MissingRowPolicy => write!(
                f,
                "generated Keyset root `{root}` for entity `{entity}` ({policy}) is refused: no \
                 RowPolicy is installed, so outside `AuthorizationMode::LegacyPermissive` every \
                 read of this entity defers to `can_read_row`, and Keyset pagination serves \
                 GraphQL requests only with `Complete` or `Unrestricted` read visibility. Install \
                 a RowPolicy that declares `ReadVisibilityCapability::Complete` for this entity, \
                 or leave `keyset` out of the entity's `query_shapes`"
            ),
            KeysetReadPolicyReason::MissingEntityMetadata => write!(
                f,
                "generated Keyset root `{root}` names entity `{entity}`, which is missing from the \
                 entity metadata passed to the Keyset read-policy check; pass the metadata of \
                 every entity in the operation catalog"
            ),
        }
    }
}

impl std::error::Error for KeysetReadPolicyError {}

/// Every exposed generated Keyset root the row policy cannot be shown to serve.
pub(crate) fn keyset_read_policy_findings<B: OrmBackend>(
    policy: Option<&dyn RowPolicy<B>>,
    keyed_reads_need_row_policy: bool,
    catalog: &GraphqlOperationCatalog,
    entities: &[&EntityMetadata],
) -> Vec<KeysetReadPolicyError> {
    let mut findings = Vec::new();
    for operation in catalog.exposed_operations() {
        if operation.query_shape() != Some(GeneratedGraphqlQueryShape::Keyset) {
            continue;
        }
        let entity_name = operation.entity_name();
        let finding = |policy_key, reason| KeysetReadPolicyError {
            field_name: operation.field_name(),
            entity_name,
            policy_key,
            reason,
        };
        let Some(entity) = entities.iter().find(|e| e.entity_name == entity_name) else {
            findings.push(finding(None, KeysetReadPolicyReason::MissingEntityMetadata));
            continue;
        };
        // Without a row policy, reads are `Unrestricted` unless a declared
        // key defers them to `can_read_row`.
        let Some(policy) = policy else {
            if keyed_reads_need_row_policy && entity.read_policy.is_some() {
                findings.push(finding(
                    entity.read_policy,
                    KeysetReadPolicyReason::MissingRowPolicy,
                ));
            }
            continue;
        };
        let reason = match policy.read_visibility_capability(entity_name, entity.read_policy) {
            ReadVisibilityCapability::Complete => continue,
            ReadVisibilityCapability::ResidualOnly => KeysetReadPolicyReason::DeclaredResidualOnly,
            ReadVisibilityCapability::Unknown => KeysetReadPolicyReason::UndeclaredCapability,
        };
        findings.push(finding(entity.read_policy, reason));
    }
    findings
}

/// Public reason carried by a refused GraphQL Keyset request.
///
/// A GraphQL Keyset request whose read visibility is `CallbackOnly` or
/// `Prefilter` fails with code `AUTHORIZATION_MISCONFIGURED`, the message
/// [`KEYSET_READ_VISIBILITY_REFUSAL_MESSAGE`] and this value in the `reason`
/// extension. Neither says anything about the policy's contents.
pub const KEYSET_READ_VISIBILITY_REFUSAL_REASON: &str = "KEYSET_REQUIRES_COMPLETE_READ_VISIBILITY";

/// Public message carried by a refused GraphQL Keyset request.
pub const KEYSET_READ_VISIBILITY_REFUSAL_MESSAGE: &str = "keyset pagination is not configured \
    for this root under its read policy; page with a bounded scan instead, or declare complete \
    read visibility for this entity";

fn keyset_read_visibility_refusal() -> async_graphql::Error {
    use async_graphql::ErrorExtensions;
    OrmPublicError::with_message(
        OrmErrorCode::AuthorizationMisconfigured,
        KEYSET_READ_VISIBILITY_REFUSAL_MESSAGE,
    )
    .into_graphql_error()
    .extend_with(|_, extensions| extensions.set("reason", KEYSET_READ_VISIBILITY_REFUSAL_REASON))
}

/// A generated stable keyset order, including its unique final primary key.
/// The values must match the order columns and preserve their scalar types.
pub trait AuthorizedKeysetEntity: Entity {
    const KEYSET_ORDER: &'static [KeysetOrderColumn];
    const KEYSET_FINGERPRINT: &'static str;
    /// Field authority for every configured ordering value carried by a cursor.
    /// Missing metadata fails closed for pagination; generated derives supply it.
    const KEYSET_CURSOR_FIELDS: &'static [KeysetCursorField] = &[];
    fn keyset_values(&self) -> Vec<KeysetValue>;

    /// Persisted scalar fields accepted by generated runtime keyset ordering.
    /// Handwritten implementations retain the configured order only.
    const KEYSET_FIELDS: &'static [KeysetField] = &[];

    /// All primary-key components, in declaration order, for a unique suffix.
    const KEYSET_PRIMARY_COLUMNS: &'static [&'static str] = &[];

    fn keyset_value_for_column(&self, column: &str) -> Option<KeysetValue> {
        Self::KEYSET_ORDER
            .iter()
            .position(|part| part.column == column)
            .and_then(|index| self.keyset_values().get(index).cloned())
    }
}

/// A generated, identifier-quoted scalar column and its policy identity.
#[derive(Clone, Copy, Debug)]
pub struct KeysetField {
    pub column: &'static str,
    pub api_name: &'static str,
    pub read_policy: Option<&'static str>,
}

/// Configured ordering metadata, independent of GraphQL payload selection.
#[derive(Clone, Copy, Debug)]
pub struct KeysetCursorField {
    pub column: &'static str,
    pub api_name: &'static str,
    pub read_policy: Option<&'static str>,
    /// Raw cursor values may be public only for readable, untransformed scalars.
    pub graphql_readable: bool,
}

/// Binds a keyset order fingerprint to what the cursor may be resumed on.
///
/// `entity` is supplied for a configured (compile-time) order, whose
/// fingerprint names columns only, so two entities with the same key columns
/// would otherwise accept one another's cursors. A runtime order already names
/// its entity and passes `None`. A non-empty `filter_identities` appends a
/// SHA-256 of the caller-visible filter; the filter itself is never placed in
/// the cursor. Unfiltered reads keep the unfiltered fingerprint.
///
/// Every generated keyset path uses this one function, so a cursor issued by
/// one path is accepted by another only for the same entity, order and filter.
#[doc(hidden)]
pub fn bind_keyset_fingerprint(
    entity: Option<&str>,
    fingerprint: &str,
    filter_identities: &[String],
) -> String {
    use sha2::{Digest, Sha256};
    let mut bound = match entity {
        Some(entity) => format!("entity-v1:{entity}:{fingerprint}"),
        None => fingerprint.to_string(),
    };
    if !filter_identities.is_empty() {
        bound = format!(
            "{bound}:filter-v1:{:x}",
            Sha256::digest(format!("{filter_identities:?}").as_bytes())
        );
    }
    bound
}

fn runtime_keyset_order<T: AuthorizedKeysetEntity>(
    mut order: Vec<KeysetOrderColumn>,
) -> async_graphql::Result<(Vec<KeysetOrderColumn>, String)> {
    if order.is_empty() || order.len() > T::KEYSET_FIELDS.len() {
        return Err(input_error());
    }
    let mut seen = std::collections::HashSet::new();
    for part in &order {
        if !T::KEYSET_FIELDS
            .iter()
            .any(|field| field.column == part.column)
            || !seen.insert(part.column)
        {
            return Err(input_error());
        }
    }
    if T::KEYSET_PRIMARY_COLUMNS.is_empty() {
        return Err(input_error());
    }
    for column in T::KEYSET_PRIMARY_COLUMNS {
        if !T::KEYSET_FIELDS.iter().any(|field| field.column == *column) {
            return Err(input_error());
        }
        if seen.insert(column) {
            order.push(KeysetOrderColumn::asc(column));
        }
    }
    // Generated identifiers cannot contain separators unescaped: serialize the
    // complete structural order rather than concatenating ambiguous names.
    let fingerprint = serde_json::to_string(
        &order
            .iter()
            .map(|part| {
                (
                    part.column,
                    matches!(part.direction, KeysetDirection::Asc),
                    matches!(part.nulls, KeysetNulls::First),
                )
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|_| input_error())?;
    Ok((
        order,
        format!("runtime-keyset-v1:{}:{fingerprint}", T::entity_name()),
    ))
}

/// Diagnostics for actual fetched database rows, before entity decoding.
/// SQL contains placeholders; parameter values are deliberately omitted.
pub trait ReadQueryObserver: Send + Sync {
    fn on_read(&self, sql: &str, fetched_rows: usize);
}

/// Server-owned scan bounds and cursor encryption key. Scan endpoints are
/// disabled until installed on a Database. Debug output redacts the key.
#[derive(Clone)]
pub struct AuthorizedScanConfig {
    pub batch_size: u32,
    pub max_scanned_rows: u32,
    key: [u8; 32],
}
impl std::fmt::Debug for AuthorizedScanConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizedScanConfig")
            .field("batch_size", &self.batch_size)
            .field("max_scanned_rows", &self.max_scanned_rows)
            .finish_non_exhaustive()
    }
}
impl AuthorizedScanConfig {
    /// Use a secret randomly generated 32-byte key, shared by server instances
    /// that need to accept one another's cursors. Rotating it invalidates cursors.
    pub fn new(key: [u8; 32], batch_size: u32, max_scanned_rows: u32) -> Self {
        Self {
            key,
            batch_size,
            max_scanned_rows,
        }
    }
    fn encode<T: AuthorizedKeysetEntity>(
        &self,
        values: Vec<KeysetValue>,
        filter_identities: &[String],
    ) -> async_graphql::Result<String> {
        let cipher = XChaCha20Poly1305::new((&self.key).into());
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let plaintext = encode_keyset_cursor(
            &bind_keyset_fingerprint(
                None,
                &format!("scan:{}:{}", T::entity_name(), T::KEYSET_FINGERPRINT),
                filter_identities,
            ),
            values,
        );
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| invalid())?;
        let bytes: Vec<u8> = nonce.iter().chain(ciphertext.iter()).copied().collect();
        Ok(format!(
            "goms1.{}",
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ))
    }
    fn decode<T: AuthorizedKeysetEntity>(
        &self,
        cursor: &str,
        filter_identities: &[String],
    ) -> async_graphql::Result<Vec<KeysetValue>> {
        let error = || OrmPublicError::new(OrmErrorCode::CursorInvalid).into_graphql_error();
        let text = cursor.strip_prefix("goms1.").ok_or_else(error)?;
        if text.len() > 32768
            || !text.len().is_multiple_of(2)
            || !text.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(error());
        }
        let bytes = (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|_| error()))
            .collect::<Result<Vec<_>, _>>()?;
        if bytes.len() < 40 {
            return Err(error());
        }
        let cipher = XChaCha20Poly1305::new((&self.key).into());
        let plaintext = cipher
            .decrypt(bytes[..24].into(), &bytes[24..])
            .map_err(|_| error())?;
        let plaintext = std::str::from_utf8(&plaintext).map_err(|_| error())?;
        decode_keyset_cursor(
            plaintext,
            &bind_keyset_fingerprint(
                None,
                &format!("scan:{}:{}", T::entity_name(), T::KEYSET_FINGERPRINT),
                filter_identities,
            ),
            T::KEYSET_ORDER.len(),
        )
        .map_err(|e| e.into_graphql_error())
    }
}

/// The continuation codec and bounds of one bounded scan.
enum ScanCursors<'a> {
    /// `fetch_authorized_scan`: the `goms1` codec of [`AuthorizedScanConfig`].
    Scan(&'a AuthorizedScanConfig),
    /// `fetch_authorized_page`: the context-bound codec.
    Page(&'a AuthorizedPageConfig, &'a AuthorizedPageBinding),
}
impl ScanCursors<'_> {
    /// `(batch_size, max_scanned_rows)`, both non-zero.
    fn bounds(&self) -> (u32, u32) {
        match self {
            Self::Scan(config) => (config.batch_size, config.max_scanned_rows),
            Self::Page(config, _) => (config.batch_size(), config.max_scanned_rows()),
        }
    }
    fn page_fingerprint<T: AuthorizedKeysetEntity>(filter_identities: &[String]) -> String {
        bind_keyset_fingerprint(
            None,
            &format!("page:{}:{}", T::entity_name(), T::KEYSET_FINGERPRINT),
            filter_identities,
        )
    }
    fn decode<T: AuthorizedKeysetEntity>(
        &self,
        cursor: &str,
        filter_identities: &[String],
    ) -> async_graphql::Result<Vec<KeysetValue>> {
        match self {
            Self::Scan(config) => config.decode::<T>(cursor, filter_identities),
            Self::Page(config, binding) => config.open(
                binding,
                T::entity_name(),
                &Self::page_fingerprint::<T>(filter_identities),
                T::KEYSET_ORDER.len(),
                cursor,
            ),
        }
    }
    fn encode<T: AuthorizedKeysetEntity>(
        &self,
        values: Vec<KeysetValue>,
        filter_identities: &[String],
    ) -> async_graphql::Result<String> {
        match self {
            Self::Scan(config) => config.encode::<T>(values, filter_identities),
            Self::Page(config, binding) => config.seal(
                binding,
                T::entity_name(),
                &Self::page_fingerprint::<T>(filter_identities),
                values,
            ),
        }
    }
}

/// Forward scan request. Continuations represent underlying scan positions,
/// never visible offsets. Exact totals are available only for complete policies.
#[derive(Clone, Debug, Default, async_graphql::InputObject)]
pub struct AuthorizedScanInput {
    pub after: Option<String>,
    pub limit: Option<i64>,
    #[graphql(default)]
    pub include_total_count: bool,
}

/// Why a scan stopped. Only Exhausted establishes that no further candidate
/// exists. The other states make no claim that another authorized row exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, async_graphql::Enum)]
pub enum ScanStatus {
    Exhausted,
    PageFull,
    BudgetExhausted,
}

#[derive(Clone, Debug, async_graphql::SimpleObject)]
#[graphql(shareable)]
pub struct ScanPageInfo {
    pub status: ScanStatus,
    /// Resume scanning even if this page returned zero visible rows.
    pub continuation: Option<String>,
    /// Absent unless explicitly requested with fully database-visible policy.
    pub total_count: Option<i64>,
}
#[derive(Clone, Debug)]
pub struct AuthorizedScanPage<T> {
    pub nodes: Vec<T>,
    pub page_info: ScanPageInfo,
}

impl<T, B> EntityQuery<T, B>
where
    B: OrmBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    /// AND an already resolved policy with caller filters. Does not enforce
    /// entity/operation access or execute residual callbacks by itself.
    pub fn with_read_visibility(
        mut self,
        visibility: &ReadVisibility,
    ) -> async_graphql::Result<Self> {
        visibility.validate::<T, B>()?;
        if let ReadVisibility::Complete(p) | ReadVisibility::Prefilter(p) = visibility {
            let mut values = Vec::new();
            // EntityQuery stores neutral placeholders; final rendering numbers
            // them together with the caller's filters and ordering parameters.
            let clause = render_filter_expression(
                DatabaseBackend::Sqlite,
                &p.expression,
                &mut 1,
                &mut values,
            );
            self = self.where_values(&format!("({clause})"), values);
        }
        Ok(self)
    }

    async fn authorize_pagination(
        &self,
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
    ) -> async_graphql::Result<ReadVisibility> {
        let surface = if ctx.is_some() {
            EntityAccessSurface::GraphqlQuery
        } else {
            EntityAccessSurface::Repository
        };
        db.ensure_entity_access(
            ctx,
            T::entity_name(),
            T::metadata().read_policy,
            EntityAccessKind::Read,
            surface,
        )
        .await
        .map_err(crate::graphql::errors::graphql_error_from_callback)?;
        db.read_visibility::<T>(ctx, surface)
            .await
            .map_err(crate::graphql::errors::graphql_error_from_callback)
    }

    async fn authorize_page_fields(
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
        rows: &[T],
    ) -> async_graphql::Result<()> {
        // GraphQL field resolvers retain their existing selection-aware checks.
        if ctx.is_none() {
            for row in rows {
                for field in T::repository_field_policies() {
                    db.ensure_repository_readable_field(
                        None,
                        T::entity_name(),
                        field.api_name,
                        field.read_policy,
                        Some(row),
                    )
                    .await
                    .map_err(|e| e.into_graphql_error())?;
                }
            }
        }
        Ok(())
    }
}

impl<T, B> EntityQuery<T, B>
where
    B: OrmBackend,
    T: AuthorizedKeysetEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    async fn authorize_keyset_order_fields(
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
        order: &[KeysetOrderColumn],
        runtime: bool,
        row: Option<&T>,
    ) -> async_graphql::Result<()> {
        for part in order {
            let (api_name, policy, graphql_readable) = if runtime {
                let field = T::KEYSET_FIELDS
                    .iter()
                    .find(|field| field.column == part.column)
                    .ok_or_else(invalid)?;
                (field.api_name, field.read_policy, true)
            } else {
                let field = T::KEYSET_CURSOR_FIELDS
                    .iter()
                    .find(|field| field.column == part.column)
                    .ok_or_else(invalid)?;
                (field.api_name, field.read_policy, field.graphql_readable)
            };
            let record = row.map(|row| row as &(dyn std::any::Any + Send + Sync));
            if let Some(ctx) = ctx {
                if !graphql_readable {
                    return Err(OrmPublicError::forbidden().into_graphql_error());
                }
                db.ensure_readable_field(ctx, T::entity_name(), api_name, policy, record)
                    .await
                    .map_err(crate::graphql::errors::graphql_error_from_callback)?;
            } else {
                db.ensure_repository_readable_field(
                    None,
                    T::entity_name(),
                    api_name,
                    policy,
                    record,
                )
                .await
                .map_err(|error| error.into_graphql_error())?;
            }
        }
        Ok(())
    }

    fn keyset_bound(
        self,
        values: &[KeysetValue],
        before: bool,
        inclusive: bool,
    ) -> async_graphql::Result<Self> {
        self.keyset_bound_with_order(T::KEYSET_ORDER, values, before, inclusive)
    }

    fn keyset_bound_with_order(
        mut self,
        order: &[KeysetOrderColumn],
        values: &[KeysetValue],
        before: bool,
        inclusive: bool,
    ) -> async_graphql::Result<Self> {
        let (mut clause, mut binds) = if before {
            render_keyset_before(B::DIALECT, order, values, self.values.len() + 1)
        } else {
            render_keyset_after(B::DIALECT, order, values, self.values.len() + 1)
        }
        .map_err(|e| e.into_graphql_error())?;
        if inclusive {
            let mut equal = Vec::new();
            for (column, value) in order.iter().zip(values) {
                if matches!(value, KeysetValue::Null) {
                    equal.push(format!("{} IS NULL", column.column));
                } else {
                    equal.push(format!(
                        "{} = {}",
                        column.column,
                        B::DIALECT.placeholder(self.values.len() + binds.len() + 1)
                    ));
                    binds.push(super::keyset_sql_value(value).ok_or_else(input_error)?);
                }
            }
            clause = format!("({clause}) OR ({})", equal.join(" AND "));
        }
        self = self.where_values(&clause, binds);
        Ok(self)
    }
    async fn bounded_rows(
        &self,
        db: &Database<B>,
        auth: Option<&DbAuthContext>,
        limit: i64,
    ) -> async_graphql::Result<Vec<T>> {
        self.clone()
            .paginate(&PageInput {
                limit: Some(limit),
                offset: Some(0),
            })
            .fetch_all_with_auth_and_pagination(db, auth, PaginationConfig::unbounded())
            .await
            .map_err(query_error)
    }

    async fn check_repository_candidates(
        db: &Database<B>,
        visibility: &ReadVisibility,
        rows: &[T],
    ) -> async_graphql::Result<()> {
        if visibility.requires_residual_checks() {
            for row in rows {
                if !db
                    .can_read_row(
                        None,
                        T::entity_name(),
                        T::metadata().read_policy,
                        EntityAccessSurface::Repository,
                        row,
                    )
                    .await
                    .map_err(crate::graphql::errors::graphql_error_from_callback)?
                {
                    return Err(OrmPublicError::forbidden().into_graphql_error());
                }
            }
        }
        Ok(())
    }

    /// A forward keyset connection authorized before LIMIT, count, and probes.
    /// Residual policies must use `fetch_authorized_scan` or legacy exact offsets.
    pub async fn fetch_authorized_keyset(
        self,
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
        auth: Option<&DbAuthContext>,
        page: KeysetPageInput,
    ) -> async_graphql::Result<Connection<T>> {
        self.fetch_authorized_keyset_connection(
            db,
            ctx,
            auth,
            KeysetConnectionInput {
                after: page.after,
                first: page.limit,
                include_total_count: page.include_total_count,
                ..Default::default()
            },
        )
        .await
    }

    /// Bidirectional version of `fetch_authorized_keyset`, with authorized probes
    /// in both directions and counts disabled unless explicitly requested.
    pub async fn fetch_authorized_keyset_connection(
        self,
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
        auth: Option<&DbAuthContext>,
        page: KeysetConnectionInput,
    ) -> async_graphql::Result<Connection<T>> {
        self.fetch_authorized_keyset_connection_with_order(db, ctx, auth, page, None)
            .await
    }

    /// Fetch a forward page using a generated scalar order. The complete primary
    /// key is appended and every order field is authorized before database I/O.
    pub async fn fetch_authorized_keyset_ordered(
        self,
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
        auth: Option<&DbAuthContext>,
        page: KeysetPageInput,
        order: Option<Vec<KeysetOrderColumn>>,
    ) -> async_graphql::Result<Connection<T>> {
        self.fetch_authorized_keyset_connection_with_order(
            db,
            ctx,
            auth,
            KeysetConnectionInput {
                after: page.after,
                first: page.limit,
                include_total_count: page.include_total_count,
                ..Default::default()
            },
            order,
        )
        .await
    }

    /// Bidirectional counterpart with an exact order-bound cursor. Omission
    /// preserves the configured order and its existing cursor encoding.
    pub async fn fetch_authorized_keyset_connection_with_order(
        self,
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
        auth: Option<&DbAuthContext>,
        page: KeysetConnectionInput,
        order: Option<Vec<KeysetOrderColumn>>,
    ) -> async_graphql::Result<Connection<T>> {
        let runtime = order.map(runtime_keyset_order::<T>).transpose()?;
        let (order, fingerprint) = runtime
            .as_ref()
            .map(|(order, fingerprint)| (order.as_slice(), fingerprint.as_str()))
            .unwrap_or((T::KEYSET_ORDER, T::KEYSET_FINGERPRINT));
        let values_for = |node: &T| -> async_graphql::Result<Vec<KeysetValue>> {
            if runtime.is_none() {
                return Ok(node.keyset_values());
            }
            order
                .iter()
                .map(|part| {
                    node.keyset_value_for_column(part.column)
                        .ok_or_else(input_error)
                })
                .collect()
        };
        // Only a size the caller asked for can be refused as oversized; an
        // omitted one takes the default and is reduced to the maximum.
        let requested_limit = page.first.or(page.last);
        let page = page
            .validate(PaginationConfig::DEFAULT_LIMIT, i64::MAX)
            .map_err(|e| e.into_graphql_error())?;
        let backward = page.direction == KeysetWindowDirection::Backward;
        let visibility = self.authorize_pagination(db, ctx).await?;
        Self::authorize_keyset_order_fields(db, ctx, order, runtime.is_some(), None).await?;
        if visibility.requires_residual_checks() {
            // A GraphQL Keyset root cannot advance through rows a callback rejects.
            if ctx.is_some() {
                return Err(keyset_read_visibility_refusal());
            }
            if page.include_total_count {
                return Err(invalid());
            }
        }
        if self.requires_in_memory_filtering() {
            return Err(input_error());
        }
        let mut base = self.with_read_visibility(&visibility)?;
        base.page = None;
        base.order_clauses = order.iter().map(|c| c.order_sql()).collect();
        base.order_values.clear();
        // A per-read maximum set with `with_max_limit` replaces the
        // database's for this page, as it does for `fetch_all`.
        let page_config = db
            .client_pagination_config()
            .with_read_maximum(base.max_limit);
        page_config
            .admit_client_limit(requested_limit)
            .map_err(|error| error.into_graphql_error())?;
        let limit = page_config
            .resolve_page(
                Some(&PageInput {
                    limit: Some(page.limit),
                    offset: Some(0),
                }),
                true,
            )
            .limit
            .unwrap_or(PaginationConfig::DEFAULT_LIMIT)
            .max(1);
        let fingerprint = bind_keyset_fingerprint(
            runtime.is_none().then(T::entity_name),
            fingerprint,
            &base.keyset_filter_identities,
        );
        let fingerprint = fingerprint.as_str();
        let after = page
            .cursor
            .as_deref()
            .map(|cursor| {
                decode_keyset_cursor(cursor, fingerprint, order.len())
                    .map_err(|e| e.into_graphql_error())
            })
            .transpose()?;
        let total_count = if page.include_total_count {
            Some(base.count_with_auth(db, auth).await.map_err(query_error)?)
        } else {
            None
        };
        let mut query = if let Some(values) = &after {
            base.clone()
                .keyset_bound_with_order(order, values, backward, false)?
        } else {
            base.clone()
        };
        if backward {
            query.order_clauses = order.iter().map(|c| c.reversed().order_sql()).collect();
        }
        let mut nodes = query
            .bounded_rows(db, auth, limit.saturating_add(1))
            .await?;
        Self::check_repository_candidates(db, &visibility, &nodes).await?;
        let has_more = nodes.len() > limit as usize;
        nodes.truncate(limit as usize);
        let opposite = if let Some(values) = nodes.first().map(&values_for).transpose()?.or(after) {
            let probe = base
                .clone()
                .keyset_bound_with_order(order, &values, !backward, nodes.is_empty())?
                .bounded_rows(db, auth, 1)
                .await?;
            Self::check_repository_candidates(db, &visibility, &probe).await?;
            !probe.is_empty()
        } else {
            false
        };
        if backward {
            nodes.reverse();
        }
        let (has_next_page, has_previous_page) = if backward {
            (opposite, has_more)
        } else {
            (has_more, opposite)
        };
        Self::authorize_page_fields(db, ctx, &nodes).await?;
        for node in &nodes {
            Self::authorize_keyset_order_fields(db, ctx, order, runtime.is_some(), Some(node))
                .await?;
        }
        Self::authorize_keyset_order_fields(db, ctx, order, runtime.is_some(), None).await?;
        let edges = nodes
            .into_iter()
            .map(|node| {
                Ok(Edge {
                    cursor: encode_keyset_cursor(fingerprint, values_for(&node)?),
                    node,
                })
            })
            .collect::<async_graphql::Result<Vec<_>>>()?;
        Ok(Connection {
            page_info: PageInfo {
                has_next_page,
                has_previous_page,
                start_cursor: edges.first().map(|e| e.cursor.clone()),
                end_cursor: edges.last().map(|e| e.cursor.clone()),
                total_count,
            },
            edges,
        })
    }

    /// Scan bounded database batches, advancing past every examined candidate.
    /// Empty visible batches do not establish exhaustion. No exact callback
    /// count is attempted; requesting one fails before fetching rows.
    pub async fn fetch_authorized_scan(
        self,
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
        auth: Option<&DbAuthContext>,
        page: AuthorizedScanInput,
    ) -> async_graphql::Result<AuthorizedScanPage<T>> {
        let visibility = self.authorize_pagination(db, ctx).await?;
        Self::authorize_keyset_order_fields(db, ctx, T::KEYSET_ORDER, false, None).await?;
        let config = db.authorized_scan_config().ok_or_else(invalid)?;
        if config.batch_size == 0 || config.max_scanned_rows == 0 {
            return Err(invalid());
        }
        if self.requires_in_memory_filtering() || page.limit.is_some_and(|n| n <= 0) {
            return Err(input_error());
        }
        if page.include_total_count && visibility.requires_residual_checks() {
            return Err(input_error());
        }
        let limit = db
            .client_pagination_config()
            .with_read_maximum(self.max_limit)
            .resolve_client_page(
                Some(&PageInput {
                    limit: page.limit,
                    offset: Some(0),
                }),
                true,
            )
            .map_err(|error| error.into_graphql_error())?
            .limit
            .unwrap_or(PaginationConfig::DEFAULT_LIMIT)
            .max(1) as usize;
        self.scan_authorized_candidates(
            db,
            ctx,
            auth,
            &visibility,
            ScanCursors::Scan(config),
            page.after.as_deref(),
            limit,
            page.include_total_count,
        )
        .await
    }

    /// Fetch one context-bound authorized page for a handwritten resolver or
    /// query builder.
    ///
    /// This is the bounded scan of [`fetch_authorized_scan`](Self::fetch_authorized_scan)
    /// with a continuation that is sealed by the ORM under the service-owned
    /// keys of [`AuthorizedPageConfig`] and accepted only under the
    /// [`AuthorizedPageBinding`] it was issued under. Entity access, read
    /// visibility, row callbacks and field policies are resolved again on
    /// every request, exactly as for a first page: a continuation is a
    /// position, never an authorization. `page_info.status` keeps the
    /// [`ScanStatus`] meanings, and `page_info.total_count` is always absent.
    ///
    /// A continuation issued under a different binding, entity, order, filter
    /// or key, or one that was altered, truncated, is oversized or has
    /// expired, is refused with `CURSOR_INVALID` before any row is read. A
    /// `limit` above `AuthorizedPageConfig::max_page_size` is refused with
    /// `PAGE_LIMIT_EXCEEDED`. Without an installed configuration the call
    /// fails closed with `AUTHORIZATION_MISCONFIGURED`.
    pub async fn fetch_authorized_page(
        self,
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
        auth: Option<&DbAuthContext>,
        request: AuthorizedPageRequest,
    ) -> async_graphql::Result<AuthorizedScanPage<T>> {
        let visibility = self.authorize_pagination(db, ctx).await?;
        Self::authorize_keyset_order_fields(db, ctx, T::KEYSET_ORDER, false, None).await?;
        let config = db.authorized_page_config().ok_or_else(invalid)?;
        if self.requires_in_memory_filtering() {
            return Err(input_error());
        }
        let limit = config.resolve_limit(request.limit)?;
        self.scan_authorized_candidates(
            db,
            ctx,
            auth,
            &visibility,
            ScanCursors::Page(config, &request.binding),
            request.after.as_deref(),
            limit,
            false,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn scan_authorized_candidates(
        self,
        db: &Database<B>,
        ctx: Option<&async_graphql::Context<'_>>,
        auth: Option<&DbAuthContext>,
        visibility: &ReadVisibility,
        cursors: ScanCursors<'_>,
        after: Option<&str>,
        limit: usize,
        include_total_count: bool,
    ) -> async_graphql::Result<AuthorizedScanPage<T>> {
        let (batch_size, max_scanned_rows) = cursors.bounds();
        let mut base = self.with_read_visibility(visibility)?;
        base.page = None;
        base.order_clauses = T::KEYSET_ORDER.iter().map(|c| c.order_sql()).collect();
        base.order_values.clear();
        // The continuation is authenticated before any count or row is read.
        let mut position = after
            .map(|c| cursors.decode::<T>(c, &base.keyset_filter_identities))
            .transpose()?;
        let total_count = if include_total_count {
            Some(base.count_with_auth(db, auth).await.map_err(query_error)?)
        } else {
            None
        };
        let mut nodes = Vec::new();
        let mut scanned = 0;
        let status = 'scan: loop {
            let batch_size = batch_size
                .min(max_scanned_rows - scanned)
                .min((limit - nodes.len()).min(u32::MAX as usize) as u32);
            let query = if let Some(values) = &position {
                base.clone().keyset_bound(values, false, false)?
            } else {
                base.clone()
            };
            let rows = query.bounded_rows(db, auth, i64::from(batch_size)).await?;
            let short = rows.len() < batch_size as usize;
            for row in rows {
                position = Some(row.keyset_values());
                scanned += 1;
                if !visibility.requires_residual_checks()
                    || db
                        .can_read_row(
                            ctx,
                            T::entity_name(),
                            T::metadata().read_policy,
                            if ctx.is_some() {
                                EntityAccessSurface::GraphqlQuery
                            } else {
                                EntityAccessSurface::Repository
                            },
                            &row,
                        )
                        .await
                        .map_err(crate::graphql::errors::graphql_error_from_callback)?
                {
                    nodes.push(row);
                }
            }
            if short {
                break 'scan ScanStatus::Exhausted;
            }
            if nodes.len() == limit {
                break 'scan ScanStatus::PageFull;
            }
            if scanned == max_scanned_rows {
                break 'scan ScanStatus::BudgetExhausted;
            }
        };
        Self::authorize_page_fields(db, ctx, &nodes).await?;
        for node in &nodes {
            Self::authorize_keyset_order_fields(db, ctx, T::KEYSET_ORDER, false, Some(node))
                .await?;
        }
        Self::authorize_keyset_order_fields(db, ctx, T::KEYSET_ORDER, false, None).await?;
        let continuation = if status == ScanStatus::Exhausted {
            None
        } else {
            Some(cursors.encode::<T>(
                position.ok_or_else(invalid)?,
                &base.keyset_filter_identities,
            )?)
        };
        Ok(AuthorizedScanPage {
            nodes,
            page_info: ScanPageInfo {
                status,
                continuation,
                total_count,
            },
        })
    }
}

#[cfg(test)]
mod predicate_tests {
    use super::*;

    #[test]
    fn negation_refuses_empty_inner_shapes() {
        for expression in [
            FilterExpression::And(vec![]),
            FilterExpression::trusted_fragment("", vec![]),
            FilterExpression::Or(vec![FilterExpression::trusted_fragment("", vec![])]),
        ] {
            let predicate = ReadPredicate {
                entity: TypeId::of::<()>(),
                backend: DatabaseBackend::Sqlite,
                expression,
                cost: Cost::default(),
            };
            assert!(predicate.not().is_err());
        }
    }

    fn sqlite_fragment(clause: &str, values: Vec<SqlValue>) -> ReadPredicate {
        ReadPredicate {
            entity: TypeId::of::<()>(),
            backend: DatabaseBackend::Sqlite,
            expression: FilterExpression::trusted_fragment(clause, values),
            cost: Cost::of(clause),
        }
    }

    #[test]
    fn folds_stay_flat_and_deep_trees_are_refused_within_a_small_stack() {
        // Build, render, format, clone, compare and drop on an explicit 2 MiB
        // stack, the size of a default tokio worker or test thread.
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(|| {
                let render = |predicate: &ReadPredicate| {
                    render_filter_expression(
                        DatabaseBackend::Sqlite,
                        &predicate.expression,
                        &mut 1,
                        &mut Vec::new(),
                    )
                };
                let leaf = || sqlite_fragment("x = ?", vec![SqlValue::Int(1)]);
                // A 50,000-term `or` fold is one node, rendered in chunks.
                let mut wide = leaf();
                for _ in 0..50_000 {
                    wide = wide.or(leaf()).unwrap();
                }
                assert!(
                    matches!(&wide.expression, FilterExpression::Or(children) if children.len() == 50_001)
                );
                let clause = render(&wide);
                assert_eq!(wide.cost.depth, Cost::of(&clause).depth);
                assert_eq!(wide.cost.units, Cost::of(&clause).units);
                assert_eq!(wide.cost.depth, 1 + super::super::query::filter_chain_levels(50_001));
                assert!(!format!("{wide:?}").is_empty());
                let copy = wide.clone();
                assert!(copy.expression == wide.expression);
                drop((copy, wide));
                // Alternating `and`/`or` and `not` chains are refused at the
                // depth limit; the deepest that builds is measured exactly.
                for alternate in [true, false] {
                    let (mut deepest, mut levels) = (leaf(), 0);
                    loop {
                        let next = if !alternate {
                            deepest.clone().not()
                        } else if levels % 2 == 0 {
                            deepest.clone().and(leaf())
                        } else {
                            deepest.clone().or(leaf())
                        };
                        match next {
                            Ok(next) => {
                                deepest = next;
                                levels += 1;
                            }
                            Err(_) => break,
                        }
                    }
                    assert_eq!(levels, READ_PREDICATE_MAX_EXPRESSION_DEPTH);
                    let clause = render(&deepest);
                    assert_eq!(Cost::of(&clause).depth, READ_PREDICATE_MAX_EXPRESSION_DEPTH);
                    assert_eq!(deepest.cost.depth, Cost::of(&clause).depth);
                    assert!(!format!("{deepest:?}").is_empty());
                    let copy = deepest.clone();
                    assert!(copy.expression == deepest.expression);
                    drop((copy, deepest));
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn relation_policy_sql_is_budgeted_per_statement() {
        let fragment = "x".repeat(READ_PREDICATE_MAX_STATEMENT_RELATION_BYTES / 3);
        let mut budget = RelationFilterBudget::new();
        for _ in 0..3 {
            budget
                .charge::<crate::graphql::orm::DefaultBackend>(&fragment)
                .unwrap();
        }
        assert!(
            budget
                .charge::<crate::graphql::orm::DefaultBackend>(&fragment)
                .is_err()
        );
        let mut nodes = RelationFilterBudget::new();
        assert_eq!((0..200).filter(|_| nodes.take_node()).count(), 128);
    }

    #[test]
    fn composition_cost_counts_subqueries_closures_and_placeholders() {
        // Parentheses in quoted names and plain groups are not subqueries.
        let clause = "EXISTS (SELECT 1 FROM (SELECT * FROM [a(b] WHERE (x = ?)) AS \"(SELECT\" \
                      WHERE 'it''s (SELECT' = (SELECT 2))";
        let cost = Cost::of(clause);
        assert_eq!(cost.nesting, 2);
        // Three parentheses deep; SQL Server units 11 + 11 + 1.
        assert_eq!((cost.depth, cost.units), (3, 23));
        assert_eq!(Cost::of("NOT (NOT (x)) AND KNOT (y)").units, 4);
        assert_eq!(cost.closures, 0);
        assert_eq!(cost.bytes, clause.len() + 6);
        assert_eq!(Cost::of("(WITH c AS (SELECT 1) SELECT 1)").nesting, 2);
        assert_eq!(Cost::of("__gom_closure_2_l0 __gom_closure_1").closures, 2);
        // Only SQL Server has a nesting budget; the others refuse by size.
        let deep = Cost {
            nesting: READ_PREDICATE_MSSQL_MAX_NESTING + 1,
            ..Cost::default()
        };
        assert!(deep.check(DatabaseBackend::Mssql).is_err());
        assert!(deep.check(DatabaseBackend::Postgres).is_ok());
        let weighted = Cost {
            units: READ_PREDICATE_MSSQL_MAX_NESTING_UNITS + 1,
            ..Cost::default()
        };
        assert!(weighted.check(DatabaseBackend::Mssql).is_err());
        assert!(weighted.check(DatabaseBackend::Sqlite).is_ok());
        let nested = Cost {
            depth: READ_PREDICATE_MAX_EXPRESSION_DEPTH + 1,
            ..Cost::default()
        };
        assert!(nested.check(DatabaseBackend::Postgres).is_err());
        let large = Cost {
            bytes: READ_PREDICATE_MAX_RENDERED_BYTES + 1,
            ..Cost::default()
        };
        assert!(large.check(DatabaseBackend::Sqlite).is_err());
        assert!(reserve(READ_PREDICATE_MAX_RENDERED_BYTES / 4, 4, 1).is_err());
        assert!(reserve(usize::MAX, 4, 0).is_err());
    }

    #[test]
    fn negation_refuses_match_nothing_terms() {
        // An empty `in_list`, an empty disjunction or another match-nothing
        // term would negate to a blanket grant.
        for expression in [
            FilterExpression::Or(vec![]),
            FilterExpression::trusted_fragment("1 = 0", vec![]),
            FilterExpression::trusted_fragment(
                "(\"rank\" = ?) AND (1 = 0)",
                vec![SqlValue::Int(1)],
            ),
            FilterExpression::trusted_fragment(
                "(1 = 0) /* spatial unsupported on mssql */",
                vec![],
            ),
            FilterExpression::trusted_fragment(
                "EXISTS (SELECT 1 FROM (SELECT * FROM \"t\" WHERE 1=0) AS v WHERE v.\"k\" = \"s\".\"k\")",
                vec![],
            ),
            FilterExpression::And(vec![
                FilterExpression::trusted_fragment("\"rank\" = ?", vec![SqlValue::Int(1)]),
                FilterExpression::Or(vec![]),
            ]),
        ] {
            let predicate = ReadPredicate {
                entity: TypeId::of::<()>(),
                backend: DatabaseBackend::Sqlite,
                expression,
                cost: Cost::default(),
            };
            assert!(predicate.clone().not().is_err(), "{predicate:?}");
            assert!(predicate.not().is_err());
        }
        for clause in ["\"rank\" = ?", "\"col1\" = 10", "\"x1\" = 0", "\"a\" <> ?"] {
            assert!(sqlite_fragment(clause, vec![]).not().is_ok(), "{clause}");
        }
    }
}
