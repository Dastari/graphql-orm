//! Request-local database visibility and bounded residual authorization scans.
use super::dialect::{SqlToken, SqlTokenKind, scan_sql};
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

    /// A hand-written filter's expression for a read predicate.
    /// [`ReadPredicate::from_filter`], generated related-row filters and
    /// relation-count orderings use it for every filter that is not
    /// generated. Generated filters do not use this method: they render
    /// each column for the row they are given, with no row name in their
    /// SQL text (#796).
    ///
    /// **Threat model.** Hand-written policy SQL is trusted host code. The
    /// ORM guarantees that every clause it accepts is evaluated against
    /// exactly the row it governs and cannot change how it composes with
    /// other policies. It refuses, with `AUTHORIZATION_MISCONFIGURED`, any
    /// construct its validator cannot classify with certainty. SQL that is
    /// deliberately obfuscated to defeat that classification is outside the
    /// guarantee. Known residuals:
    ///
    /// - SQLite reads the bare words `true` and `false` as a column of
    ///   that name when one is visible: in the governed table or, inside a
    ///   subquery, in an enclosing one. Do not write the bare literals in a
    ///   policy that such a table can reach; compare with `1` and `0`.
    /// - SQL Server compares identifiers with the catalog's collation. The
    ///   reserved-name check folds ASCII case only, so a table whose name
    ///   equals a generated alias only under a wider collation equivalence
    ///   (accent-insensitive, as `__góm_visibility_1`, or
    ///   width-insensitive) is not recognised as reserved. Do not name a
    ///   table that way.
    /// - A filter that overrides the macro-only `__gom_row_bound_filter`
    ///   hook is rendered as generated code and is not validated.
    ///
    /// **What a hand-written filter may hold.** Each clause must be a
    /// single-row boolean expression over the entity's own columns and
    /// bound parameters. Each clause is validated on its own, before it is
    /// combined with anything. A filter is refused with
    /// `AUTHORIZATION_MISCONFIGURED` when a clause holds, outside string
    /// constants:
    ///
    /// - `SELECT`, `FROM`, `JOIN`, `WITH`, `UNION`, `INTERSECT`, `EXCEPT`,
    ///   `VALUES` or `TABLE`. A policy that reads another table is built
    ///   with [`ReadPredicate::related`], [`ReadPredicate::not_related`] or
    ///   the closure constructors, which bind every row structurally;
    /// - an unmatched parenthesis or bracket, or a semicolon, so a clause
    ///   can close neither its own group nor the one it is composed into;
    /// - a comment, a PostgreSQL `U&'...'` string or `U&"..."` identifier,
    ///   two string constants separated only by whitespace, or a string
    ///   constant beside a dot;
    /// - a delimited identifier that names the row qualifier or a
    ///   generated alias (`"__gom_self"`, `"__gom_row_1"`, ...);
    /// - a byte-order mark or other non-ASCII whitespace;
    /// - on SQL Server, a `#` name;
    /// - on SQLite, `rowid`, `_rowid_` or `oid`, and `IN` over anything but
    ///   a parenthesised list (a table, or a function SQLite could read as
    ///   table-valued). This is conservative: a call that opens the list is
    ///   accepted only for common scalar functions, so a valid one such as
    ///   `x IN (json_extract(...), 2)` is refused. Put a constant or
    ///   parameter first, or wrap the call in parentheses:
    ///   `x IN ((json_extract(...)), 2)`;
    /// - a numbered or named placeholder (`?1`, `$1`, `@p1`, `:name`,
    ///   `@name`, `$name`), or a clause whose anonymous `?` placeholders
    ///   and bind values differ in number. Only `?` composes: the statement
    ///   numbers placeholders in order across every predicate in it. The
    ///   conditions of [`DatabaseFilter::to_sql_conditions`] share one flat
    ///   list of values, so their placeholders in total must equal its
    ///   length: none is dropped and none is missing;
    /// - the bare word `__gom_self` with no dot after it, or an unclosed
    ///   string, identifier or dollar quote.
    ///
    /// **Binding its columns.** Write each column as `__gom_self.<column>`
    /// ([`READ_PREDICATE_ROW`], a bare word followed by a dot) and return
    /// the entity's table from [`Self::table_name`]. The qualifier is then
    /// bound to the row the predicate governs wherever it is rendered: the
    /// entity's table at the root of a statement, and a row alias that no
    /// table or alias in scope has inside a relation, `not_related`,
    /// closure or count subquery. The qualifier without a table is refused.
    ///
    /// The default returns [`DatabaseFilter::to_filter_expression`], whose
    /// columns are unqualified. Such a filter, and any filter that names a
    /// column other than through the qualifier, is accepted only for the
    /// root of a statement, where no enclosing row exists. It is refused
    /// with `AUTHORIZATION_MISCONFIGURED` wherever its predicate would be
    /// evaluated in a correlated subquery: as a relation, `not_related` or
    /// closure target, and as the policy of an entity that a generated
    /// related-row filter or relation-count ordering targets. There a
    /// column the table lacks would resolve against an outer row. A word is
    /// read as a keyword only where its dialect reads it as one: `LIKE`,
    /// `IN`, `BETWEEN`, `ESCAPE`, `COLLATE` and `END` after an operand,
    /// `ILIKE`, `SIMILAR TO` and `IS [NOT] UNKNOWN` on PostgreSQL, `GLOB`
    /// and `REGEXP` on SQLite. Anywhere else such a word counts as a column
    /// (`unknown = 0` names a column on SQLite). Numbers,
    /// bound parameters, function calls, `NULL`, and the dialect's own
    /// constants (`TRUE`/`FALSE` except on SQL Server, `CURRENT_TIMESTAMP`,
    /// and `CURRENT_DATE`, `CURRENT_TIME`, `CURRENT_USER`, `LOCALTIMESTAMP`
    /// where the dialect has them) are not columns.
    ///
    /// The SQL is read with the backend's own lexical rules. An ordinary
    /// PostgreSQL string is read with `standard_conforming_strings` on, as
    /// generated SQL requires.
    fn to_row_bound_filter_expression(&self) -> Option<FilterExpression> {
        self.to_filter_expression()
    }

    /// The entity's declared table, which binds the governed row at the
    /// root of a statement. Generated filters return it; the default is
    /// `None`.
    fn table_name() -> Option<&'static str> {
        None
    }

    /// Macro-only. The entity derive implements this for generated where
    /// inputs, whose SQL is trusted generated code: it is rendered for the
    /// row it is given and is not read by the hand-written validator. A
    /// hand-written filter must keep the default (`None`), so that its SQL
    /// is validated wherever a predicate is built from it; overriding this
    /// is outside the guarantee described on
    /// [`Self::to_row_bound_filter_expression`].
    #[doc(hidden)]
    fn __gom_row_bound_filter(&self) -> Option<RowBoundFilter> {
        None
    }
}

/// The qualifier a hand-written [`ReadVisibilityFilter`] writes for "the row
/// this predicate describes": `__gom_self.<column>`. It is never sent to the
/// database. [`ReadPredicate::from_filter`] reads the filter's SQL once and
/// turns each qualifier into a row reference, which every rendering binds to
/// the row's own name. Predicates the ORM builds, and generated filters,
/// never hold this word in their SQL (#796).
pub const READ_PREDICATE_ROW: &str = "__gom_self";

/// `Err` when `table`'s own name, compared without ASCII case, is one read
/// predicates generate: `__gom_self`, `__gom_row_<n>`,
/// `__gom_visibility_<n>`, `__gom_closure_<n>` or a name derived from it
/// (`__gom_closure_<n>_<suffix>`). A table of such a name could take a row
/// alias's place for a column the aliased table lacks, or be shadowed by a
/// closure's common table expression. The entity derive refuses it at
/// compile time; for a hand-built entity every read-predicate constructor,
/// related-row filter and count ordering refuses it, as does attaching a
/// predicate to a statement over the entity. Other names starting with
/// `__gom_` are allowed: aliases are allocated around them.
fn ensure_table_name(backend: DatabaseBackend, table: &str) -> async_graphql::Result<()> {
    if generated_name(&backend.table_name_part(table).to_ascii_lowercase()) {
        return Err(invalid());
    }
    Ok(())
}

type RowRenderer = std::sync::Arc<dyn Fn(&str) -> String + Send + Sync>;

#[derive(Clone)]
enum RowPiece {
    /// SQL text that names no governed row.
    Sql(String),
    /// The governed row.
    Row,
    /// A generated filter, which renders its own SQL for the governed row.
    Rendered(RowRenderer),
}

/// Policy SQL in which "the row this predicate governs" is a value of its
/// own rather than text: a sequence of SQL text, row references and
/// generated filters. [`Self::bind`] writes the row's name at each
/// reference; nothing is searched for or replaced (#796).
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct RowBoundSql {
    pieces: Vec<RowPiece>,
}

impl std::fmt::Debug for RowBoundSql {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.bind("<row>"))
    }
}

/// SQL text that refers to no governed row.
impl From<&str> for RowBoundSql {
    fn from(text: &str) -> Self {
        Self::sql(text)
    }
}

impl From<String> for RowBoundSql {
    fn from(text: String) -> Self {
        Self::sql(text)
    }
}

impl RowBoundSql {
    fn sql(text: impl Into<String>) -> Self {
        let mut sql = Self::default();
        sql.push_str(&text.into());
        sql
    }

    fn push_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(RowPiece::Sql(last)) = self.pieces.last_mut() {
            last.push_str(text);
        } else {
            self.pieces.push(RowPiece::Sql(text.to_owned()));
        }
    }

    fn push_row(&mut self) {
        self.pieces.push(RowPiece::Row);
    }

    fn push(&mut self, other: &Self) {
        for piece in &other.pieces {
            match piece {
                RowPiece::Sql(text) => self.push_str(text),
                piece => self.pieces.push(piece.clone()),
            }
        }
    }

    /// `prefix`, this SQL, then `suffix`.
    fn wrapped(&self, prefix: &str, suffix: &str) -> Self {
        let mut sql = Self::sql(prefix);
        sql.push(self);
        sql.push_str(suffix);
        sql
    }

    /// Whether any piece refers to the governed row.
    fn holds_row(&self) -> bool {
        self.pieces
            .iter()
            .any(|piece| !matches!(piece, RowPiece::Sql(_)))
    }

    /// This SQL with `row`, a rendered table path or alias, written at each
    /// reference to the governed row.
    pub fn bind(&self, row: &str) -> String {
        let mut bound = String::new();
        for piece in &self.pieces {
            match piece {
                RowPiece::Sql(text) => bound.push_str(text),
                RowPiece::Row => bound.push_str(row),
                RowPiece::Rendered(render) => bound.push_str(&render(row)),
            }
        }
        bound
    }
}

/// Generated filter support: a generated where input as a function from the
/// governed row's name to its SQL conditions and bind values. The filter
/// writes `row.<delimited column>` for every column it tests, so no row name
/// is ever substituted into its SQL (#796).
#[doc(hidden)]
#[derive(Clone)]
pub struct RowBoundFilter {
    render: std::sync::Arc<dyn Fn(&str) -> (Vec<String>, Vec<SqlValue>) + Send + Sync>,
}

impl RowBoundFilter {
    pub fn new(
        render: impl Fn(&str) -> (Vec<String>, Vec<SqlValue>) + Send + Sync + 'static,
    ) -> Self {
        Self {
            render: std::sync::Arc::new(render),
        }
    }

    /// The filter's conditions and bind values for the row `row`, a
    /// rendered table path or alias.
    pub fn conditions(&self, row: &str) -> (Vec<String>, Vec<SqlValue>) {
        (self.render)(row)
    }

    /// The filter's conditions for `row`, joined as
    /// [`DatabaseFilter::to_filter_expression`] joins them, with anonymous
    /// placeholders, and its bind values. `None` for an empty filter.
    fn render(&self, backend: DatabaseBackend, row: &str) -> Option<(String, Vec<SqlValue>)> {
        let (conditions, values) = self.conditions(row);
        let expression =
            super::query::filter_expression_from_raw_parts(backend, &conditions, &values)?;
        let mut bound = Vec::new();
        let clause =
            render_filter_expression(DatabaseBackend::Sqlite, &expression, &mut 1, &mut bound);
        Some((clause, bound))
    }

    /// The filter as one policy leaf, or `None` when it is empty. The bind
    /// values do not depend on the row.
    fn policy(self, backend: DatabaseBackend) -> Option<Policy> {
        let (_, values) = self.render(backend, READ_PREDICATE_ROW)?;
        let renderer: RowRenderer = std::sync::Arc::new(move |row| {
            self.render(backend, row)
                .map(|(clause, _)| clause)
                .unwrap_or_default()
        });
        Some(Policy::Sql {
            sql: RowBoundSql {
                pieces: vec![RowPiece::Rendered(renderer)],
            },
            values,
        })
    }
}

/// A read predicate's SQL: leaves over the governed row, and the `and`/`or`
/// lists that combine them.
#[derive(Clone, Debug)]
enum Policy {
    Sql {
        sql: RowBoundSql,
        values: Vec<SqlValue>,
    },
    /// Keep the original rendering and binds for composition. The alternate
    /// form is used only when this closure is the entire root policy.
    Closure {
        sql: RowBoundSql,
        values: Vec<SqlValue>,
        root: std::sync::Arc<(RowBoundSql, Vec<SqlValue>)>,
    },
    And(Vec<Policy>),
    Or(Vec<Policy>),
}

impl Policy {
    fn sql(text: &str) -> Self {
        Self::Sql {
            sql: RowBoundSql::sql(text),
            values: Vec::new(),
        }
    }

    /// Whether any leaf refers to the governed row.
    fn holds_row(&self) -> bool {
        match self {
            Self::Sql { sql, .. } | Self::Closure { sql, .. } => sql.holds_row(),
            Self::And(children) | Self::Or(children) => children.iter().any(Self::holds_row),
        }
    }

    /// This policy with the governed row bound to `row`.
    fn expression(&self, row: &str) -> FilterExpression {
        match self {
            Self::Sql { sql, values } | Self::Closure { sql, values, .. } => {
                FilterExpression::trusted_fragment(sql.bind(row), values.clone())
            }
            Self::And(children) => {
                FilterExpression::And(children.iter().map(|child| child.expression(row)).collect())
            }
            Self::Or(children) => {
                FilterExpression::Or(children.iter().map(|child| child.expression(row)).collect())
            }
        }
    }

    /// One SQL fragment over the governed row with its bind values in
    /// order, joined exactly as [`render_filter_expression`] joins a
    /// [`FilterExpression`]: anonymous placeholders, each child of a list in
    /// parentheses, long lists as balanced pairs.
    fn flatten(&self) -> (RowBoundSql, Vec<SqlValue>) {
        let mut values = Vec::new();
        let sql = self.flatten_into(&mut values);
        (sql, values)
    }

    fn flatten_into(&self, values: &mut Vec<SqlValue>) -> RowBoundSql {
        fn join(parts: &[RowBoundSql], separator: &str, out: &mut RowBoundSql) {
            match parts {
                [] => {}
                [only] => out.push(only),
                [left, right] => {
                    out.push(left);
                    out.push_str(separator);
                    out.push(right);
                }
                _ => {
                    let (left, right) = parts.split_at(parts.len() / 2);
                    for (index, half) in [left, right].into_iter().enumerate() {
                        if index == 1 {
                            out.push_str(separator);
                        }
                        if half.len() == 1 {
                            out.push(&half[0]);
                        } else {
                            out.push_str("(");
                            join(half, separator, out);
                            out.push_str(")");
                        }
                    }
                }
            }
        }
        let empty = |sql: &RowBoundSql| !sql.holds_row() && sql.bind("").is_empty();
        match self {
            Self::Sql { sql, values: own }
            | Self::Closure {
                sql, values: own, ..
            } => {
                values.extend(own.iter().cloned());
                sql.clone()
            }
            Self::And(children) => {
                let parts = children
                    .iter()
                    .map(|child| child.flatten_into(values))
                    .filter(|sql| !empty(sql))
                    .map(|sql| sql.wrapped("(", ")"))
                    .collect::<Vec<_>>();
                let mut out = RowBoundSql::default();
                join(&parts, " AND ", &mut out);
                out
            }
            Self::Or(children) => {
                if children.is_empty() {
                    return RowBoundSql::sql("1 = 0");
                }
                let parts = children
                    .iter()
                    .map(|child| {
                        let sql = child.flatten_into(values);
                        if empty(&sql) {
                            RowBoundSql::sql("(1 = 1)")
                        } else {
                            sql.wrapped("(", ")")
                        }
                    })
                    .collect::<Vec<_>>();
                let mut out = RowBoundSql::default();
                join(&parts, " OR ", &mut out);
                out
            }
        }
    }

    /// Every fragment is non-empty and every conjunction has a child. An
    /// empty disjunction is accepted here and refused as a match-nothing
    /// term.
    fn negatable(&self) -> bool {
        match self {
            Self::Sql { sql, .. } => sql.holds_row() || !sql.bind("").trim().is_empty(),
            Self::Closure { .. } => true,
            Self::And(children) => !children.is_empty() && children.iter().all(Self::negatable),
            Self::Or(children) => children.iter().all(Self::negatable),
        }
    }
}

/// The significant tokens of `sql` (comments left out), read with the
/// backend's lexical rules. SQL that cannot be read with certainty is
/// refused rather than guessed at.
fn sql_tokens(backend: DatabaseBackend, sql: &str) -> async_graphql::Result<Vec<SqlToken>> {
    let mut tokens = Vec::new();
    scan_sql(backend, sql, |token| {
        if token.kind != SqlTokenKind::Comment {
            tokens.push(token);
        }
    })
    .map_err(|_| invalid())?;
    Ok(tokens)
}

/// Why hand-written policy SQL was refused. The reason is reported with the
/// `AUTHORIZATION_MISCONFIGURED` error as its server-side diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PolicySqlRefusal {
    /// A string, identifier or dollar quote is not closed.
    Unreadable,
    /// `SELECT`, `FROM`, `JOIN`, `WITH`, `UNION`, `INTERSECT`, `EXCEPT`,
    /// `VALUES` or `TABLE`.
    Keyword,
    /// A comment.
    Comment,
    /// A PostgreSQL `U&'...'` string or `U&"..."` identifier.
    UnicodeEscape,
    /// Two string constants separated only by whitespace.
    StringContinuation,
    /// A delimited identifier that names a generated alias or the row
    /// qualifier.
    ReservedName,
    /// The bare word `__gom_self` with no dot after it.
    AmbiguousRow,
    /// SQLite's implicit `rowid`, `_rowid_` or `oid`.
    ImplicitRowid,
    /// The row qualifier in a filter that names no table.
    MissingTable,
    /// A column that is not written through the row qualifier, where the
    /// predicate is evaluated in a correlated subquery.
    UnboundColumn,
    /// A closing delimiter with no opening one, an opening one left open,
    /// or a statement separator.
    Unbalanced,
    /// A string constant where an identifier stands: beside a dot.
    StringIdentifier,
    /// A SQL Server `#` name.
    TemporaryName,
    /// On SQLite, `IN` over a table or a table-valued function.
    TableOperand,
    /// A byte-order mark or other non-ASCII whitespace outside a string.
    HiddenWhitespace,
    /// A numbered or named placeholder: only the anonymous `?` composes.
    NamedPlaceholder,
    /// A clause whose anonymous placeholders and bind values differ in
    /// number.
    PlaceholderCount,
}

impl PolicySqlRefusal {
    fn reason(self) -> &'static str {
        match self {
            Self::Unreadable => {
                "hand-written policy SQL holds an unclosed string, identifier or dollar quote"
            }
            Self::Keyword => {
                "hand-written policy SQL holds SELECT, FROM, JOIN, WITH, UNION, INTERSECT, EXCEPT, VALUES or TABLE; use the related, not_related or closure builders to read another table"
            }
            Self::Unbalanced => {
                "hand-written policy SQL holds an unmatched parenthesis or bracket, or a semicolon"
            }
            Self::StringIdentifier => {
                "hand-written policy SQL writes a string constant beside a dot, where an identifier stands"
            }
            Self::TemporaryName => "hand-written policy SQL holds a # name",
            Self::TableOperand => {
                "hand-written policy SQL uses IN over a table or table-valued function"
            }
            Self::NamedPlaceholder => {
                "hand-written policy SQL holds a numbered or named placeholder; write the anonymous ?"
            }
            Self::PlaceholderCount => {
                "a hand-written policy clause has a different number of ? placeholders and bind values"
            }
            Self::HiddenWhitespace => {
                "hand-written policy SQL holds a byte-order mark or non-ASCII whitespace outside a string"
            }
            Self::Comment => "hand-written policy SQL holds a comment",
            Self::UnicodeEscape => "hand-written policy SQL holds a U& string or identifier",
            Self::StringContinuation => "hand-written policy SQL holds adjacent string constants",
            Self::ReservedName => {
                "hand-written policy SQL holds a delimited identifier reserved for generated names"
            }
            Self::AmbiguousRow => {
                "hand-written policy SQL writes __gom_self without a column after it"
            }
            Self::ImplicitRowid => {
                "hand-written policy SQL names SQLite's implicit rowid, _rowid_ or oid"
            }
            Self::MissingTable => {
                "a hand-written filter writes the __gom_self qualifier but returns no table_name"
            }
            Self::UnboundColumn => {
                "a hand-written filter evaluated in a correlated subquery must write every column as __gom_self.<column>"
            }
        }
    }
}

impl From<PolicySqlRefusal> for async_graphql::Error {
    fn from(refusal: PolicySqlRefusal) -> Self {
        OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured)
            .with_internal(refusal.reason())
            .into_graphql_error()
    }
}

/// The name inside a delimited identifier token, a doubled closing
/// delimiter read as one, in lower case.
fn delimited_name(text: &str) -> String {
    let close = match text.as_bytes().first() {
        Some(b'[') => ']',
        Some(open) => *open as char,
        None => return String::new(),
    };
    text.get(1..text.len().saturating_sub(1))
        .unwrap_or("")
        .replace(&format!("{close}{close}"), &close.to_string())
        .to_ascii_lowercase()
}

/// Whether `name`, in lower case, is a name read predicates generate: the
/// row qualifier `__gom_self`, a row alias `__gom_row_<n>` or
/// `__gom_visibility_<n>`, a closure name `__gom_closure_<n>`, or a name
/// derived from one, `__gom_closure_<n>_<suffix>`.
fn generated_name(name: &str) -> bool {
    let numbered = |rest: &str| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit());
    if name == READ_PREDICATE_ROW {
        return true;
    }
    if let Some(rest) = ROW_ALIASES
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
    {
        return numbered(rest);
    }
    name.strip_prefix(CLOSURE_ALIASES[0]).is_some_and(|rest| {
        let (level, suffix) = rest.split_once('_').unwrap_or((rest, "x"));
        numbered(level) && !suffix.is_empty()
    })
}

/// Words that are reserved in every supported dialect wherever they stand
/// in an expression, so they never name a column.
const POLICY_SQL_RESERVED: &[&str] = &[
    "and", "as", "case", "else", "is", "not", "null", "or", "then", "when",
];

/// Words that are operators only after an operand. Elsewhere, or in a
/// dialect that does not have them, a dialect may read them as a column
/// name, so they are classified as one. The dialects that have each:
/// every one for the first row, then PostgreSQL, then SQLite (and
/// PostgreSQL for the postfix null tests).
fn policy_sql_operator(backend: DatabaseBackend, word: &str) -> bool {
    match word {
        "between" | "collate" | "end" | "escape" | "in" | "like" => true,
        "ilike" | "similar" => backend == DatabaseBackend::Postgres,
        "glob" => backend == DatabaseBackend::Sqlite,
        "regexp" => matches!(backend, DatabaseBackend::Sqlite | DatabaseBackend::Mysql),
        "isnull" | "notnull" => {
            matches!(backend, DatabaseBackend::Sqlite | DatabaseBackend::Postgres)
        }
        _ => false,
    }
}

/// Whether `word`, in lower case, could be a keyword that expects an operand
/// after it: after such a word an operator cannot stand, and a number may
/// start with its dot.
fn policy_sql_expects_operand(word: &str) -> bool {
    matches!(
        word,
        "and"
            | "as"
            | "between"
            | "case"
            | "collate"
            | "else"
            | "escape"
            | "glob"
            | "ilike"
            | "in"
            | "is"
            | "like"
            | "not"
            | "or"
            | "regexp"
            | "similar"
            | "then"
            | "to"
            | "when"
    )
}

/// Whether `word`, in lower case, is a constant of `backend` that needs no
/// row: a boolean literal where the dialect has one, or a scalar constant
/// such as `CURRENT_TIMESTAMP`. SQL Server has no `TRUE` or `FALSE`, so
/// there they would be column names.
///
/// Residual: SQLite reads `true` and `false` as a column of that name when
/// the table in scope has one, so a policy over such a table must not write
/// the bare literal.
fn policy_sql_constant(backend: DatabaseBackend, word: &str) -> bool {
    match word {
        "true" | "false" => backend != DatabaseBackend::Mssql,
        "current_timestamp" => true,
        "current_date" | "current_time" => backend != DatabaseBackend::Mssql,
        "localtimestamp" | "localtime" | "session_user" => backend == DatabaseBackend::Postgres,
        "current_user" => matches!(backend, DatabaseBackend::Postgres | DatabaseBackend::Mssql),
        _ => false,
    }
}

/// SQLite scalar functions a policy may call directly inside `IN (...)`,
/// where any other call could be a table-valued function.
const SQLITE_SCALAR_FUNCTIONS: &[&str] = &[
    "abs",
    "cast",
    "char",
    "coalesce",
    "date",
    "datetime",
    "format",
    "hex",
    "ifnull",
    "iif",
    "instr",
    "julianday",
    "length",
    "lower",
    "ltrim",
    "max",
    "min",
    "nullif",
    "printf",
    "quote",
    "replace",
    "round",
    "rtrim",
    "sign",
    "strftime",
    "substr",
    "substring",
    "time",
    "trim",
    "typeof",
    "unicode",
    "unixepoch",
    "upper",
];

/// One clause of a hand-written filter as SQL over the governed row, and
/// whether every column it tests is written through the row qualifier.
///
/// A hand-written clause may only be a single-row boolean expression over
/// the governed row's own columns and bound parameters. It is refused when,
/// outside string constants, it holds:
///
/// - `SELECT`, `FROM`, `JOIN`, `WITH`, `UNION`, `INTERSECT` or `EXCEPT`: no
///   subquery and no alias of its own, so no name in it can capture a
///   generated alias or the root table. Policies over another table use the
///   structured builders;
/// - a comment, a PostgreSQL `U&` string or identifier, or two string
///   constants separated only by whitespace;
/// - a delimited identifier that names a generated alias or the qualifier;
/// - on SQLite, `rowid`, `_rowid_` or `oid`, which every table has
///   implicitly;
/// - the bare word `__gom_self` with no dot after it, or SQL with an
///   unclosed string, identifier or dollar quote.
///
/// Each [`READ_PREDICATE_ROW`] qualifier (the bare word, without ASCII
/// case, followed by a dot and not itself after a dot) becomes a row
/// reference. A column is bound when it follows the qualifier. Any other
/// name the clause could resolve as a column makes it unbound: a bare or
/// delimited identifier that is not a keyword of a boolean expression, a
/// number, a parameter, a function call, a type after `AS` or `::`, a
/// collation after `COLLATE` or a string prefix, and any name qualified by
/// something other than the row.
fn handwritten_sql(
    backend: DatabaseBackend,
    clause: &str,
    binds: usize,
) -> Result<(RowBoundSql, bool), PolicySqlRefusal> {
    let mut tokens = Vec::new();
    let readable = scan_sql(backend, clause, |token| tokens.push(token));
    if tokens
        .iter()
        .any(|token| token.kind == SqlTokenKind::Comment)
    {
        return Err(PolicySqlRefusal::Comment);
    }
    readable.map_err(|_| PolicySqlRefusal::Unreadable)?;
    let text = |token: &SqlToken| &clause[token.start..token.end];
    let at = |index: Option<usize>| index.and_then(|index| tokens.get(index));
    let symbol = |token: Option<&SqlToken>, symbol: &str| {
        token.is_some_and(|token| token.kind == SqlTokenKind::Symbol && text(token) == symbol)
    };
    let word = |token: Option<&SqlToken>, word: &str| {
        token.is_some_and(|token| {
            token.kind == SqlTokenKind::Word && text(token).eq_ignore_ascii_case(word)
        })
    };
    // The qualifier: the bare word, not after a dot.
    let qualifier = |index: Option<usize>| {
        word(at(index), READ_PREDICATE_ROW)
            && !symbol(at(index.and_then(|i| i.checked_sub(1))), ".")
    };
    let implicit_rowid = |name: &str| {
        backend == DatabaseBackend::Sqlite && matches!(name, "rowid" | "_rowid_" | "oid")
    };
    // Whether an operand ends at `token`, so that an operator may follow.
    // Whether the token at `index` is a word that reads as a keyword
    // expecting an operand. A word directly after a dot is the last part of
    // a qualified name: always an identifier, whatever it spells.
    let expects_operand = |index: Option<usize>| {
        at(index).is_some_and(|token| {
            token.kind == SqlTokenKind::Word
                && !symbol(at(index.and_then(|index| index.checked_sub(1))), ".")
                && policy_sql_expects_operand(&text(token).to_ascii_lowercase())
        })
    };
    // Whether an operand ends at the token at `index`, so that an operator
    // may follow.
    let ends_operand = |index: Option<usize>| {
        at(index).is_some_and(|token| match token.kind {
            SqlTokenKind::StringLiteral
            | SqlTokenKind::QuotedIdentifier
            | SqlTokenKind::Placeholder => true,
            SqlTokenKind::Symbol => matches!(text(token), ")" | "]"),
            SqlTokenKind::Word => !expects_operand(index),
            SqlTokenKind::Comment => false,
        })
    };
    // Only anonymous placeholders compose: the statement numbers them in
    // order after the binds before them. Each clause carries exactly its
    // own values.
    let mut placeholders = 0;
    for token in &tokens {
        match token.kind {
            SqlTokenKind::Placeholder if text(token) == "?" => placeholders += 1,
            SqlTokenKind::Placeholder => return Err(PolicySqlRefusal::NamedPlaceholder),
            SqlTokenKind::Word if matches!(text(token).as_bytes()[0], b'@' | b':' | b'$') => {
                return Err(PolicySqlRefusal::NamedPlaceholder);
            }
            _ => {}
        }
    }
    if placeholders != binds {
        return Err(PolicySqlRefusal::PlaceholderCount);
    }
    let mut sql = RowBoundSql::default();
    let mut copied = 0;
    let mut bound = true;
    // Open parentheses and brackets, so the clause can close neither its
    // own group nor the group it is composed into.
    let mut open: Vec<&str> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let before = at(index.checked_sub(1));
        let after = at(Some(index + 1));
        // Whitespace the lexer does not skip would be read as part of a
        // word here and as a separator by the database.
        if !matches!(
            token.kind,
            SqlTokenKind::StringLiteral | SqlTokenKind::QuotedIdentifier
        ) && text(token)
            .chars()
            .any(|character| character == '\u{feff}' || character.is_whitespace())
        {
            return Err(PolicySqlRefusal::HiddenWhitespace);
        }
        let after_dot = symbol(before, ".");
        // Whether this name is a column of the governed row.
        let of_row = after_dot && qualifier(index.checked_sub(2));
        match token.kind {
            SqlTokenKind::Word => {
                let lower = text(token).to_ascii_lowercase();
                if matches!(
                    lower.as_str(),
                    "select"
                        | "from"
                        | "join"
                        | "with"
                        | "union"
                        | "intersect"
                        | "except"
                        | "values"
                        | "table"
                ) {
                    return Err(PolicySqlRefusal::Keyword);
                }
                if backend == DatabaseBackend::Mssql && lower.starts_with('#') {
                    return Err(PolicySqlRefusal::TemporaryName);
                }
                // SQLite reads `x IN t` and `x IN f(...)` as a table or
                // table-valued function, and so `x IN (f(...))` for a
                // function it does not know as scalar.
                if backend == DatabaseBackend::Sqlite && lower == "in" && !after_dot {
                    if !symbol(after, "(") {
                        return Err(PolicySqlRefusal::TableOperand);
                    }
                    let operand = at(Some(index + 2));
                    if operand.is_some_and(|operand| operand.kind == SqlTokenKind::Word)
                        && symbol(at(Some(index + 3)), "(")
                        && !operand.is_some_and(|operand| {
                            SQLITE_SCALAR_FUNCTIONS
                                .binary_search(&text(operand).to_ascii_lowercase().as_str())
                                .is_ok()
                        })
                    {
                        return Err(PolicySqlRefusal::TableOperand);
                    }
                }
                if implicit_rowid(&lower) {
                    return Err(PolicySqlRefusal::ImplicitRowid);
                }
                let adjacent = |left: &SqlToken, right: Option<&SqlToken>| {
                    right.is_some_and(|right| right.start == left.end)
                };
                let quoted = |token: Option<&SqlToken>| {
                    token.is_some_and(|token| {
                        matches!(
                            token.kind,
                            SqlTokenKind::StringLiteral | SqlTokenKind::QuotedIdentifier
                        )
                    })
                };
                if backend == DatabaseBackend::Postgres
                    && lower == "u"
                    && symbol(after, "&")
                    && adjacent(token, after)
                    && quoted(at(Some(index + 2)))
                    && after.is_some_and(|ampersand| adjacent(ampersand, at(Some(index + 2))))
                {
                    return Err(PolicySqlRefusal::UnicodeEscape);
                }
                if qualifier(Some(index)) {
                    if !symbol(after, ".") {
                        return Err(PolicySqlRefusal::AmbiguousRow);
                    }
                    sql.push_str(&clause[copied..token.start]);
                    sql.push_row();
                    copied = token.end;
                    continue;
                }
                let number = lower.as_bytes()[0].is_ascii_digit();
                let allowed = if after_dot {
                    // A column of the row, or the fraction of a number:
                    // after its whole part, or with none (`.5`). A dot
                    // directly before a digit starts a number unless a
                    // name, a quote or a closing bracket precedes it.
                    of_row
                        || (number
                            && before.is_some_and(|dot| dot.end == token.start)
                            && at(index.checked_sub(2)).is_none_or(|whole| match whole.kind {
                                SqlTokenKind::Word => {
                                    text(whole).as_bytes()[0].is_ascii_digit()
                                        || expects_operand(index.checked_sub(2))
                                }
                                SqlTokenKind::Symbol => !matches!(text(whole), ")" | "]"),
                                SqlTokenKind::Placeholder => true,
                                _ => false,
                            }))
                } else {
                    number
                        || symbol(after, "(")
                        || POLICY_SQL_RESERVED.binary_search(&lower.as_str()).is_ok()
                        // An operator, after an operand or after the `NOT`
                        // that negates it.
                        || (policy_sql_operator(backend, &lower)
                            && (ends_operand(index.checked_sub(1))
                                || (word(before, "not")
                                    && ends_operand(index.checked_sub(2))
                                    && matches!(
                                        lower.as_str(),
                                        "between"
                                            | "glob"
                                            | "ilike"
                                            | "in"
                                            | "like"
                                            | "regexp"
                                            | "similar"
                                    ))))
                        // PostgreSQL `SIMILAR TO` and `IS [NOT] UNKNOWN`.
                        || (backend == DatabaseBackend::Postgres
                            && ((lower == "to" && word(before, "similar"))
                                || (lower == "unknown"
                                    && (word(before, "is")
                                        || (word(before, "not")
                                            && word(at(index.checked_sub(2)), "is"))))))
                        || policy_sql_constant(backend, &lower)
                        || word(before, "as")
                        || word(before, "collate")
                        || (symbol(before, ":") && symbol(at(index.checked_sub(2)), ":"))
                        || (after.is_some_and(|next| next.kind == SqlTokenKind::StringLiteral)
                            && adjacent(token, after))
                };
                bound &= allowed;
            }
            SqlTokenKind::QuotedIdentifier => {
                let name = delimited_name(text(token));
                if generated_name(&name) {
                    return Err(PolicySqlRefusal::ReservedName);
                }
                if implicit_rowid(&name) {
                    return Err(PolicySqlRefusal::ImplicitRowid);
                }
                bound &= of_row;
            }
            SqlTokenKind::StringLiteral => {
                if after.is_some_and(|next| next.kind == SqlTokenKind::StringLiteral) {
                    return Err(PolicySqlRefusal::StringContinuation);
                }
                // A string beside a dot stands where an identifier does:
                // some dialects read it as a name.
                if after_dot || symbol(after, ".") {
                    return Err(PolicySqlRefusal::StringIdentifier);
                }
            }
            SqlTokenKind::Symbol => match text(token) {
                // MySQL reads `#` to the end of the line as a comment.
                "#" if backend == DatabaseBackend::Mysql => {
                    return Err(PolicySqlRefusal::Comment);
                }
                ";" => return Err(PolicySqlRefusal::Unbalanced),
                opening @ ("(" | "[") => open.push(opening),
                closing @ (")" | "]") => {
                    let opening = if closing == ")" { "(" } else { "[" };
                    if open.pop() != Some(opening) {
                        return Err(PolicySqlRefusal::Unbalanced);
                    }
                }
                _ => {}
            },
            SqlTokenKind::Comment | SqlTokenKind::Placeholder => {}
        }
    }
    if !open.is_empty() {
        return Err(PolicySqlRefusal::Unbalanced);
    }
    sql.push_str(&clause[copied..]);
    Ok((sql, bound))
}

/// A hand-written filter expression as a policy, and whether every column
/// it tests is written through the row qualifier: see [`handwritten_sql`].
fn handwritten_policy(
    backend: DatabaseBackend,
    expression: &FilterExpression,
    bound: &mut bool,
) -> Result<Policy, PolicySqlRefusal> {
    Ok(match expression {
        FilterExpression::Raw { clause, values }
        | FilterExpression::TrustedFragment { clause, values } => {
            let (sql, clause_bound) = handwritten_sql(backend, clause, values.len())?;
            *bound &= clause_bound;
            Policy::Sql {
                sql,
                values: values.clone(),
            }
        }
        FilterExpression::And(children) => Policy::And(
            children
                .iter()
                .map(|child| handwritten_policy(backend, child, bound))
                .collect::<Result<_, _>>()?,
        ),
        FilterExpression::Or(children) => Policy::Or(
            children
                .iter()
                .map(|child| handwritten_policy(backend, child, bound))
                .collect::<Result<_, _>>()?,
        ),
    })
}

/// `Err` unless `conditions`, read with the backend's lexer so that text in
/// strings, delimited names and comments is not counted, hold exactly
/// `binds` placeholders in total. A filter's conditions share one flat list
/// of bind values, which is divided among them by their placeholder counts:
/// a surplus value would be dropped silently and a missing one would shift
/// the values of the conditions after it.
fn ensure_bind_count(
    backend: DatabaseBackend,
    conditions: &[String],
    binds: usize,
) -> Result<(), PolicySqlRefusal> {
    let mut placeholders = 0;
    for condition in conditions {
        scan_sql(backend, condition, |token| {
            placeholders += usize::from(token.kind == SqlTokenKind::Placeholder);
        })
        .map_err(|_| PolicySqlRefusal::Unreadable)?;
    }
    if placeholders != binds {
        return Err(PolicySqlRefusal::PlaceholderCount);
    }
    Ok(())
}

/// `filter` as a policy over the governed row, or `None` when it is empty,
/// and whether every column it tests is bound to the row. A generated
/// filter renders itself for the row; any other filter's SQL is read by
/// [`handwritten_policy`].
fn filter_policy<F: ReadVisibilityFilter>(
    backend: DatabaseBackend,
    filter: &F,
) -> async_graphql::Result<(Option<Policy>, bool)> {
    if let Some(generated) = filter.__gom_row_bound_filter() {
        let (conditions, values) = generated.conditions(READ_PREDICATE_ROW);
        ensure_bind_count(backend, &conditions, values.len())?;
        return Ok((generated.policy(backend), true));
    }
    // One call into the filter: everything below reads this one result.
    // It is a checked call for this dialect, so the default adapter counts
    // placeholders as `backend` reads them, whatever the build's default
    // backend is, and refuses a flat bind list that does not pair up. A
    // filter that returns an expression of its own is validated leaf by
    // leaf below.
    let expression = super::query::checked_filter_expression(backend, || {
        filter.to_row_bound_filter_expression()
    })
    .map_err(|_| PolicySqlRefusal::PlaceholderCount)?;
    let mut bound = true;
    let policy = expression
        .map(|expression| handwritten_policy(backend, &expression, &mut bound))
        .transpose()?;
    Ok((policy, bound))
}

/// The name the governed row has at the root of a statement over `table`.
fn root_row(backend: DatabaseBackend, table: Option<&str>) -> String {
    match table {
        Some(table) => backend.render_table_identifier_path(table),
        None => READ_PREDICATE_ROW.to_owned(),
    }
}

/// `column`, a validated declared column of the governed row.
fn row_bound_column(backend: DatabaseBackend, column: &str) -> RowBoundSql {
    let mut sql = RowBoundSql::default();
    sql.push_row();
    sql.push_str(&format!(".{}", backend.delimited_column_identifier(column)));
    sql
}

/// The names a new subquery scope must not reuse: every reserved name
/// (`__gom_...`, lower-cased) that the SQL inside the scope mentions, bare
/// or delimited, as a table, an alias or a common table expression, plus
/// each table and alias the caller knows to be visible around it.
///
/// A row alias exists so that a column its table lacks fails the statement.
/// SQLite keeps searching enclosing scopes for a table exposing the same
/// name, and a common table expression shadows a table of its name, so an
/// alias or expression name must differ from every table and alias in
/// scope. All of them come from [`Self::level`] (#796).
struct Scope {
    names: std::collections::BTreeSet<String>,
}

impl Scope {
    /// The reserved names `sql` mentions.
    fn of(backend: DatabaseBackend, sql: &str) -> async_graphql::Result<Self> {
        let mut names = std::collections::BTreeSet::new();
        for token in sql_tokens(backend, sql)? {
            let text = &sql[token.start..token.end];
            let name = match token.kind {
                SqlTokenKind::Word => text.to_ascii_lowercase(),
                SqlTokenKind::QuotedIdentifier => delimited_name(text),
                _ => continue,
            };
            if name.starts_with("__gom_") {
                names.insert(name);
            }
        }
        Ok(Self { names })
    }

    /// Also avoid `table`'s own name, the qualifier it is visible by.
    fn table(mut self, backend: DatabaseBackend, table: &str) -> Self {
        self.names
            .insert(backend.table_name_part(table).to_ascii_lowercase());
        self
    }

    /// The lowest level `N` (from 1) for which no name in scope is
    /// `<prefix>N` or starts with `<prefix>N_`, for every prefix given. The
    /// names `<prefix>N` and `<prefix>N_<suffix>` are then free to use.
    fn level(&self, prefixes: &[&str]) -> usize {
        (1..)
            .find(|level| {
                !prefixes.iter().any(|prefix| {
                    let taken = format!("{prefix}{level}");
                    let derived = format!("{taken}_");
                    self.names
                        .iter()
                        .any(|name| *name == taken || name.starts_with(&derived))
                })
            })
            .unwrap_or(usize::MAX)
    }
}

/// The prefixes of a relation's row alias and derived-table alias.
const ROW_ALIASES: &[&str] = &["__gom_row_", "__gom_visibility_"];
/// The prefix of a closure's common table expression, and of the aliases
/// derived from it.
const CLOSURE_ALIASES: &[&str] = &["__gom_closure_"];

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
/// `AUTHORIZATION_MISCONFIGURED`. Steps that inline their target (relations,
/// negations, closures) check the size before rendering the larger string;
/// [`Self::from_filter`] renders the filter once to measure it.
///
/// - on every backend, at most [`READ_PREDICATE_MAX_RENDERED_BYTES`] of
///   rendered SQL, [`READ_PREDICATE_MAX_EXPRESSION_DEPTH`] levels of
///   expression nesting, [`READ_PREDICATE_MAX_CASE_NESTING`] nested `CASE`
///   expressions and [`READ_PREDICATE_CLOSURE_MAX_NESTING`] closures nested
///   inside one another;
/// - on SQLite and PostgreSQL, at most [`READ_PREDICATE_MAX_RELATION_NESTING`]
///   nested relations and [`READ_PREDICATE_MAX_RELATIONS`] in total;
/// - on SQL Server, at most [`READ_PREDICATE_MSSQL_MAX_RELATION_NESTING`]
///   nested relations, [`READ_PREDICATE_MSSQL_MAX_RELATIONS`] in total,
///   [`READ_PREDICATE_MSSQL_MAX_NESTING`] nested subqueries and
///   [`READ_PREDICATE_MSSQL_MAX_NESTING_UNITS`] weighted levels.
///
/// The generated relation filters and count orderings of one statement are
/// budgeted together as well: see [`READ_PREDICATE_MAX_STATEMENT_RELATIONS`].
///
/// `and` and `or` flatten a child of the same kind, so a fold of any length
/// is one node, and every list of three or more renders as a balanced binary
/// tree of parenthesised pairs, so its parse depth grows with the logarithm
/// of its length. Within these limits a predicate renders,
/// formats and drops without deep recursion, and every shape probed at the
/// limits ran or was refused, within bounded server memory. They do not
/// bound execution time, cover a statement's bind-parameter count (which
/// each server caps itself, SQL Server at 2,100 for the whole statement), or
/// rule out SQL Server plan-resource errors (8623, 8621) on untested shapes,
/// which fail that read: set a statement timeout on the server as well.
#[derive(Clone, Debug)]
pub struct ReadPredicate {
    entity: TypeId,
    backend: DatabaseBackend,
    /// The entity's declared table, which names the governed row at the
    /// root of a statement. `None` only for a hand-written filter that
    /// refers to no governed row.
    table: Option<&'static str>,
    /// The predicate over the governed row.
    policy: Policy,
    /// Whether a hand-written clause names a column other than through the
    /// row qualifier. Such a predicate is refused wherever it would be
    /// evaluated in a correlated subquery.
    unbound: bool,
    cost: Cost,
    /// How [`Self::not`] renders the rows where the predicate is FALSE.
    negation: Negation,
}

/// The rendering [`ReadPredicate::not`] uses. Each predicate is TRUE, FALSE or
/// UNKNOWN per row, and only TRUE admits. Its admitting rendering may answer
/// FALSE where the predicate is UNKNOWN (a relation's `EXISTS` does), which is
/// harmless under `and` and `or` but not under SQL `NOT`. So each predicate
/// also knows a rendering that is TRUE exactly where it is FALSE, built by
/// De Morgan through `and`/`or` and swapped by `not`.
#[derive(Clone, Debug)]
enum Negation {
    /// `NOT (policy)`: the policy is three-valued SQL.
    Sql,
    /// TRUE exactly where the predicate is FALSE, over the same governed
    /// row, with its checked cost.
    Explicit(Policy, Cost),
    /// Built from a part that cannot be negated, or whose negation is over
    /// the composition budget; `not` refuses it.
    Unsupported,
}

/// The rendering TRUE exactly where a predicate is FALSE, with its checked
/// cost, or `None` when the predicate cannot be negated: an empty fragment or
/// conjunction, the deny fallback `1 = 0` (its negation would be a blanket
/// grant), or a negation over the composition budget. Like the predicate, it
/// is SQL over the governed row, measured as the root of a statement over
/// `table` renders it.
fn negated(
    backend: DatabaseBackend,
    table: Option<&str>,
    policy: &Policy,
    cost: Cost,
    negation: Negation,
) -> Option<(Policy, Cost)> {
    match negation {
        Negation::Sql => {
            if !policy.negatable() {
                return None;
            }
            reserve(cost.bytes, 1, 16).ok()?;
            let (clause, values) = policy.flatten();
            if contains_match_nothing(&clause.bind(READ_PREDICATE_ROW)) {
                return None;
            }
            let clause = clause.wrapped("NOT (", ")");
            let negated = Cost {
                // A negation nests closures no deeper than what it negates.
                closures: cost.closures,
                ..Cost::of(backend, &clause.bind(&root_row(backend, table))).ok()?
            }
            .check(backend)
            .ok()?;
            Some((
                Policy::Sql {
                    sql: clause,
                    values,
                },
                negated,
            ))
        }
        Negation::Explicit(policy, cost) => Some((policy, cost)),
        Negation::Unsupported => None,
    }
}

/// `and`/`or` of two measured policies. A child of the same kind is
/// flattened into the node, so a fold of any length stays one level deep:
/// `(a) OR (b) OR (c)`. Its rendered depth is its deepest child plus one,
/// plus the grouping levels a long chain is rendered with. The cost is not
/// checked here.
fn join(left: (Policy, Cost), right: (Policy, Cost), and: bool) -> (Policy, Cost) {
    let (left_cost, right_cost) = (left.1, right.1);
    let mut children = Vec::new();
    let (mut inner, mut inner_units) = (0, 0);
    for (policy, cost) in [left, right] {
        match policy {
            Policy::And(nested) if and && !nested.is_empty() => {
                inner = inner.max(cost.inner);
                inner_units = inner_units.max(cost.inner_units);
                children.extend(nested);
            }
            Policy::Or(nested) if !and && !nested.is_empty() => {
                inner = inner.max(cost.inner);
                inner_units = inner_units.max(cost.inner_units);
                children.extend(nested);
            }
            policy => {
                inner = inner.max(cost.depth);
                inner_units = inner_units.max(cost.units);
                children.push(policy);
            }
        }
    }
    // Each child is wrapped in plain parentheses, one unit each.
    let levels = 1 + super::query::filter_chain_levels(children.len());
    let cost = Cost {
        bytes: left_cost
            .bytes
            .saturating_add(right_cost.bytes)
            // Each operand of a balanced pair tree costs at most its own
            // `(` `)`, one separator and one grouping pair: nine bytes
            // with ` AND `, eight with ` OR `.
            .saturating_add(if and { 9 } else { 8 }),
        depth: inner.saturating_add(levels),
        inner,
        units: inner_units.saturating_add(levels),
        inner_units,
        nesting: left_cost.nesting.max(right_cost.nesting),
        relations: left_cost.relations.max(right_cost.relations),
        exists: left_cost.exists.saturating_add(right_cost.exists),
        instances: left_cost.instances.saturating_add(right_cost.instances),
        cases: left_cost.cases.max(right_cost.cases),
        closures: left_cost.closures.max(right_cost.closures),
    };
    let policy = if and {
        Policy::And(children)
    } else {
        Policy::Or(children)
    };
    (policy, cost)
}
/// A persisted physical column of `T`. A table-qualified column is accepted
/// only when its qualifier names `T`'s own table: the predicate binds the
/// column to `T`'s row, so any other qualifier is refused rather than
/// stripped (#782).
fn validate_column<B: OrmBackend, T: Entity>(column: &str) -> async_graphql::Result<()> {
    // Every SQLite table has an implicit row id under these names. If a
    // declared column of that name were missing from the table, SQLite
    // would read the row id instead of failing the statement.
    if B::DIALECT == DatabaseBackend::Sqlite
        && matches!(
            B::DIALECT
                .delimited_column_identifier(column)
                .trim_matches('"')
                .to_ascii_lowercase()
                .as_str(),
            "rowid" | "_rowid_" | "oid"
        )
    {
        return Err(PolicySqlRefusal::ImplicitRowid.into());
    }
    if T::columns()
        .iter()
        .any(|definition| definition.name == column)
        && B::DIALECT.column_qualifier_names_table(column, T::TABLE_NAME)
    {
        Ok(())
    } else {
        Err(invalid())
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

/// The largest hop bound [`ReadPredicate::related_closure_within`] accepts on
/// SQL Server, where the walk is unrolled into one nested derived table per
/// hop. SQL Server refuses statements nested much deeper ("nested too
/// deeply", error 191), so a larger bound is refused with
/// `AUTHORIZATION_MISCONFIGURED`. It equals
/// [`READ_PREDICATE_CLOSURE_DEFAULT_DEPTH`], so `related_closure` and
/// `related_closure_all` behave the same on every backend.
pub const READ_PREDICATE_CLOSURE_MSSQL_MAX_DEPTH: u32 = 64;

/// The largest rendered SQL, in bytes, that one [`ReadPredicate`] may hold on
/// any backend. Composition embeds rendered SQL, so policies that duplicate
/// their inputs can grow geometrically. A predicate over this limit is refused with
/// `AUTHORIZATION_MISCONFIGURED` before it is rendered.
pub const READ_PREDICATE_MAX_RENDERED_BYTES: usize = 1 << 20;

/// How many closures may nest inside one another, through any number of
/// relations, negations, `and` and `or`, on any backend: a closure whose
/// target holds a closure is allowed, a third level is refused with
/// `AUTHORIZATION_MISCONFIGURED`. Each level evaluates the inner walk for
/// every row the outer walk reaches, so the cost multiplies per level.
pub const READ_PREDICATE_CLOSURE_MAX_NESTING: u32 = 2;

/// The deepest subquery nesting one [`ReadPredicate`] may render on SQL
/// Server, counted as nested `(SELECT ...)` levels. On the owned server,
/// statements failed with "nested too deeply" (error 191) from 97 levels,
/// and shapes up to 83 ran; the rest is left for the host query, such as a
/// page, count or relation filter around the policy. A deeper predicate is
/// refused with `AUTHORIZATION_MISCONFIGURED` when it is built. A closure at
/// [`READ_PREDICATE_CLOSURE_MSSQL_MAX_DEPTH`] uses 66 levels with the
/// standalone projection and 67 with the original rendering used for
/// composition and budgeting.
pub const READ_PREDICATE_MSSQL_MAX_NESTING: u32 = MSSQL_MAX_NESTING;
const MSSQL_MAX_NESTING: u32 = 75;

/// The deepest nesting of relation subqueries (`EXISTS (SELECT ...)`, as
/// [`ReadPredicate::related`] and [`ReadPredicate::not_related`] render) one
/// [`ReadPredicate`] may render on SQL Server. Relations are correlated
/// joins the optimizer plans together, and statements failed with
/// "insufficient resources" (error 8623) from 29 relations in all; see
/// [`READ_PREDICATE_MSSQL_MAX_STATEMENT_RELATIONS`]. A deeper predicate is
/// refused with `AUTHORIZATION_MISCONFIGURED`.
pub const READ_PREDICATE_MSSQL_MAX_RELATION_NESTING: u32 = MSSQL_MAX_RELATION_NESTING;
const MSSQL_MAX_RELATION_NESTING: u32 = 9;

/// The deepest nesting of relation subqueries one [`ReadPredicate`] may
/// render on SQLite and PostgreSQL. PostgreSQL plans nested relations
/// together, and its memory per statement grows steeply with their depth:
/// on a memory-capped server, a chain of 32 nested relations peaked at about
/// 30 MB, 64 at 230 MB, 96 at 900 MB and 128 at 2.5 GB. A deeper predicate is
/// refused with `AUTHORIZATION_MISCONFIGURED`.
pub const READ_PREDICATE_MAX_RELATION_NESTING: u32 = MAX_RELATION_NESTING;
const MAX_RELATION_NESTING: u32 = 32;

/// The most relation subqueries (`EXISTS (SELECT ...)`) one
/// [`ReadPredicate`] may render in total on SQLite and PostgreSQL, however
/// they are arranged. Each one is planned, so memory grows with the total as
/// well as the depth. A predicate over it is refused with
/// `AUTHORIZATION_MISCONFIGURED`. See also
/// [`READ_PREDICATE_MAX_STATEMENT_RELATIONS`].
pub const READ_PREDICATE_MAX_RELATIONS: u32 = MAX_RELATIONS;
const MAX_RELATIONS: u32 = 128;

/// The most relations one [`ReadPredicate`] may render in total on SQL
/// Server, closures counted at
/// [`READ_PREDICATE_MSSQL_CLOSURE_RELATION_WEIGHT`]. See
/// [`READ_PREDICATE_MSSQL_MAX_STATEMENT_RELATIONS`] for the measurements.
pub const READ_PREDICATE_MSSQL_MAX_RELATIONS: u32 = MSSQL_MAX_RELATIONS;
const MSSQL_MAX_RELATIONS: u32 = 9;

/// How many relations one closure counts as towards the relation totals on
/// SQLite and PostgreSQL ([`READ_PREDICATE_MAX_RELATIONS`] and
/// [`READ_PREDICATE_MAX_STATEMENT_RELATIONS`]). A closure walks and
/// evaluates its target for every row it reaches, so it costs far more than
/// one relation. Budgeting retains the original closure rendering, which
/// counts each copy of the target and its FALSE test at both evaluation sites.
pub const READ_PREDICATE_CLOSURE_RELATION_WEIGHT: u32 = CLOSURE_RELATION_WEIGHT;
const CLOSURE_RELATION_WEIGHT: u32 = 16;

/// As [`READ_PREDICATE_CLOSURE_RELATION_WEIGHT`], on SQL Server, where a
/// closure renders no relation subquery of its own.
pub const READ_PREDICATE_MSSQL_CLOSURE_RELATION_WEIGHT: u32 = MSSQL_CLOSURE_RELATION_WEIGHT;
const MSSQL_CLOSURE_RELATION_WEIGHT: u32 = 1;

/// The deepest nesting of `CASE` expressions one [`ReadPredicate`] may
/// render, on every backend. SQL Server refuses `CASE` nested deeper than
/// ten (error 125). A closure adds one level, and the negation of a relation
/// ([`ReadPredicate::related`] under [`ReadPredicate::not`], or
/// [`ReadPredicate::not_related`]) adds one around its target's own
/// negation. So this also bounds negation: a chain of more than ten nested
/// relations builds but cannot be negated, and a `not_related` chain stops at
/// twenty levels.
pub const READ_PREDICATE_MAX_CASE_NESTING: u32 = MAX_CASE_NESTING;
const MAX_CASE_NESTING: u32 = 10;

/// The deepest expression nesting one [`ReadPredicate`] may render on any
/// backend, counted as nested parentheses: each `not()`, each `and`/`or`
/// level, each relation and each subquery adds at least one. Rendering and
/// dropping a predicate recurse once per level, so this keeps that recursion
/// far inside a 2 MiB thread stack, even in debug builds. A deeper predicate
/// is refused with `AUTHORIZATION_MISCONFIGURED` when it is built. `and` and
/// `or` folds are flattened and render as balanced pairs, so a list of `n`
/// terms adds about `log2(n)` levels. SQLite, which refuses expressions over
/// 1,000 deep, counts about one level per counted parenthesis for negations
/// and `and`/`or`, and about two for relation chains (themselves limited to
/// [`READ_PREDICATE_MAX_RELATION_NESTING`]), so this stays well under it.
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
    relations: u32,
    exists: u32,
    instances: u32,
    cases: u32,
    closures: u32,
}
impl Cost {
    /// Relation subqueries plus closures at their weight on `backend`.
    fn relation_weight(self, backend: DatabaseBackend) -> u32 {
        let closure = if backend == DatabaseBackend::Mssql {
            MSSQL_CLOSURE_RELATION_WEIGHT
        } else {
            CLOSURE_RELATION_WEIGHT
        };
        self.exists
            .saturating_add(self.instances.saturating_mul(closure))
    }
    /// What `clause`, rendered SQL with anonymous placeholders, costs. SQL
    /// that cannot be read with certainty is refused.
    fn of(backend: DatabaseBackend, clause: &str) -> async_graphql::Result<Self> {
        // Placeholders render as `$n` or `@Pn` on the server: count each as
        // up to seven bytes rather than the anonymous `?`.
        let placeholders = clause.bytes().filter(|byte| *byte == b'?').count();
        let Nesting {
            depth,
            units,
            subqueries: nesting,
            relations,
            exists,
            instances,
            cases,
            closures,
        } = nesting(backend, clause)?;
        Ok(Self {
            bytes: clause.len().saturating_add(placeholders.saturating_mul(6)),
            depth,
            inner: depth,
            units,
            inner_units: units,
            nesting,
            relations,
            exists,
            instances,
            cases,
            closures,
        })
    }
    fn check(self, backend: DatabaseBackend) -> async_graphql::Result<Self> {
        let mssql = backend == DatabaseBackend::Mssql;
        let (relation_nesting, relations) = if mssql {
            (MSSQL_MAX_RELATION_NESTING, MSSQL_MAX_RELATIONS)
        } else {
            (MAX_RELATION_NESTING, MAX_RELATIONS)
        };
        if self.bytes > READ_PREDICATE_MAX_RENDERED_BYTES
            || self.closures > READ_PREDICATE_CLOSURE_MAX_NESTING
            || self.depth > MAX_EXPRESSION_DEPTH
            || self.cases > MAX_CASE_NESTING
            || self.relations > relation_nesting
            || self.relation_weight(backend) > relations
            || (mssql && (self.nesting > MSSQL_MAX_NESTING || self.units > MSSQL_MAX_NESTING_UNITS))
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
    /// `EXISTS (SELECT ...)` relation subqueries.
    relations: u32,
    /// Relation subqueries in total, along every path.
    exists: u32,
    /// Closures in total, as rendered.
    instances: u32,
    /// `CASE ... END` expressions.
    cases: u32,
    /// How many distinct levels `N` open a closure (`__gom_closure_N`).
    /// A closure takes a level no closure inside it has, so this is at
    /// least how deep closures nest inside one another, and exactly that
    /// unless a name in the policy made a closure skip a level.
    closures: u32,
}

/// Measure `sql` over its tokens, so strings, delimited identifiers and
/// comments are never read as structure.
fn nesting(backend: DatabaseBackend, sql: &str) -> async_graphql::Result<Nesting> {
    let tokens = sql_tokens(backend, sql)?;
    let text = |token: &SqlToken| &sql[token.start..token.end];
    let is_word = |token: Option<&SqlToken>, word: &str| {
        token.is_some_and(|token| {
            token.kind == SqlTokenKind::Word && text(token).eq_ignore_ascii_case(word)
        })
    };
    // Per open parenthesis: (is a subquery, is a relation, weight).
    let mut open: Vec<(bool, bool, u32)> = Vec::new();
    let (mut subqueries, mut relations, mut units) = (0_u32, 0_u32, 0_u32);
    let mut deepest = Nesting {
        depth: 0,
        units: 0,
        subqueries: 0,
        relations: 0,
        exists: 0,
        instances: 0,
        cases: 0,
        closures: 0,
    };
    let mut cases = 0_u32;
    let mut levels = std::collections::BTreeSet::new();
    // The word before the current one, to recognise a closure's opening:
    // `WITH RECURSIVE __gom_closure_N` (the CTE form) or
    // `AS __gom_closure_N_g` (the SQL Server walk), once per closure.
    let mut previous = "";
    for (index, token) in tokens.iter().enumerate() {
        match (token.kind, text(token)) {
            (SqlTokenKind::Symbol, "(") => {
                let before = index.checked_sub(1).and_then(|at| tokens.get(at));
                let after = tokens.get(index + 1);
                let negation = is_word(before, "NOT");
                let subquery = is_word(after, "SELECT") || is_word(after, "WITH");
                let relation = subquery && is_word(before, "EXISTS");
                let weight = if subquery {
                    SUBQUERY_UNITS
                } else if negation {
                    NEGATION_UNITS
                } else {
                    1
                };
                subqueries += u32::from(subquery);
                relations += u32::from(relation);
                deepest.exists += u32::from(relation);
                units += weight;
                open.push((subquery, relation, weight));
                deepest.depth = deepest.depth.max(open.len() as u32);
                deepest.units = deepest.units.max(units);
                deepest.subqueries = deepest.subqueries.max(subqueries);
                deepest.relations = deepest.relations.max(relations);
            }
            (SqlTokenKind::Symbol, ")") => {
                if let Some((subquery, relation, weight)) = open.pop() {
                    subqueries -= u32::from(subquery);
                    relations -= u32::from(relation);
                    units -= weight;
                }
            }
            (SqlTokenKind::Word, word) => {
                if word.eq_ignore_ascii_case("CASE") {
                    cases += 1;
                    deepest.cases = deepest.cases.max(cases);
                } else if word.eq_ignore_ascii_case("END") {
                    cases = cases.saturating_sub(1);
                } else if let Some(rest) = word.strip_prefix(CLOSURE_ALIASES[0]) {
                    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                    let opening = (previous.eq_ignore_ascii_case("RECURSIVE")
                        && digits == rest.len())
                        || (previous.eq_ignore_ascii_case("AS") && &rest[digits..] == "_g");
                    if digits > 0 && opening {
                        deepest.instances += 1;
                        levels.insert(&rest[..digits]);
                    }
                }
                previous = word;
            }
            _ => {}
        }
    }
    deepest.closures = levels.len() as u32;
    Ok(deepest)
}

/// Columns of the governed row read by the target SQL. The lexer is
/// also used for scope allocation. The SQL itself keeps its structural row
/// references; the projection only gives those references a common row
/// source for the anchor and the reached rows.
fn closure_columns(dialect: DatabaseBackend, truth: &RowBoundSql) -> Option<Vec<String>> {
    let sql = truth.bind(READ_PREDICATE_ROW);
    let tokens = sql_tokens(dialect, &sql).ok()?;
    let text = |token: &SqlToken| &sql[token.start..token.end];
    let mut columns = std::collections::BTreeMap::new();
    for (index, token) in tokens.iter().enumerate() {
        if token.kind != SqlTokenKind::Word || text(token) != READ_PREDICATE_ROW {
            continue;
        }
        let [dot, column] = tokens.get(index + 1..index + 3)? else {
            return None;
        };
        if text(dot) != "."
            || !matches!(
                column.kind,
                SqlTokenKind::Word | SqlTokenKind::QuotedIdentifier
            )
        {
            return None;
        }
        let name = text(column);
        let column = dialect.delimited_column_identifier(name);
        if !column.is_ascii() {
            return None;
        }
        let key = delimited_name(&column);
        if let Some(previous) = columns.insert(key, column.clone()) {
            if previous != column && dialect == DatabaseBackend::Mssql {
                // SQL Server may use a case-sensitive column collation.
                return None;
            }
        }
    }
    Some(columns.into_values().collect())
}

fn closure_column_projection(columns: &[String], row: &str) -> String {
    columns
        .iter()
        .map(|column| format!(", {row}.{column} AS {column}"))
        .collect()
}

fn closure_anchor_projection(columns: &[String], origin: &str) -> RowBoundSql {
    let mut sql = RowBoundSql::sql(format!("SELECT 1 AS {origin}"));
    for column in columns {
        sql.push_str(", ");
        sql.push_row();
        sql.push_str(&format!(".{column} AS {column}"));
    }
    sql
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
/// 4. The governed row's projected columns are included with the matches
///    for the depth-zero pointer. The projection marks that row so it
///    contributes to the truth aggregate without counting as a key match.
///    If the target cannot be projected, its own truth uses a separate
///    correlated expression instead.
///
/// Every row of `table` the walk reads has its own alias: `{prefix}_hK` in
/// hop `K` and `{prefix}_t` for target matches. Each projected column comes
/// from that physical alias or the governed row. A missing pointer, key or
/// target column therefore fails the statement (#796).
///
/// The CTE form expands a pointer each time it is reached, but a later
/// expansion only reaches pointers already reached earlier, so both forms see
/// the same pointers, first depths and reached rows, and the verdict below is
/// the CTE's. Each hop's `OUTER APPLY` carries `TOP (2147483647)`, which is
/// no limit but keeps the hop a correlated lookup so the plan does not grow
/// with join reordering.
fn render_mssql_closure(
    dialect: DatabaseBackend,
    table: &str,
    prefix: &str,
    keys: &[(&str, &str)],
    quantifier: ReadPredicateQuantifier,
    truth: &RowBoundSql,
    columns_for_truth: Option<&[String]>,
    max_depth: u32,
) -> RowBoundSql {
    let columns = |render: &dyn Fn(usize, &str, &str) -> String, separator: &str| {
        keys.iter()
            .enumerate()
            .map(|(index, &(pointer, key))| render(index, pointer, key))
            .collect::<Vec<_>>()
            .join(separator)
    };
    let of = |alias: &str| columns(&|index, _, _| format!("{alias}.__gom_p{index}"), ", ");
    let names = columns(&|index, _, _| format!("__gom_p{index}"), ", ");
    // The pointer columns of the row `row`.
    let pointers = |row: &str| {
        columns(
            &|index, pointer, _| {
                format!(
                    "{row}.{} AS __gom_p{index}",
                    dialect.delimited_column_identifier(pointer)
                )
            },
            ", ",
        )
    };
    // The row `row` of `table` is one whose key equals the pointer carried
    // by `alias`.
    let named_by = |row: &str, alias: &str| {
        columns(
            &|index, _, key| {
                format!(
                    "{row}.{} = {alias}.__gom_p{index}",
                    dialect.delimited_column_identifier(key)
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
    // Level 0 reads the governed row's own pointers.
    let mut walk = RowBoundSql::sql("(SELECT ");
    for (index, (pointer, _)) in keys.iter().enumerate() {
        if index > 0 {
            walk.push_str(", ");
        }
        walk.push_row();
        walk.push_str(&format!(
            ".{} AS __gom_p{index}",
            dialect.delimited_column_identifier(pointer)
        ));
    }
    walk.push_str(&format!(", 0 AS __gom_depth) AS {}", level(0)));
    for depth in 1..=max_depth {
        let previous = level(depth - 1);
        let child = format!("{prefix}_c{depth}");
        let merged = format!("{prefix}_v{depth}");
        let hop = format!("{prefix}_h{depth}");
        let opening = format!(
            "(SELECT {merged_names}, MIN({merged}.__gom_depth) AS __gom_depth FROM ",
            merged_names = of(&merged),
        );
        let rest = format!(
            " \
             OUTER APPLY (SELECT DISTINCT TOP (2147483647) 1 AS __gom_r, {hop_pointers} \
             FROM {table} AS {hop} \
             WHERE {previous}.__gom_depth = {frontier} AND {step}) AS {child} \
             CROSS APPLY (VALUES ({previous_names}, {previous}.__gom_depth), \
             ({child_names}, CASE WHEN {child}.__gom_r IS NOT NULL THEN {depth} END)) \
             AS {merged} ({names}, __gom_depth) WHERE {merged}.__gom_depth IS NOT NULL \
             GROUP BY {merged_names}) AS {current}",
            merged_names = of(&merged),
            hop_pointers = pointers(&hop),
            frontier = depth - 1,
            step = named_by(&hop, &previous),
            previous_names = of(&previous),
            child_names = of(&child),
            current = level(depth),
        );
        walk = walk.wrapped(&opening, &rest);
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
    let reached = format!("{prefix}_r");
    let truths = format!("{prefix}_x");
    let summary = format!("{prefix}_s");
    let own = format!("{prefix}_a");
    let target = format!("{prefix}_t");
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
    if let Some(columns) = columns_for_truth {
        let input = format!("{prefix}_i");
        let origin = format!("{prefix}_origin");
        let verdict = match quantifier {
            ReadPredicateQuantifier::Any => format!(
                "CASE WHEN MAX({reached}.__gom_max) = 2 THEN 1 WHEN MAX({reached}.__gom_max) = 1 OR "
            ),
            ReadPredicateQuantifier::All => format!(
                "CASE WHEN MIN({reached}.__gom_min) = 0 THEN 0 WHEN MIN({reached}.__gom_min) = 1 OR "
            ),
        };
        let fallback = u8::from(quantifier == ReadPredicateQuantifier::All);
        let mut clause = RowBoundSql::sql(format!(
            "(SELECT {verdict}MAX(CASE WHEN {ambiguous}({followed} AND (\
             {group}.__gom_depth = {max_depth} OR {reached}.__gom_rows = 0)) THEN 1 ELSE 0 END) = 1 \
             THEN NULL ELSE {fallback} END FROM "
        ));
        clause.push(&walk);
        clause.push_str(&format!(
            " OUTER APPLY (SELECT MAX({truths}.__gom_truth) AS __gom_max, \
             MIN({truths}.__gom_truth) AS __gom_min, \
             COUNT(CASE WHEN {truths}.{origin} = 0 THEN 1 END) AS __gom_rows \
             FROM (SELECT {target_truth} AS __gom_truth, {input}.{origin} FROM (",
            target_truth = truth.bind(&input),
        ));
        clause.push(&closure_anchor_projection(columns, &origin));
        clause.push_str(&format!(
            " WHERE {group}.__gom_depth = 0 UNION ALL SELECT 0{reached_columns} \
             FROM {table} AS {target} WHERE {step} AND {group}.__gom_depth < {max_depth}\
             ) AS {input}) AS {truths}) AS {reached})",
            reached_columns = closure_column_projection(columns, &target),
            step = named_by(&target, &group),
        ));
        return clause;
    }
    // In `truths` the target is the reached row `target`. `own` has no FROM
    // and comes first, so it sees no `__gom_` column of this subquery: its
    // target is the authorized row, as the enclosing context binds it.
    let mut clause = RowBoundSql::sql(format!("(SELECT {verdict} FROM (SELECT "));
    clause.push(truth);
    clause.push_str(&format!(
        " AS __gom_truth) AS {own} CROSS APPLY (\
         SELECT MAX({reached}.__gom_max) AS __gom_max, MIN({reached}.__gom_min) AS __gom_min, \
         MAX(CASE WHEN {ambiguous}({followed} AND ({group}.__gom_depth = {max_depth} \
         OR {reached}.__gom_rows = 0)) THEN 1 ELSE 0 END) AS __gom_incomplete FROM "
    ));
    clause.push(&walk);
    clause.push_str(&format!(
        " \
         OUTER APPLY (SELECT MAX({truths}.__gom_truth) AS __gom_max, \
         MIN({truths}.__gom_truth) AS __gom_min, COUNT(*) AS __gom_rows \
         FROM (SELECT {target_truth} AS __gom_truth FROM {table} AS {target} \
         WHERE {step} AND {group}.__gom_depth < {max_depth}) AS {truths}) AS {reached}) AS {summary})",
        target_truth = truth.bind(&target),
        step = named_by(&target, &group),
    ));
    clause
}

/// SQLite and PostgreSQL walk the same pointers with a recursive CTE.
fn render_recursive_closure(
    dialect: DatabaseBackend,
    table: &str,
    cte: &str,
    keys: &[(&str, &str)],
    quantifier: ReadPredicateQuantifier,
    truth: &RowBoundSql,
    columns_for_truth: Option<&[String]>,
    max_depth: u32,
) -> RowBoundSql {
    let join = |items: Vec<String>, separator: &str| items.join(separator);
    let indexed = |render: &dyn Fn(usize, &str, &str) -> String| {
        keys.iter()
            .enumerate()
            .map(|(index, &(pointer, key))| render(index, pointer, key))
            .collect::<Vec<_>>()
    };
    let columns = join(indexed(&|index, _, _| format!("__gom_p{index}")), ", ");
    // The anchor has no FROM: its row is the one being authorized, the
    // governed row, which the enclosing context binds. The step uses
    // `reached` for the physical row. The target reads that same row,
    // either through the projection or directly through `reached`.
    // A missing column fails the statement (#796).
    let reached = format!("{cte}_t");
    let step = join(
        indexed(&|index, _, key| {
            format!(
                "{reached}.{} = {cte}.__gom_p{index}",
                dialect.delimited_column_identifier(key)
            )
        }),
        " AND ",
    );
    let named = format!("{cte}_k");
    let node = format!("{cte}_n");
    let seen_row = format!("{cte}_e");
    let followed = join(
        indexed(&|index, _, _| format!("{node}.__gom_p{index} IS NOT NULL")),
        " AND ",
    );
    let ambiguous = if keys.len() > 1 {
        format!(
            "(NOT ({followed}) AND ({})) OR ",
            join(
                indexed(&|index, _, _| format!("{node}.__gom_p{index} IS NOT NULL")),
                " OR ",
            )
        )
    } else {
        String::new()
    };
    let resolves = join(
        indexed(&|index, _, key| {
            format!(
                "{named}.{} = {node}.__gom_p{index}",
                dialect.delimited_column_identifier(key)
            )
        }),
        " AND ",
    );
    let seen = join(
        indexed(&|index, _, _| format!("{seen_row}.__gom_p{index} = {node}.__gom_p{index}")),
        " AND ",
    );
    let incomplete = format!(
        "EXISTS (SELECT 1 FROM {cte} AS {node} WHERE {ambiguous}({followed} AND (\
         NOT EXISTS (SELECT 1 FROM {table} AS {named} WHERE {resolves}) \
         OR ({node}.__gom_depth = {max_depth} AND NOT EXISTS (SELECT 1 FROM {cte} AS {seen_row} \
         WHERE {seen} AND {seen_row}.__gom_depth < {max_depth})))))"
    );
    let verdict = match quantifier {
        ReadPredicateQuantifier::Any => format!(
            "CASE WHEN MAX(__gom_truth) = 2 THEN 1 WHEN MAX(__gom_truth) = 1 OR {incomplete} THEN NULL ELSE 0 END"
        ),
        ReadPredicateQuantifier::All => format!(
            "CASE WHEN MIN(__gom_truth) = 0 THEN 0 WHEN MIN(__gom_truth) = 1 OR {incomplete} THEN NULL ELSE 1 END"
        ),
    };
    let reached_pointers = join(
        indexed(&|_, pointer, _| {
            format!("{reached}.{}", dialect.delimited_column_identifier(pointer))
        }),
        ", ",
    );
    let truth_column = if columns_for_truth.is_some() {
        ""
    } else {
        ", __gom_truth"
    };
    let mut clause = RowBoundSql::sql(format!(
        "(WITH RECURSIVE {cte} ({columns}, __gom_depth{truth_column}) AS (SELECT "
    ));
    for (index, (pointer, _)) in keys.iter().enumerate() {
        if index > 0 {
            clause.push_str(", ");
        }
        clause.push_row();
        clause.push_str(&format!(
            ".{}",
            dialect.delimited_column_identifier(pointer)
        ));
    }
    clause.push_str(", 0");
    if columns_for_truth.is_none() {
        clause.push_str(", ");
        clause.push(truth);
    }
    clause.push_str(&format!(
        " UNION SELECT {reached_pointers}, {cte}.__gom_depth + 1"
    ));
    if columns_for_truth.is_none() {
        clause.push_str(&format!(", {}", truth.bind(&reached)));
    }
    clause.push_str(&format!(
        " FROM {cte} JOIN {table} AS {reached} ON {step} \
         WHERE {cte}.__gom_depth < {max_depth}) SELECT {verdict} FROM "
    ));
    if let Some(columns) = columns_for_truth {
        // The walk needs only (pointer, depth). Joining each pointer
        // below the bound once more recovers every reached input to
        // MIN/MAX. Repeated matches do not affect these aggregates.
        let input = format!("{cte}_i");
        let truths = format!("{cte}_x");
        let origin = format!("{cte}_origin");
        clause.push_str(&format!(
            "(SELECT {} AS __gom_truth FROM (",
            truth.bind(&input)
        ));
        clause.push(&closure_anchor_projection(columns, &origin));
        clause.push_str(&format!(
            " UNION ALL SELECT 0{reached_columns} FROM {cte} JOIN {table} AS {reached} ON {step} \
             WHERE {cte}.__gom_depth < {max_depth}) AS {input}) AS {truths})",
            reached_columns = closure_column_projection(columns, &reached),
        ));
    } else {
        clause.push_str(&format!("{cte})"));
    }
    clause
}

/// `predicate`'s truth for the governed row: 2 = TRUE, 1 = UNKNOWN,
/// 0 = FALSE. `false_rows` is TRUE exactly where the predicate is FALSE.
fn truth(predicate: &RowBoundSql, false_rows: &RowBoundSql) -> RowBoundSql {
    let mut sql = predicate.wrapped("CASE WHEN (", ") THEN 2 WHEN ");
    sql.push(false_rows);
    sql.push_str(" THEN 0 ELSE 1 END");
    sql
}

/// A validated key relation from `S` to `T` with its target predicate over
/// the related row (anonymous placeholders) and bind values.
struct Relation {
    table: String,
    /// `(target key, source key)` pairs, each one quoted column.
    keys: Vec<(String, String)>,
    /// `<source key> IS NOT NULL` for every key column, over the governed
    /// (source) row.
    present_keys: RowBoundSql,
    predicate: RowBoundSql,
    values: Vec<SqlValue>,
    /// The names a subquery around `predicate`, or around the target's
    /// negation, must not reuse: those they mention and both tables.
    scope: Scope,
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
        // The target is evaluated in a correlated subquery: a column it
        // does not write through the row could resolve against an outer row.
        if target.unbound {
            return Err(PolicySqlRefusal::UnboundColumn.into());
        }
        ensure_table_name(B::DIALECT, S::TABLE_NAME)?;
        ensure_table_name(B::DIALECT, T::TABLE_NAME)?;
        let mut pairs = Vec::new();
        let mut present_keys = RowBoundSql::default();
        for (source, destination) in keys {
            if validate_column::<B, S>(source).is_err()
                || validate_column::<B, T>(destination).is_err()
            {
                return Err(invalid());
            }
            pairs.push((
                B::DIALECT.delimited_column_identifier(destination),
                B::DIALECT.delimited_column_identifier(source),
            ));
            if !present_keys.pieces.is_empty() {
                present_keys.push_str(" AND ");
            }
            present_keys.push(&row_bound_column(B::DIALECT, source));
            present_keys.push_str(" IS NOT NULL");
        }
        let (predicate, values) = target.policy.flatten();
        // The aliases of this relation enclose the target and, in the
        // relation's negation, the target's own negation: avoid the names
        // of both.
        let mut inside = predicate.bind(READ_PREDICATE_ROW);
        if let Negation::Explicit(negation, _) = &target.negation {
            inside.push(' ');
            inside.push_str(&negation.flatten().0.bind(READ_PREDICATE_ROW));
        }
        let scope = Scope::of(B::DIALECT, &inside)?
            .table(B::DIALECT, S::TABLE_NAME)
            .table(B::DIALECT, T::TABLE_NAME);
        Ok(Self {
            table: B::DIALECT.render_table_identifier_path(T::TABLE_NAME),
            keys: pairs,
            present_keys,
            predicate,
            values,
            scope,
        })
    }

    /// Correlated EXISTS over the related rows where `predicate` (over the
    /// related row) is TRUE. The related row and the derived table each get
    /// an alias that no table or alias in scope has. The source keys refer
    /// to the governed row, which the enclosing context binds.
    fn exists(&self, predicate: &RowBoundSql) -> RowBoundSql {
        let level = self.scope.level(ROW_ALIASES);
        let row = format!("{}{level}", ROW_ALIASES[0]);
        let related = format!("{}{level}", ROW_ALIASES[1]);
        let mut sql = RowBoundSql::sql(format!(
            "EXISTS (SELECT 1 FROM (SELECT * FROM {} AS {row} WHERE {}) AS {related} WHERE ",
            self.table,
            predicate.bind(&row),
        ));
        for (index, (destination, source)) in self.keys.iter().enumerate() {
            if index > 0 {
                sql.push_str(" AND ");
            }
            sql.push_str(&format!("{related}.{destination} = "));
            sql.push_row();
            sql.push_str(&format!(".{source}"));
        }
        sql.push_str(")");
        sql
    }
}

impl ReadPredicate {
    /// A predicate over `policy`, measured and checked against the
    /// composition budget.
    fn measured(
        entity: TypeId,
        backend: DatabaseBackend,
        table: Option<&'static str>,
        policy: Policy,
        closures: Option<u32>,
    ) -> async_graphql::Result<Self> {
        let (sql, _) = policy.flatten();
        let mut cost = Cost::of(backend, &sql.bind(&root_row(backend, table)))?;
        // How deep closures nest, where the caller knows it from how the
        // policy was assembled; otherwise as the rendered text shows it.
        if let Some(closures) = closures {
            cost.closures = closures;
        }
        let cost = cost.check(backend)?;
        Ok(Self {
            entity,
            backend,
            table,
            policy,
            unbound: false,
            cost,
            negation: Negation::Sql,
        })
    }

    /// A predicate over one trusted fragment, measured and checked. It is
    /// measured as the root of a statement renders it, where the governed
    /// row's name, the table path, can be longest; inside a subquery the
    /// enclosing predicate is measured again as rendered.
    fn fragment(
        entity: TypeId,
        backend: DatabaseBackend,
        table: Option<&'static str>,
        sql: RowBoundSql,
        values: Vec<SqlValue>,
        closures: u32,
    ) -> async_graphql::Result<Self> {
        let cost = Cost {
            // How deep closures nest is known from how the predicate was
            // built; the rendered text gives the rest.
            closures,
            ..Cost::of(backend, &sql.bind(&root_row(backend, table)))?
        }
        .check(backend)?;
        Ok(Self {
            entity,
            backend,
            table,
            policy: Policy::Sql { sql, values },
            unbound: false,
            cost,
            negation: Negation::Sql,
        })
    }

    /// This predicate as the root of a statement renders it: the governed
    /// row is the entity's table, which is the row the statement reads.
    fn root_expression(&self) -> FilterExpression {
        let row = root_row(self.backend, self.table);
        if let Policy::Closure { root, .. } = &self.policy {
            let (sql, values) = root.as_ref();
            return FilterExpression::trusted_fragment(sql.bind(&row), values.clone());
        }
        self.policy.expression(&row)
    }

    /// The rendering TRUE exactly where this predicate is FALSE; see
    /// [`negated`].
    fn negated(&self) -> Option<(Policy, Cost)> {
        negated(
            self.backend,
            self.table,
            &self.policy,
            self.cost,
            self.negation.clone(),
        )
    }

    /// Compare a validated persisted physical column with a bound value.
    /// Private and non-filterable columns are accepted; this does not expose
    /// them in GraphQL inputs or grant any field access. Unknown names fail closed.
    pub fn column<B: OrmBackend, T: Entity + 'static>(
        column: &str,
        op: ReadPredicateOp,
        value: SqlValue,
    ) -> async_graphql::Result<Self> {
        validate_column::<B, T>(column)?;
        ensure_table_name(B::DIALECT, T::TABLE_NAME)?;
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
            Some(T::TABLE_NAME),
            row_bound_column(B::DIALECT, column).wrapped("", &format!(" {operator} ?")),
            vec![value],
            0,
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
        validate_column::<B, T>(column)?;
        ensure_table_name(B::DIALECT, T::TABLE_NAME)?;
        Self::fragment(
            TypeId::of::<T>(),
            B::DIALECT,
            Some(T::TABLE_NAME),
            row_bound_column(B::DIALECT, column).wrapped("", &format!(" {test}")),
            Vec::new(),
            0,
        )
    }

    /// Negation: the rows where this predicate is FALSE.
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
    /// A column comparison, NULL test, generated filter or closure renders as
    /// `NOT (...)` with the inner bind values in their original order.
    /// [`Self::related`] is three-valued, so its negation never admits a row
    /// whose relation key is NULL or whose related rows leave the target
    /// UNKNOWN, also inside `and`/`or` under a `not` (De Morgan); see there
    /// for the rendering. To allow absent or matching rows, compose
    /// `exists_any.not()?.or(exists_matching)?`.
    ///
    /// Refused with `AUTHORIZATION_MISCONFIGURED`:
    /// - empty inner fragments or conjunctions;
    /// - a predicate that renders the deny fallback `1 = 0` anywhere,
    ///   including inside a relation: an empty `in_list` or `or` list, a
    ///   generated filter that fails validation (for example a list over
    ///   [`crate::graphql::filters::MAX_FILTER_LIST_OPERANDS`] operands), or
    ///   an invalid filter value. Its negation would be a blanket grant. Omit
    ///   the term, or use [`Self::is_not_null`] or
    ///   [`ReadVisibility::Unrestricted`] deliberately;
    /// - a negation over the composition budget.
    #[allow(clippy::should_implement_trait)] // Fallible policy construction.
    pub fn not(self) -> async_graphql::Result<Self> {
        let Self {
            entity,
            backend,
            table,
            policy,
            unbound,
            cost,
            negation,
        } = self;
        let (negated, negated_cost) =
            negated(backend, table, &policy, cost, negation).ok_or_else(invalid)?;
        Ok(Self {
            entity,
            backend,
            table,
            policy: negated,
            unbound,
            cost: negated_cost,
            negation: Negation::Explicit(policy, cost),
        })
    }

    /// Reject filters requiring residual Rust matching on this backend. A partial
    /// policy must supply a SQL-only prefilter and retain its row callback.
    pub fn from_filter<B: OrmBackend, F: ReadVisibilityFilter>(
        filter: &F,
    ) -> async_graphql::Result<Self> {
        if F::backend() != B::DIALECT || filter.requires_in_memory_filtering(B::DIALECT) {
            return Err(invalid());
        }
        let (policy, bound) = filter_policy(B::DIALECT, filter)?;
        let policy = policy.ok_or_else(invalid)?;
        match F::table_name() {
            Some(table) => ensure_table_name(B::DIALECT, table)?,
            // The governed row is the entity's table at the root; without
            // the table it could not be named.
            None if policy.holds_row() => return Err(PolicySqlRefusal::MissingTable.into()),
            None => {}
        }
        Ok(Self {
            unbound: !bound,
            ..Self::measured(
                F::entity_type_id(),
                B::DIALECT,
                F::table_name(),
                policy,
                None,
            )?
        })
    }

    /// A parameterized relation test over validated persisted columns: "some
    /// related row matches `target`". `keys` maps source columns to target
    /// columns; composite keys are supported. Target restrictions come from a
    /// typed target predicate. This is an explicit equality relation, not a
    /// named relation resolver: include any discriminator constraints in
    /// source/target predicates yourself. Target entity/row policies are not
    /// implicit grants; the policy owns this join.
    ///
    /// The relation is three-valued, so it composes with [`Self::not`], `and`
    /// and `or` under the same rule as every other predicate. For each source
    /// row it is:
    ///
    /// - UNKNOWN when any source key column is NULL: the relation cannot be
    ///   evaluated;
    /// - TRUE when a related row has `target` TRUE;
    /// - UNKNOWN when no related row has `target` TRUE but one has it UNKNOWN
    ///   (it might match);
    /// - FALSE otherwise, including when there is no related row at all.
    ///
    /// Only TRUE admits a row, so it renders as the plain correlated
    /// `EXISTS (<related rows where target>)`. A negation, including one that
    /// reaches the relation through `and`/`or`, renders the rows where the
    /// relation is FALSE: `<every key IS NOT NULL> AND NOT EXISTS (<related
    /// rows where target is TRUE or UNKNOWN>)`. It never admits a row whose key
    /// is NULL or whose related rows only leave `target` UNKNOWN. To admit rows
    /// with a NULL key, add [`Self::is_null`] on the key with `or`. Both forms
    /// are single correlated subqueries that use an index on the target key
    /// (PostgreSQL plans the negation as an anti-join). Each form binds the
    /// target's values once.
    ///
    /// A target that renders the deny fallback `1 = 0` (such as an empty
    /// `in_list`) is accepted here, because the admitting form only denies
    /// with it; [`Self::not`] refuses to negate such a relation.
    pub fn related<B, S, T>(keys: &[(&str, &str)], target: Self) -> async_graphql::Result<Self>
    where
        B: OrmBackend,
        S: Entity + 'static,
        T: Entity + 'static,
    {
        reserve(target.cost.bytes, 1, 1024)?;
        let closures = target.cost.closures;
        let relation = Relation::new::<B, S, T>(keys, &target)?;
        let source_row = root_row(B::DIALECT, Some(S::TABLE_NAME));
        // How deep closures nest is known from the target; the rendered
        // text, as the root of a statement over the source, gives the rest.
        let measure = |clause: &RowBoundSql| {
            Cost {
                closures,
                ..Cost::of(B::DIALECT, &clause.bind(&source_row))?
            }
            .check(B::DIALECT)
        };
        let clause = relation.exists(&relation.predicate);
        let cost = measure(&clause)?;
        // Related rows where the target is TRUE or UNKNOWN: not FALSE. The
        // target's FALSE rows are SQL over the related row, bound to that
        // row's alias like the target itself.
        let negation = match target.negated() {
            Some((false_rows, false_cost))
                if reserve(
                    false_cost.bytes,
                    1,
                    2048 + relation.present_keys.bind(&source_row).len(),
                )
                .is_ok() =>
            {
                let (false_rows, values) = false_rows.flatten();
                let mut clause = relation.present_keys.clone();
                clause.push_str(" AND NOT ");
                clause.push(
                    &relation.exists(&false_rows.wrapped("CASE WHEN ", " THEN 0 ELSE 1 END = 1")),
                );
                match measure(&clause) {
                    Ok(cost) => Negation::Explicit(
                        Policy::Sql {
                            sql: clause,
                            values,
                        },
                        cost,
                    ),
                    Err(_) => Negation::Unsupported,
                }
            }
            _ => Negation::Unsupported,
        };
        Ok(Self {
            entity: TypeId::of::<S>(),
            backend: B::DIALECT,
            table: Some(S::TABLE_NAME),
            policy: Policy::Sql {
                sql: clause,
                values: relation.values,
            },
            unbound: false,
            cost,
            negation,
        })
    }

    /// Fail-closed absence: "no related row can match `target`". Exactly
    /// `ReadPredicate::related::<B, S, T>(keys, target)?.not()`.
    ///
    /// `keys` and `target` are validated as for [`Self::related`], and as
    /// there the policy owns the join: target row policies are not imported,
    /// and a hidden target row counts like any other. The outcome reveals
    /// what `related(keys, target)` reveals plus whether some related row has
    /// `target` UNKNOWN (a hidden one included), and that case fails closed:
    /// it can deny a row, never admit one. For each source row it is:
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
    /// It renders as `<every key IS NOT NULL> AND NOT EXISTS (<related rows
    /// where target is TRUE or UNKNOWN>)` on SQLite, PostgreSQL and SQL
    /// Server, binding the target's values once. A target that cannot be
    /// negated, such as one that renders the deny fallback `1 = 0` (an empty
    /// `in_list`), is refused with `AUTHORIZATION_MISCONFIGURED`, as for
    /// [`Self::not`].
    pub fn not_related<B, S, T>(keys: &[(&str, &str)], target: Self) -> async_graphql::Result<Self>
    where
        B: OrmBackend,
        S: Entity + 'static,
        T: Entity + 'static,
    {
        Self::related::<B, S, T>(keys, target)?.not()
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
    /// to descendants instead. Each leaf has an unreachable step because its
    /// non-NULL key matches no child pointer. A walk that reaches a leaf
    /// cannot admit under `related_closure_all` or a negated
    /// `related_closure`. Composite keys are supported, and columns are
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
    /// backend. A hop bound does not cap how many rows a non-unique pointer
    /// matches. Even when the walk deduplicates pointers at each level, the
    /// join visits all fan-out matches before deduplicating. For example, a
    /// count over N rows sharing a key can process on the order of
    /// N² × `max_depth` matches. Index the key columns and prefer a unique key.
    /// Hitting the bound makes the walk incomplete: `All` is then never TRUE (it fails
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
    /// pages, search and subscription delivery re-reads. A standalone
    /// closure can share the target SQL between the governed and reached rows
    /// on SQLite and SQL Server. PostgreSQL keeps one traversal join and can
    /// use a single scalar truth test at each site. Composed policies use
    /// the original separate renderings, including the explicit FALSE test.
    /// SQLite and PostgreSQL render a
    /// `WITH RECURSIVE` common table expression. SQL Server does not allow a
    /// common table expression inside a subquery, so there the walk is
    /// unrolled into a breadth-first search with one nested derived table and
    /// one correlated `OUTER APPLY` lookup per hop. Each level keeps only the
    /// distinct pointers reached so far.
    /// Both forms admit the same rows. No backend evaluates the closure in
    /// memory or for the whole table. Other backends are refused.
    ///
    /// **Composition limits.** A closure inside a closure evaluates the inner
    /// walk for every row the outer walk reaches, so execution cost can
    /// multiply with each level. At most
    /// [`READ_PREDICATE_CLOSURE_MAX_NESTING`] closures may nest, through any
    /// relations, negations, `and` and `or`; the rendered SQL may not exceed
    /// [`READ_PREDICATE_MAX_RENDERED_BYTES`]. On SQL Server, the policy's
    /// subqueries may nest at most [`READ_PREDICATE_MSSQL_MAX_NESTING`]
    /// levels. Each `related` around a closure adds two levels, and the
    /// relation and weighted nesting budgets also apply.
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
        if !(1..=limit).contains(&max_depth) || !target.policy.negatable() {
            return Err(invalid());
        }
        // Nested closures multiply the work of the walk even when the SQL
        // shares the target expression. Keep the independent depth cap.
        if target.cost.closures >= READ_PREDICATE_CLOSURE_MAX_NESTING {
            return Err(invalid());
        }
        let walk = if B::DIALECT == DatabaseBackend::Mssql {
            max_depth as usize * (1024 + 256 * keys.len())
        } else {
            0
        };
        // The rows where the target is FALSE. `NOT (target)` is exact for a
        // target SQL negates. A three-valued relation target supplies its own
        // FALSE rows. A target that cannot be negated still decides which
        // rows the closure admits, so it keeps `NOT`, and the closure then
        // refuses negation.
        let explicit = match &target.negation {
            Negation::Explicit(policy, cost) => Some((policy.flatten(), cost.bytes)),
            Negation::Sql | Negation::Unsupported => None,
        };
        let false_bytes = explicit
            .as_ref()
            .map_or(target.cost.bytes.saturating_add(8), |(_, bytes)| *bytes);
        reserve(
            target.cost.bytes.saturating_add(false_bytes),
            2,
            4096 + walk,
        )?;
        let closures = target.cost.closures;
        let relation = Relation::new::<B, T, T>(keys, &target)?;
        if relation
            .predicate
            .bind(READ_PREDICATE_ROW)
            .trim()
            .is_empty()
        {
            return Err(invalid());
        }
        let uses_native_truth = explicit.is_none();
        let (false_rows, false_values) = match explicit {
            Some(((sql, values), _)) => (sql.wrapped("(", ")"), values),
            None => (
                relation.predicate.wrapped("NOT (", ")"),
                relation.values.clone(),
            ),
        };
        let negation = match target.negation {
            Negation::Unsupported => Negation::Unsupported,
            Negation::Sql | Negation::Explicit(..) => Negation::Sql,
        };
        let dialect = B::DIALECT;
        let cte = format!(
            "{}{}",
            CLOSURE_ALIASES[0],
            relation.scope.level(CLOSURE_ALIASES)
        );
        let render = |truth: &RowBoundSql, columns: Option<&[String]>| {
            let renderer = if dialect == DatabaseBackend::Mssql {
                render_mssql_closure
            } else {
                render_recursive_closure
            };
            renderer(
                dialect,
                &relation.table,
                &cte,
                keys,
                quantifier,
                truth,
                columns,
                max_depth,
            )
            .wrapped("", " = 1")
        };

        // Build and check exactly the original composition, including every
        // copy of the TRUE and FALSE tests. Relations, negations, other
        // closures and statement-level budgets always see this rendering.
        let legacy_truth = truth(&relation.predicate, &false_rows);
        let mut legacy_values = relation.values.clone();
        legacy_values.extend(false_values);
        let mut values = legacy_values.clone();
        values.extend_from_within(..);
        let mut predicate = Self::fragment(
            TypeId::of::<T>(),
            dialect,
            Some(T::TABLE_NAME),
            render(&legacy_truth, None),
            values,
            closures + 1,
        )?;
        predicate.negation = negation;
        if closures != 0 {
            return Ok(predicate);
        }

        // PostgreSQL retains the single traversal join. An extra join after
        // the walk regressed nested queries, and is unnecessary when SQL can
        // encode TRUE, FALSE and UNKNOWN directly at each evaluation site.
        let (root_sql, root_values) = match dialect {
            DatabaseBackend::Postgres if uses_native_truth => {
                let native = relation
                    .predicate
                    .wrapped("COALESCE(2 * ((", ")::integer), 1)");
                let mut values = relation.values;
                values.extend_from_within(..);
                (render(&native, None), values)
            }
            DatabaseBackend::Postgres => return Ok(predicate),
            _ => {
                let (truth, values) = if dialect == DatabaseBackend::Sqlite && uses_native_truth {
                    (
                        relation
                            .predicate
                            .wrapped("COALESCE(2 * (NOT NOT (", ")), 1)"),
                        relation.values,
                    )
                } else {
                    (legacy_truth, legacy_values)
                };
                let Some(columns) = closure_columns(dialect, &truth) else {
                    return Ok(predicate);
                };
                (render(&truth, Some(&columns)), values)
            }
        };
        // The executable alternative must also meet the SQL parser limits.
        if Cost::of(
            dialect,
            &root_sql.bind(&root_row(dialect, Some(T::TABLE_NAME))),
        )
        .and_then(|cost| cost.check(dialect))
        .is_ok()
            && let Policy::Sql { sql, values } = predicate.policy
        {
            predicate.policy = Policy::Closure {
                sql,
                values,
                root: std::sync::Arc::new((root_sql, root_values)),
            };
        }
        Ok(predicate)
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
        let backend = self.backend;
        // One entity has one table; a side without it refers to no
        // governed row.
        let table = self.table.or(other.table);
        let unbound = self.unbound || other.unbound;
        // De Morgan: `a AND b` is FALSE where either is FALSE, `a OR b` where
        // both are. Two SQL-negatable operands stay SQL-negatable, so nothing
        // is rendered for an ordinary fold.
        let negation = match (self.negation, other.negation) {
            (Negation::Sql, Negation::Sql) => Negation::Sql,
            (left, right) => match (
                negated(backend, table, &self.policy, self.cost, left),
                negated(backend, table, &other.policy, other.cost, right),
            ) {
                (Some(left), Some(right)) => {
                    let (policy, cost) = join(left, right, !and);
                    match cost.check(backend) {
                        Ok(cost) => Negation::Explicit(policy, cost),
                        Err(_) => Negation::Unsupported,
                    }
                }
                _ => Negation::Unsupported,
            },
        };
        let (policy, cost) = join((self.policy, self.cost), (other.policy, other.cost), and);
        Ok(Self {
            entity: self.entity,
            backend,
            table,
            unbound,
            policy,
            cost: cost.check(backend)?,
            negation,
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

/// The most relations (`EXISTS (SELECT ...)` subqueries, closures at
/// [`READ_PREDICATE_CLOSURE_RELATION_WEIGHT`], and one per relation-count
/// ordering) that the generated relation filters and count orderings of one
/// statement may add together on SQLite and PostgreSQL. With the entity's
/// own policy (at most [`READ_PREDICATE_MAX_RELATIONS`]), a statement holds
/// at most 192. On a PostgreSQL server capped at 4 GB, every shape probed at
/// these limits ran, peaking at about 470 MB of server memory; the slowest
/// took about 8 s on 60 rows, so set `statement_timeout` as well.
pub const READ_PREDICATE_MAX_STATEMENT_RELATIONS: u32 = MAX_STATEMENT_RELATIONS;
const MAX_STATEMENT_RELATIONS: u32 = 64;

/// As [`READ_PREDICATE_MAX_STATEMENT_RELATIONS`], on SQL Server. With the
/// entity's own policy (at most [`READ_PREDICATE_MSSQL_MAX_RELATIONS`]), a
/// statement holds at most 14. With these limits lifted, statements on the
/// owned server failed with error 8623 from 29 relations (deep relation
/// chains on both sides) and 32 (relations to many children combined with
/// `and`), and closure-heavy statements from 28; the limit leaves twice that
/// margin. Plan-resource errors on untested shapes remain
/// possible: they fail the read.
pub const READ_PREDICATE_MSSQL_MAX_STATEMENT_RELATIONS: u32 = MSSQL_MAX_STATEMENT_RELATIONS;
const MSSQL_MAX_STATEMENT_RELATIONS: u32 = 5;

/// The most terms a generated `orderBy` list may hold. Each relation-count
/// term adds a correlated `COUNT(*)` subquery; a longer list is refused as
/// invalid input.
pub const MAX_ORDER_BY_TERMS: usize = 16;

/// Per-statement budget threaded through generated where-input and ordering
/// preparation: where-input nodes left, and relation policy SQL added so far.
#[doc(hidden)]
#[derive(Debug)]
pub struct RelationFilterBudget {
    nodes: usize,
    bytes: usize,
    relations: u32,
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
            relations: 0,
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
        self.charge_with::<B>(fragment, 0)
    }
    /// As [`Self::charge`], adding `extra` relations: a count ordering's own
    /// correlated `COUNT(*)` subquery counts as one.
    fn charge_with<B: OrmBackend>(
        &mut self,
        fragment: &str,
        extra: u32,
    ) -> async_graphql::Result<()> {
        let cost = Cost::of(B::DIALECT, fragment)?.check(B::DIALECT)?;
        self.bytes = self.bytes.saturating_add(cost.bytes);
        self.relations = self
            .relations
            .saturating_add(cost.relation_weight(B::DIALECT))
            .saturating_add(extra);
        let relations = if B::DIALECT == DatabaseBackend::Mssql {
            MSSQL_MAX_STATEMENT_RELATIONS
        } else {
            MAX_STATEMENT_RELATIONS
        };
        if self.bytes > READ_PREDICATE_MAX_STATEMENT_RELATION_BYTES || self.relations > relations {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Internal authorization bridge for generated static collection filters.
/// Only complete SQL visibility is accepted; callback policies fail closed.
///
/// The returned clauses refer to the source row as a value: the generated
/// filter binds each one with [`RowBoundSql::bind`] to the row it is
/// rendered for.
#[doc(hidden)]
pub async fn prepare_static_relation_filter<B, S, T, F>(
    ctx: &async_graphql::Context<'_>,
    keys: &[(&str, &str)],
    alternatives: &[F],
    budget: &mut RelationFilterBudget,
) -> async_graphql::Result<(Vec<RowBoundSql>, Vec<SqlValue>)>
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
    let mut policies = Vec::new();
    for filter in alternatives {
        filter.validate().map_err(query_error)?;
        if filter.requires_in_memory_filtering(B::DIALECT) {
            return Err(input_error());
        }
        super::authorize_read_fields::<T, B>(db, ctx, filter.referenced_fields()).await?;
        let (policy, bound) = filter_policy(B::DIALECT, filter)?;
        if !bound {
            return Err(PolicySqlRefusal::UnboundColumn.into());
        }
        policies.push(policy.unwrap_or_else(|| Policy::sql("1 = 1")));
    }
    let mut policy = if policies.is_empty() {
        Policy::sql("1 = 0")
    } else {
        Policy::Or(policies)
    };
    // The alternatives are generated filters, whose nested relation clauses
    // are rendered text: their closure depth is read from it. The target's
    // own policy carries its depth from how it was built.
    let mut closures = Cost::of(B::DIALECT, &policy.flatten().0.bind(READ_PREDICATE_ROW))?.closures;
    if let Some(target) = visibility.row_predicate() {
        // The target's own policy is evaluated in the relation's subquery.
        if target.unbound {
            return Err(PolicySqlRefusal::UnboundColumn.into());
        }
        closures = closures.max(target.cost.closures);
        policy = Policy::And(vec![policy, target.policy.clone()]);
    }
    let predicate = ReadPredicate::related::<B, S, T>(
        keys,
        ReadPredicate::measured(
            TypeId::of::<T>(),
            B::DIALECT,
            Some(T::TABLE_NAME),
            policy,
            Some(closures),
        )?,
    )?;
    // Keep anonymous placeholders in this composable fragment. EntityQuery
    // rebases them after preceding scalar/policy binds using B::DIALECT.
    // Identifiers inside the EXISTS are already quoted by ReadPredicate::related.
    // The source keys refer to the governed row: the generated filter binds
    // them to the source table at the root of a statement, or to the row an
    // enclosing read predicate renders it for (#796).
    let (clause, values) = predicate.policy.flatten();
    if values.len() > 512 {
        return Err(input_error());
    }
    budget.charge::<B>(&clause.bind(&B::DIALECT.render_table_identifier_path(S::TABLE_NAME)))?;
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
/// `source` is the source entity's declared table, which the statement
/// reads unaliased. `keys` are `(target column, source column)` pairs as
/// generated SQL renders them; the counted rows are joined to the source row
/// through an alias allocated here, which differs from every table and
/// alias in scope.
#[doc(hidden)]
pub async fn prepare_relation_count_order<B, T>(
    ctx: &async_graphql::Context<'_>,
    source: &str,
    keys: &[(&str, &str)],
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
    let (count, values) = relation_count_sql(
        B::DIALECT,
        source,
        T::TABLE_NAME,
        keys,
        visibility.row_predicate(),
    )?;
    budget.charge_with::<B>(&count, 1)?;
    Ok((count, values))
}

/// The correlated `COUNT(*)` over the rows of `target` that `policy` admits,
/// for a statement that reads `source` unaliased. `keys` are
/// `(target column, source column)` pairs as generated SQL renders them.
fn relation_count_sql(
    backend: DatabaseBackend,
    source: &str,
    target: &str,
    keys: &[(&str, &str)],
    policy: Option<&ReadPredicate>,
) -> async_graphql::Result<(String, Vec<SqlValue>)> {
    ensure_table_name(backend, source)?;
    ensure_table_name(backend, target)?;
    if keys.is_empty() {
        return Err(invalid());
    }
    let table = backend.render_identifier_path(target);
    let source_row = backend.render_identifier_path(source);
    // The counted rows and, under a policy, the row the policy governs
    // each get an alias that no table or alias in scope has: not the
    // source table the statement reads, the counted table, or any name
    // inside the policy. A key or policy column the counted table lacks
    // then fails the statement (#796).
    let count = |scope: Scope, from: &dyn Fn(&str) -> String| {
        let level = scope
            .table(backend, source)
            .table(backend, target)
            .level(ROW_ALIASES);
        let counted = format!("{}{level}", ROW_ALIASES[1]);
        let join = keys
            .iter()
            .map(|(target_key, source_key)| {
                format!("{counted}.{target_key} = {source_row}.{source_key}")
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        format!(
            "(SELECT COUNT(*) FROM {} AS {counted} WHERE {join})",
            from(&format!("{}{level}", ROW_ALIASES[0]))
        )
    };
    let Some(policy) = policy else {
        // Unrestricted target: the count subquery still costs a relation.
        return Ok((
            count(Scope::of(backend, "")?, &|_| table.to_string()),
            Vec::new(),
        ));
    };
    // The policy is evaluated in the count's subquery, correlated with the
    // source row.
    if policy.unbound {
        return Err(PolicySqlRefusal::UnboundColumn.into());
    }
    // Anonymous placeholders: the sort renderer renumbers them per dialect
    // after the WHERE binds, as for contextual order expressions.
    let (predicate, values) = policy.policy.flatten();
    let rendered = predicate.bind(READ_PREDICATE_ROW);
    if rendered.trim().is_empty() {
        return Err(invalid());
    }
    if values.len() > 512 {
        return Err(input_error());
    }
    // The count wraps the policy in two more subquery levels: check the
    // wrapped expression, as for a relation filter.
    reserve(rendered.len(), 1, 1024)?;
    Ok((
        count(Scope::of(backend, &rendered)?, &|row| {
            format!(
                "(SELECT * FROM {table} AS {row} WHERE {})",
                predicate.bind(row)
            )
        }),
        values,
    ))
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
    /// The SQL predicate as the root of a statement over the entity's table
    /// renders it, its row qualifiers bound to that table.
    pub(crate) fn predicate_expression(&self) -> Option<FilterExpression> {
        match self {
            Self::Prefilter(predicate) | Self::Complete(predicate) => {
                Some(predicate.root_expression())
            }
            _ => None,
        }
    }
    /// The SQL predicate over the governed row, to be embedded where a
    /// subquery binds the row.
    fn row_predicate(&self) -> Option<&ReadPredicate> {
        match self {
            Self::Prefilter(predicate) | Self::Complete(predicate) => Some(predicate),
            _ => None,
        }
    }
    pub fn requires_residual_checks(&self) -> bool {
        matches!(self, Self::CallbackOnly | Self::Prefilter(_))
    }
    pub(crate) fn validate<T: Entity + 'static, B: OrmBackend>(&self) -> async_graphql::Result<()> {
        // A predicate attached to a statement over `T` names its rows by
        // `T`'s table: a hand-built entity over a generated name is refused
        // here too, whatever built the predicate.
        self.validate_table(B::DIALECT, T::TABLE_NAME)?;
        self.validate_identity(Some(TypeId::of::<T>()), B::DIALECT)
    }
    /// `Err` when a predicate is attached to a statement over `table` and
    /// the table's own name is one read predicates generate.
    pub(crate) fn validate_table(
        &self,
        backend: DatabaseBackend,
        table: &str,
    ) -> async_graphql::Result<()> {
        if matches!(self, Self::Prefilter(_) | Self::Complete(_)) {
            ensure_table_name(backend, table)?;
        }
        Ok(())
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
    /// The schema's operation catalogue could not be composed.
    OperationCatalog(GraphqlOperationCatalogError),
}

/// A generated Keyset root reported by the read-policy check, or an operation
/// catalogue error returned before the check can run.
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
    /// The reported GraphQL root field, or the empty string for a catalogue error.
    pub const fn field_name(&self) -> &'static str {
        self.field_name
    }
    /// The entity the root pages through, or the empty string for a catalogue error.
    pub const fn entity_name(&self) -> &'static str {
        self.entity_name
    }
    /// The entity's read policy key, if it declares one.
    pub const fn policy_key(&self) -> Option<&'static str> {
        self.policy_key
    }
    /// Why the root was reported or why the check could not run.
    pub const fn reason(&self) -> KeysetReadPolicyReason {
        self.reason
    }
    /// Whether generated schema builders refuse registration for this reason.
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
            KeysetReadPolicyReason::OperationCatalog(error) => error.fmt(f),
        }
    }
}

impl From<GraphqlOperationCatalogError> for KeysetReadPolicyError {
    fn from(error: GraphqlOperationCatalogError) -> Self {
        Self {
            field_name: "",
            entity_name: "",
            policy_key: None,
            reason: KeysetReadPolicyReason::OperationCatalog(error),
        }
    }
}

impl std::error::Error for KeysetReadPolicyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.reason {
            KeysetReadPolicyReason::OperationCatalog(error) => Some(error),
            _ => None,
        }
    }
}

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
            // The policy keeps its own bind values, apart from the caller's
            // clauses and values. A statement composes them when it is
            // rendered, after validating the caller's on their own.
            self.attach_policy(p.root_expression());
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
        let parameter_type = |column: &str| {
            if B::DIALECT == DatabaseBackend::Mssql {
                mssql_parameter_type::<T>(column)
            } else {
                None
            }
        };
        let render = if before {
            super::query::render_keyset_before_with_types
        } else {
            super::query::render_keyset_after_with_types
        };
        let (mut clause, mut binds) = render(
            B::DIALECT,
            order,
            values,
            self.values.len() + 1,
            parameter_type,
        )
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
                    binds.push(
                        super::keyset_sql_value(value)
                            .ok_or_else(input_error)?
                            .with_mssql_type(parameter_type(column.column).unwrap_or("")),
                    );
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
        match (ctx, requested_limit) {
            // A trusted Rust caller: a limit above the maximum is an error,
            // never a shorter page than the caller asked for.
            (None, Some(requested)) => {
                page_config
                    .checked_explicit_limit(requested)
                    .map_err(|error| error.into_graphql_error())?;
            }
            // A GraphQL client: reduced, or refused when the host opted in.
            _ => page_config
                .admit_client_limit(requested_limit)
                .map_err(|error| error.into_graphql_error())?,
        }
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
            query.order_values.clear();
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
        let page_config = db
            .client_pagination_config()
            .with_read_maximum(self.max_limit);
        if let (None, Some(requested)) = (ctx, page.limit) {
            // A trusted Rust caller's limit is honoured exactly or refused.
            page_config
                .checked_explicit_limit(requested)
                .map_err(|error| error.into_graphql_error())?;
        }
        let limit = page_config
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

    fn cost_of(clause: &str) -> Cost {
        Cost::of(DatabaseBackend::Sqlite, clause).unwrap()
    }

    /// A policy over trusted clauses that are not read as hand-written SQL.
    fn raw_policy(expression: &FilterExpression) -> Policy {
        match expression {
            FilterExpression::Raw { clause, values }
            | FilterExpression::TrustedFragment { clause, values } => Policy::Sql {
                sql: RowBoundSql::sql(clause.as_str()),
                values: values.clone(),
            },
            FilterExpression::And(children) => {
                Policy::And(children.iter().map(raw_policy).collect())
            }
            FilterExpression::Or(children) => Policy::Or(children.iter().map(raw_policy).collect()),
        }
    }

    const BACKENDS: [DatabaseBackend; 3] = [
        DatabaseBackend::Sqlite,
        DatabaseBackend::Postgres,
        DatabaseBackend::Mssql,
    ];

    /// The public code of a refusal.
    fn code(error: &async_graphql::Error) -> String {
        error
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code"))
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    #[test]
    fn hand_written_qualifiers_become_row_references() {
        use DatabaseBackend::{Mssql, Postgres, Sqlite};
        // The clause bound to the row `R`, and whether every column in it
        // is written through the qualifier.
        let bind = |backend, clause: &str| {
            handwritten_sql(backend, clause, clause.matches('?').count())
                .map(|(sql, bound)| (sql.bind("R"), bound))
        };
        for backend in BACKENDS {
            // Only the bare word before a dot is the qualifier, matched
            // without ASCII case: not text in a string.
            assert_eq!(
                bind(
                    backend,
                    "__gom_self.a = ? AND __GOM_SELF . b <> '__gom_self.c' AND __gom_self.d IS NOT NULL"
                ),
                Ok((
                    "R.a = ? AND R . b <> '__gom_self.c' AND R.d IS NOT NULL".to_owned(),
                    true
                )),
                "{backend:?}"
            );
            // A physical column named like the qualifier keeps its name.
            assert_eq!(
                bind(backend, "__gom_self.__gom_self = 1"),
                Ok(("R.__gom_self = 1".to_owned(), true)),
                "{backend:?}"
            );
            // The expression forms a single-row test is written with.
            for clause in [
                "__gom_self.a IN (?, ?) OR NOT (__gom_self.b BETWEEN 1 AND 2.5)",
                "LOWER(__gom_self.a) LIKE ? ESCAPE '\\' AND __gom_self.b IS NULL",
                "CASE WHEN __gom_self.a > 0 THEN 1 ELSE 0 END = 1",
                // Constants that need no row, and numbers with no whole part.
                "__gom_self.expires_at > CURRENT_TIMESTAMP AND __gom_self.a > .5",
                "__gom_self.a IN (?, ?) AND __gom_self.b IN (LOWER(?), 'x')",
                "CAST(__gom_self.a AS INTEGER) = 1 AND COALESCE(__gom_self.b, 0) = 0",
                "1 = 1",
            ] {
                assert_eq!(
                    bind(backend, clause).map(|(_, bound)| bound),
                    Ok(true),
                    "{backend:?}: {clause}"
                );
            }
            // A column not written through the qualifier leaves the clause
            // unbound: bare, delimited, or qualified by anything else.
            for clause in [
                "a = 1",
                "__gom_self.a = b",
                "\"a\" = 1",
                "t.a = 1",
                "sources.a = __gom_self.a",
                "t.__gom_self = 1",
                "__gom_self.a = 1 AND x.y.z = 2",
            ] {
                assert_eq!(
                    bind(backend, clause).map(|(_, bound)| bound),
                    Ok(false),
                    "{backend:?}: {clause}"
                );
            }
            // Refused whatever the context.
            for (clause, refusal) in [
                // A subquery, or an alias of its own: the reviewer's probes.
                (
                    "EXISTS (SELECT 1 FROM lookup AS '__gom_row_1' WHERE '__gom_row_1'.id = __gom_self.id)",
                    PolicySqlRefusal::Keyword,
                ),
                (
                    "EXISTS (SELECT 1 FROM lookup AS sources WHERE sources.id = __gom_self.id)",
                    PolicySqlRefusal::Keyword,
                ),
                ("__gom_self.a IN (select 1)", PolicySqlRefusal::Keyword),
                ("__gom_self.a IS DISTINCT FROM 1", PolicySqlRefusal::Keyword),
                ("__gom_self.a = 1 JOIN t", PolicySqlRefusal::Keyword),
                ("WITH c AS (x) y", PolicySqlRefusal::Keyword),
                ("__gom_self.a = 1 UNION x", PolicySqlRefusal::Keyword),
                ("__gom_self.a = 1 INTERSECT x", PolicySqlRefusal::Keyword),
                ("__gom_self.a = 1 EXCEPT x", PolicySqlRefusal::Keyword),
                ("__gom_self.a = (VALUES (1))", PolicySqlRefusal::Keyword),
                ("__gom_self.a = (TABLE t)", PolicySqlRefusal::Keyword),
                // A clause that would close its own group, or the group it
                // is composed into, or end the statement.
                ("__gom_self.a = 1) OR (1 = 1", PolicySqlRefusal::Unbalanced),
                ("__gom_self.a = 1)", PolicySqlRefusal::Unbalanced),
                ("(__gom_self.a = 1", PolicySqlRefusal::Unbalanced),
                ("__gom_self.a = 1; x", PolicySqlRefusal::Unbalanced),
                // A string where an identifier stands.
                ("'__gom_row_1'.a = 1", PolicySqlRefusal::StringIdentifier),
                ("__gom_self.'a' = 1", PolicySqlRefusal::StringIdentifier),
                // Whitespace only the database would read as such.
                (
                    "\u{feff}__gom_self.a = 1",
                    PolicySqlRefusal::HiddenWhitespace,
                ),
                ("__gom_self.a\u{a0}= 1", PolicySqlRefusal::HiddenWhitespace),
                (
                    "__gom_self.a = 1 AND\u{2003}b = 2",
                    PolicySqlRefusal::HiddenWhitespace,
                ),
                // Comments, closed or not.
                ("__gom_self.a = 1 /* it's */", PolicySqlRefusal::Comment),
                (
                    "__gom_self.a = 1 -- it's\n AND __gom_self.b = 2",
                    PolicySqlRefusal::Comment,
                ),
                ("__gom_self.a = 1 -- to the end", PolicySqlRefusal::Comment),
                ("__gom_self.a = 1 /* open", PolicySqlRefusal::Comment),
                // String continuation.
                (
                    "__gom_self.a = 'x' 'y'",
                    PolicySqlRefusal::StringContinuation,
                ),
                (
                    "__gom_self.a = 'x'\n  'y'",
                    PolicySqlRefusal::StringContinuation,
                ),
                // Delimited spellings of generated names.
                ("\"__gom_self\".a = 1", PolicySqlRefusal::ReservedName),
                ("\"__GOM_ROW_1\".a = 1", PolicySqlRefusal::ReservedName),
                (
                    "\"__gom_closure_2_t\".a = 1",
                    PolicySqlRefusal::ReservedName,
                ),
                // The qualifier with no column after it.
                ("__gom_self = 1", PolicySqlRefusal::AmbiguousRow),
                ("f(__gom_self)", PolicySqlRefusal::AmbiguousRow),
                // Unreadable SQL.
                ("__gom_self.a = 'open", PolicySqlRefusal::Unreadable),
                ("__gom_self.a = \"open", PolicySqlRefusal::Unreadable),
            ] {
                assert_eq!(bind(backend, clause), Err(refusal), "{backend:?}: {clause}");
            }
            // Keywords inside a string are text.
            assert_eq!(
                bind(backend, "__gom_self.a = 'SELECT x FROM y -- z'").map(|(_, bound)| bound),
                Ok(true),
                "{backend:?}"
            );
        }
        // A word is a keyword only where its dialect reads it as one, in
        // the position it reads it as one. Anywhere else it could name a
        // column, so it leaves the clause unbound.
        for (backends, clause, bound) in [
            // `UNKNOWN` is a PostgreSQL truth value after `IS [NOT]` only.
            (
                &BACKENDS[..],
                "__gom_self.active = 1 AND unknown = 0",
                false,
            ),
            (&[Postgres][..], "(__gom_self.a = 1) IS NOT UNKNOWN", true),
            (&[Postgres], "(__gom_self.a = 1) IS UNKNOWN", true),
            (&[Sqlite, Mssql], "(__gom_self.a = 1) IS UNKNOWN", false),
            // Operators where no operand precedes them.
            (&BACKENDS, "like = 1", false),
            (&BACKENDS, "__gom_self.a = end", false),
            (&BACKENDS, "__gom_self.a = 1 AND escape = 2", false),
            (&BACKENDS, "between > 1", false),
            (&BACKENDS, "NOT glob", false),
            // Operators of another dialect.
            (
                &[Postgres],
                "__gom_self.a ILIKE ? AND __gom_self.b SIMILAR TO 'x'",
                true,
            ),
            (&[Sqlite, Mssql], "__gom_self.a ILIKE ?", false),
            (&[Sqlite, Mssql], "__gom_self.b SIMILAR TO 'x'", false),
            (
                &[Sqlite],
                "__gom_self.a GLOB ? OR __gom_self.b NOT REGEXP 'x'",
                true,
            ),
            (&[Postgres, Mssql], "__gom_self.a GLOB ?", false),
            (
                &[Sqlite, Postgres],
                "__gom_self.a ISNULL OR __gom_self.b NOTNULL",
                true,
            ),
            (&[Mssql], "__gom_self.a NOTNULL", false),
            // Quantifiers and casts are calls, never bare words.
            (&BACKENDS, "__gom_self.a = ANY (?)", true),
            (&BACKENDS, "__gom_self.a = any", false),
            (&BACKENDS, "cast = 1", false),
            // The last part of a qualified name is an identifier, whatever
            // it spells: an operator may follow it, and no number starts
            // after it.
            (&[Sqlite], "__gom_self.glob GLOB ?", true),
            (
                &BACKENDS,
                "__gom_self.like = ? AND __gom_self.in IN (1) AND __gom_self.not LIKE ?",
                true,
            ),
            (
                &BACKENDS,
                "__gom_self.between BETWEEN 1 AND 2 AND __gom_self.escape NOT IN (3)",
                true,
            ),
            (&BACKENDS, "__gom_self.like.5 > 1", false),
            (&BACKENDS, "__gom_self.a > t.between.5", false),
            // The ordinary positions.
            (
                &BACKENDS,
                "__gom_self.a NOT LIKE ? ESCAPE '!' AND __gom_self.b NOT IN (1, 2) \
                 AND __gom_self.c NOT BETWEEN 1 AND 2 AND (__gom_self.d) IN (3)",
                true,
            ),
            (
                &BACKENDS,
                "CASE WHEN __gom_self.a IS NOT NULL THEN __gom_self.b ELSE 0 END IN (1) \
                 AND 'x' LIKE ? AND ? BETWEEN 1 AND 2",
                true,
            ),
            // A number may start with its dot after a keyword or operator.
            (&BACKENDS, "__gom_self.score BETWEEN .5 AND .8", true),
            (
                &BACKENDS,
                "__gom_self.score IN (.5, -.25) OR __gom_self.score > .1",
                true,
            ),
            (
                &BACKENDS,
                "CASE WHEN __gom_self.a > 0 THEN .5 ELSE .25 END > __gom_self.b",
                true,
            ),
            // Not after a name, and not across whitespace.
            (&BACKENDS, "__gom_self.score > t.5", false),
            (&BACKENDS, "__gom_self.score BETWEEN . 5 AND 1", false),
        ] {
            for backend in backends {
                assert_eq!(
                    bind(*backend, clause).map(|(_, bound)| bound),
                    Ok(bound),
                    "{backend:?}: {clause}"
                );
            }
        }
        // Placeholders: only the anonymous `?`, one per bind value.
        for backend in BACKENDS {
            for clause in [
                "__gom_self.owner_id = ?1",
                "__gom_self.owner_id = $1",
                "__gom_self.owner_id = @p1",
                "__gom_self.owner_id = @P2 AND __gom_self.a = ?",
            ] {
                assert_eq!(
                    handwritten_sql(backend, clause, 1).map(|_| ()),
                    Err(PolicySqlRefusal::NamedPlaceholder),
                    "{backend:?}: {clause}"
                );
            }
            // More placeholders than values, and fewer.
            for (clause, binds) in [
                ("__gom_self.owner_id = ?", 0),
                ("__gom_self.owner_id = ? AND __gom_self.a = ?", 1),
                ("__gom_self.owner_id = 1", 1),
                ("__gom_self.owner_id = ?", 2),
            ] {
                assert_eq!(
                    handwritten_sql(backend, clause, binds).map(|_| ()),
                    Err(PolicySqlRefusal::PlaceholderCount),
                    "{backend:?}: {clause} with {binds}"
                );
            }
            // A `?` in a string or a delimited name is not a placeholder.
            assert_eq!(
                handwritten_sql(backend, "__gom_self.a = '?' AND __gom_self.\"b?\" = ?", 1)
                    .map(|(sql, bound)| (sql.bind("R"), bound)),
                Ok(("R.a = '?' AND R.\"b?\" = ?".to_owned(), true)),
                "{backend:?}"
            );
        }
        // A filter's conditions share one flat bind list: its total must
        // match before the list is divided among them.
        for backend in BACKENDS {
            let conditions = [
                "__gom_self.tenant_id = ?".to_owned(),
                "__gom_self.owner_id = ? AND __gom_self.note <> '?' AND \"a?\" IS NULL".to_owned(),
            ];
            assert_eq!(
                ensure_bind_count(backend, &conditions, 2),
                Ok(()),
                "{backend:?}"
            );
            for binds in [0, 1, 3] {
                assert_eq!(
                    ensure_bind_count(backend, &conditions, binds),
                    Err(PolicySqlRefusal::PlaceholderCount),
                    "{backend:?}: {binds}"
                );
            }
            assert_eq!(
                ensure_bind_count(backend, &["a = 'open".to_owned()], 0),
                Err(PolicySqlRefusal::Unreadable),
                "{backend:?}"
            );
        }
        // Named parameters and variables, as each dialect spells them.
        for (backend, clause) in [
            (Sqlite, "__gom_self.owner_id = :owner"),
            (Sqlite, "__gom_self.owner_id = @owner"),
            (Sqlite, "__gom_self.owner_id = $owner"),
            (Mssql, "__gom_self.owner_id = @owner"),
            (Mssql, "__gom_self.owner_id = @@spid"),
            (Postgres, "__gom_self.owner_id = $owner"),
        ] {
            assert_eq!(
                handwritten_sql(backend, clause, 0).map(|_| ()),
                Err(PolicySqlRefusal::NamedPlaceholder),
                "{backend:?}: {clause}"
            );
        }
        // Non-ASCII text in a string is text.
        assert_eq!(
            bind(Sqlite, "__gom_self.a = '\u{feff}\u{a0}'").map(|(_, bound)| bound),
            Ok(true)
        );
        // `TRUE` and `FALSE` are literals where the dialect has them; on
        // SQL Server they would be column names.
        for (backend, bound) in [(Sqlite, true), (Postgres, true), (Mssql, false)] {
            assert_eq!(
                bind(backend, "__gom_self.a = TRUE OR __gom_self.b = false").map(|(_, b)| b),
                Ok(bound),
                "{backend:?}"
            );
        }
        // Scalar constants, per dialect.
        for (backend, clause, bound) in [
            (Mssql, "__gom_self.a = CURRENT_USER", true),
            (Postgres, "__gom_self.a < LOCALTIMESTAMP", true),
            (Sqlite, "__gom_self.a = CURRENT_USER", false),
            (Mssql, "__gom_self.a < CURRENT_DATE", false),
            (Sqlite, "__gom_self.a < CURRENT_DATE", true),
        ] {
            assert_eq!(
                bind(backend, clause).map(|(_, b)| b),
                Ok(bound),
                "{backend:?}: {clause}"
            );
        }
        // A fraction needs a number, not a name, before its dot.
        assert_eq!(
            bind(Postgres, "__gom_self.a > f(1).5").map(|(_, b)| b),
            Ok(false)
        );
        // SQL Server `#` names; PostgreSQL unmatched subscripts.
        for clause in ["#t.a = 1", "__gom_self.a = #b"] {
            assert_eq!(
                bind(Mssql, clause),
                Err(PolicySqlRefusal::TemporaryName),
                "{clause}"
            );
        }
        for clause in [
            "__gom_self.t[1 = 1",
            "__gom_self.t 1] = 1",
            "(__gom_self.t[1) ]",
        ] {
            assert_eq!(
                bind(Postgres, clause),
                Err(PolicySqlRefusal::Unbalanced),
                "{clause}"
            );
        }
        // SQLite: `IN` over a table or a table-valued function.
        for clause in [
            "__gom_self.a IN t",
            "__gom_self.a NOT IN main.t",
            "__gom_self.a IN (json_each(?))",
            "__gom_self.a IN (generate_series(1, 9))",
        ] {
            assert_eq!(
                bind(Sqlite, clause),
                Err(PolicySqlRefusal::TableOperand),
                "{clause}"
            );
        }
        // PostgreSQL: `U&` strings and identifiers, and an unclosed dollar
        // quote; dollar-quoted and escape strings are text, an array
        // subscript is code.
        for clause in ["__gom_self.a = U&'\\0061'", "u&\"\\0061\" = 1"] {
            assert_eq!(
                bind(Postgres, clause),
                Err(PolicySqlRefusal::UnicodeEscape),
                "{clause}"
            );
        }
        assert_eq!(
            bind(Postgres, "__gom_self.a = $q$ open"),
            Err(PolicySqlRefusal::Unreadable)
        );
        assert_eq!(
            bind(
                Postgres,
                "__gom_self.a = $q$ __gom_self.b $q$ AND __gom_self.c = E'\\'__gom_self.d' AND __gom_self.tags[1] = __gom_self.i::text"
            ),
            Ok((
                "R.a = $q$ __gom_self.b $q$ AND R.c = E'\\'__gom_self.d' AND R.tags[1] = R.i::text"
                    .to_owned(),
                true
            ))
        );
        // SQL Server and SQLite: brackets delimit a name.
        for backend in [Mssql, Sqlite] {
            assert_eq!(
                bind(backend, "__gom_self.[j] = 1 AND [k] = 2"),
                Ok(("R.[j] = 1 AND [k] = 2".to_owned(), false)),
                "{backend:?}"
            );
            assert_eq!(
                bind(backend, "[__gom_row_1].j = 1"),
                Err(PolicySqlRefusal::ReservedName),
                "{backend:?}"
            );
        }
        assert_eq!(
            bind(
                Mssql,
                "__gom_self.a = N'__gom_self.b' AND __gom_self.b = N'x'"
            ),
            Ok(("R.a = N'__gom_self.b' AND R.b = N'x'".to_owned(), true))
        );
        // SQLite: backticks delimit a name, a named parameter is a word,
        // and the implicit row id columns are refused however written.
        assert_eq!(
            bind(Sqlite, "__gom_self.a = 1 AND `b` = 1"),
            Ok(("R.a = 1 AND `b` = 1".to_owned(), false))
        );
        for clause in [
            "rowid = 1",
            "__gom_self.ROWID = 1",
            "__gom_self._rowid_ = 1",
            "__gom_self.\"oid\" = 1",
            "[oid] = 1",
        ] {
            assert_eq!(
                bind(Sqlite, clause),
                Err(PolicySqlRefusal::ImplicitRowid),
                "{clause}"
            );
        }
        // Elsewhere those are ordinary column names.
        assert_eq!(
            bind(Postgres, "__gom_self.oid = 1").map(|(_, bound)| bound),
            Ok(true)
        );
        // Every refusal is AUTHORIZATION_MISCONFIGURED.
        let error = async_graphql::Error::from(PolicySqlRefusal::Keyword);
        assert_eq!(code(&error), "\"AUTHORIZATION_MISCONFIGURED\"");
    }

    #[test]
    fn the_bind_splitter_fails_closed() {
        use super::super::query::{
            checked_filter_expression, ensure_composable_sql, split_filter_binds,
        };
        let conditions = [
            "tenant_id = ?".to_owned(),
            "owner_id = ? AND note <> '?' AND \"a?\" IS NULL".to_owned(),
        ];
        let ints = |values: &[i64]| {
            values
                .iter()
                .copied()
                .map(SqlValue::Int)
                .collect::<Vec<_>>()
        };
        let refused =
            |error: sqlx::Error| code(&crate::graphql::errors::graphql_error_from_sqlx(error));
        for backend in BACKENDS {
            // Each condition takes exactly its own values.
            assert_eq!(
                split_filter_binds(backend, &conditions, &ints(&[7, 42])).unwrap(),
                Some(FilterExpression::And(vec![
                    FilterExpression::Raw {
                        clause: conditions[0].clone(),
                        values: ints(&[7]),
                    },
                    FilterExpression::Raw {
                        clause: conditions[1].clone(),
                        values: ints(&[42]),
                    },
                ])),
                "{backend:?}"
            );
            assert_eq!(split_filter_binds(backend, &[], &[]).unwrap(), None);
            // A surplus, a deficit, and values with no condition at all:
            // nothing is divided, dropped or moved.
            for (conditions, values) in [
                (&conditions[..], ints(&[7, 7, 42])),
                (&conditions[..], ints(&[7])),
                (&conditions[..], ints(&[])),
                (&[][..], ints(&[1])),
            ] {
                assert_eq!(
                    refused(split_filter_binds(backend, conditions, &values).unwrap_err()),
                    "\"AUTHORIZATION_MISCONFIGURED\"",
                    "{backend:?}: {values:?}"
                );
            }
        }
        // Placeholders a statement does not number itself cannot be
        // composed with a policy. PostgreSQL and SQL Server renumber every
        // `$N` and `@pN`, as generated filters write them; SQLite sends the
        // text as written.
        use DatabaseBackend::{Mssql, Postgres, Sqlite};
        for (backend, clause, composable) in [
            (Sqlite, "status = ?", true),
            (Sqlite, "status = ?1", false),
            (Sqlite, "status = ?1 OR previous = ?1", false),
            (Sqlite, "status = :status", false),
            (Sqlite, "status = @status", false),
            (Sqlite, "status = $status", false),
            (Sqlite, "status = $1", false),
            (Sqlite, "status = '?1' AND \":name\" = ?", true),
            (Postgres, "status = $1 AND other = $2", true),
            (Postgres, "status = ? AND other @> ?", true),
            (Postgres, "status = $name", false),
            (Mssql, "[status] = @P1 AND [other] = @p2", true),
            (Mssql, "[status] = @status", false),
            (Mssql, "[label]]?] = ?", true),
        ] {
            assert_eq!(
                ensure_composable_sql(backend, clause).is_ok(),
                composable,
                "{backend:?}: {clause}"
            );
        }
        assert_eq!(
            refused(ensure_composable_sql(Sqlite, "status = ?1").unwrap_err()),
            "\"AUTHORIZATION_MISCONFIGURED\""
        );
        // The default adapter counts with the dialect of the checked call,
        // not the build's default backend: `]]` is an escaped bracket in a
        // SQL Server name, and SQLite closes the name at its first `]`.
        struct Labelled;
        impl DatabaseFilter for Labelled {
            fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>) {
                (vec!["[label]]?] = ?".to_owned()], vec![SqlValue::Int(1)])
            }
            fn is_empty(&self) -> bool {
                false
            }
        }
        assert_eq!(
            checked_filter_expression(Mssql, || Labelled.to_filter_expression()).unwrap(),
            Some(FilterExpression::Raw {
                clause: "[label]]?] = ?".to_owned(),
                values: ints(&[1]),
            })
        );
        assert_eq!(
            refused(
                checked_filter_expression(Sqlite, || Labelled.to_filter_expression()).unwrap_err()
            ),
            "\"AUTHORIZATION_MISCONFIGURED\""
        );
        // A filter that returns an expression of its own is returned as is.
        let own = Some(FilterExpression::trusted_fragment("1 = 1", Vec::new()));
        assert_eq!(
            checked_filter_expression(Mssql, || own.clone()).unwrap(),
            own
        );
        // A filter that panics leaves no dialect behind on the thread.
        assert!(super::super::query::checked_adapter_is_idle());
        let panicked = std::panic::catch_unwind(|| {
            checked_filter_expression(Mssql, || panic!("a host filter panicked"))
        });
        assert!(panicked.is_err());
        assert!(super::super::query::checked_adapter_is_idle());
        // SQL Server `@@name` system functions are not parameters.
        assert!(ensure_composable_sql(Mssql, "[id] = @@SPID AND @@ROWCOUNT > 0").is_ok());
        assert!(ensure_composable_sql(Mssql, "[id] = @spid").is_err());
    }

    #[test]
    fn a_filter_expression_override_is_checked_beside_a_policy() {
        use super::super::query::composed_filter_expression;
        /// A host filter that returns an expression of its own.
        struct Override(&'static str, Vec<SqlValue>);
        impl DatabaseFilter for Override {
            fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>) {
                (Vec::new(), Vec::new())
            }
            fn is_empty(&self) -> bool {
                false
            }
            fn to_filter_expression(&self) -> Option<FilterExpression> {
                Some(FilterExpression::Raw {
                    clause: self.0.to_owned(),
                    values: self.1.clone(),
                })
            }
        }
        type Backend = crate::graphql::orm::DefaultBackend;
        let ints = |values: &[i64]| {
            values
                .iter()
                .copied()
                .map(SqlValue::Int)
                .collect::<Vec<_>>()
        };
        let composed = |clause, values: &[i64], policed| {
            composed_filter_expression::<_, Backend>(&Override(clause, ints(values)), policed)
                .map_err(|error| code(&crate::graphql::errors::graphql_error_from_sqlx(error)))
        };
        // Beside a policy every leaf holds exactly its own values: a
        // surplus would shift the policy's binds, a deficit takes them.
        for values in [&[1, 99][..], &[]] {
            assert_eq!(
                composed("score >= ?", values, true),
                Err("\"AUTHORIZATION_MISCONFIGURED\"".to_owned()),
                "{values:?}"
            );
        }
        assert_eq!(
            composed("score >= ?", &[1], true),
            Ok(Some(FilterExpression::Raw {
                clause: "score >= ?".to_owned(),
                values: ints(&[1]),
            }))
        );
        // With no policy the filter's expression is passed through.
        assert!(composed("score >= ?", &[1, 99], false).is_ok());
    }

    #[test]
    fn generated_names_are_exactly_the_reserved_ones() {
        for name in [
            "__gom_self",
            "__gom_row_1",
            "__gom_row_27",
            "__gom_visibility_3",
            "__gom_closure_1",
            "__gom_closure_2_t",
            "__gom_closure_2_h64",
            "__gom_closure_10_anything",
        ] {
            assert!(generated_name(name), "{name}");
            assert_eq!(
                code(&ensure_table_name(DatabaseBackend::Sqlite, name).unwrap_err()),
                "\"AUTHORIZATION_MISCONFIGURED\"",
                "{name}"
            );
        }
        for name in [
            "__gom_orders",
            "__gom_",
            "__gom_row_",
            "__gom_row_x",
            "__gom_row_1_x",
            "__gom_rows_1",
            "__gom_closure_",
            "__gom_closure_x",
            "__gom_closure_1_",
            "__gom_self_service",
            "gom_row_1",
        ] {
            assert!(!generated_name(name), "{name}");
            assert!(
                ensure_table_name(DatabaseBackend::Sqlite, name).is_ok(),
                "{name}"
            );
        }
        // The table's own name decides, in any case, delimited or not.
        for backend in BACKENDS {
            assert!(ensure_table_name(backend, "main.\"__GOM_Row_1\"").is_err());
            assert!(ensure_table_name(backend, "__gom_row_1.orders").is_ok());
        }
    }

    #[test]
    fn aliases_avoid_every_name_in_scope() {
        use DatabaseBackend::{Mssql, Postgres, Sqlite};
        let scope = |backend, sql: &str| Scope::of(backend, sql).unwrap();
        assert_eq!(scope(Sqlite, "x = 1").level(ROW_ALIASES), 1);
        assert_eq!(scope(Sqlite, "x = 1").level(CLOSURE_ALIASES), 1);
        // An alias already rendered inside, of either row family.
        assert_eq!(scope(Sqlite, "FROM t AS __gom_row_1").level(ROW_ALIASES), 2);
        assert_eq!(
            scope(Sqlite, "FROM t AS __gom_visibility_1").level(ROW_ALIASES),
            2
        );
        // Tables named like an alias, bare or delimited, in any case.
        assert_eq!(
            scope(
                Sqlite,
                "FROM \"__GOM_ROW_1\" JOIN [__gom_row_2] JOIN `__gom_visibility_3`"
            )
            .level(ROW_ALIASES),
            4
        );
        assert_eq!(
            scope(Mssql, "FROM [__gom_row_1] JOIN \"__gom_row_2\"").level(ROW_ALIASES),
            3
        );
        // A table with the exact name of a closure's common table
        // expression, and names derived from a level.
        for backend in [Sqlite, Postgres, Mssql] {
            assert_eq!(
                scope(backend, "FROM \"__gom_closure_1\"").level(CLOSURE_ALIASES),
                2,
                "{backend:?}"
            );
            assert_eq!(
                scope(
                    backend,
                    "FROM __gom_closure_1_t JOIN \"__gom_closure_2_h7\""
                )
                .level(CLOSURE_ALIASES),
                3,
                "{backend:?}"
            );
            // A longer number is another level.
            assert_eq!(
                scope(backend, "FROM __gom_closure_10").level(CLOSURE_ALIASES),
                1,
                "{backend:?}"
            );
            // Strings and comments name nothing.
            assert_eq!(
                scope(backend, "'__gom_closure_1' /* __gom_closure_1 */").level(CLOSURE_ALIASES),
                1,
                "{backend:?}"
            );
        }
        // The tables the caller knows to be visible.
        assert_eq!(
            scope(Sqlite, "FROM t AS __gom_row_2")
                .table(Sqlite, "main.__GOM_row_1")
                .level(ROW_ALIASES),
            3
        );
        assert_eq!(
            scope(Mssql, "x = 1")
                .table(Mssql, "[dbo].[__gom_closure_1]")
                .level(CLOSURE_ALIASES),
            2
        );
        // SQL that cannot be read with certainty allocates nothing.
        assert!(Scope::of(Sqlite, "x = 'open").is_err());
    }

    #[test]
    fn a_count_ordering_allocates_both_of_its_aliases() {
        use DatabaseBackend::Sqlite;
        let policy = |clause: &str, unbound: bool| ReadPredicate {
            entity: TypeId::of::<()>(),
            backend: Sqlite,
            table: Some("targets"),
            policy: Policy::Sql {
                sql: handwritten_sql(Sqlite, clause, 0).unwrap().0,
                values: Vec::new(),
            },
            unbound,
            cost: Cost::default(),
            negation: Negation::Sql,
        };
        let count = |source: &str, target: &str, policy: Option<&ReadPredicate>| {
            relation_count_sql(Sqlite, source, target, &[("s", "id")], policy).map(|(sql, _)| sql)
        };
        let flag = policy("__gom_self.flag = 1", false);
        assert_eq!(
            count("sources", "targets", Some(&flag)).unwrap(),
            "(SELECT COUNT(*) FROM (SELECT * FROM targets AS __gom_row_1 WHERE \
             __gom_row_1.flag = 1) AS __gom_visibility_1 WHERE __gom_visibility_1.s = sources.id)"
        );
        assert_eq!(
            count("sources", "targets", None).unwrap(),
            "(SELECT COUNT(*) FROM targets AS __gom_visibility_1 WHERE __gom_visibility_1.s = sources.id)"
        );
        // A table once named like the fixed alias is an ordinary table now.
        assert_eq!(
            count("__graphql_orm_order_relation", "targets", None).unwrap(),
            "(SELECT COUNT(*) FROM targets AS __gom_visibility_1 WHERE \
             __gom_visibility_1.s = __graphql_orm_order_relation.id)"
        );
        // A source or target table named like a row alias never reaches the
        // statement: SQLite would fall back to it for a column the counted
        // table lacks.
        for table in [
            "__gom_row_1",
            "main.\"__GOM_ROW_1\"",
            "[__gom_visibility_1]",
        ] {
            for result in [
                count(table, "targets", Some(&flag)),
                count(table, "targets", None),
                count("sources", table, None),
            ] {
                assert_eq!(
                    code(&result.unwrap_err()),
                    "\"AUTHORIZATION_MISCONFIGURED\"",
                    "{table}"
                );
            }
        }
        // A name inside the policy that an alias would collide with moves
        // both aliases.
        let nested = ReadPredicate {
            policy: Policy::sql(
                "EXISTS (SELECT 1 FROM (SELECT * FROM o AS __gom_row_1 WHERE 1 = 1) AS \
                 __gom_visibility_1 WHERE 1 = 1)",
            ),
            ..policy("1 = 1", false)
        };
        assert!(
            count("sources", "targets", Some(&nested))
                .unwrap()
                .starts_with("(SELECT COUNT(*) FROM (SELECT * FROM targets AS __gom_row_2 WHERE ")
        );
        assert!(
            count("sources", "targets", Some(&nested))
                .unwrap()
                .ends_with(" AS __gom_visibility_2 WHERE __gom_visibility_2.s = sources.id)")
        );
        // A policy with a column that is not written through the row is
        // refused: the count evaluates it in a correlated subquery.
        assert_eq!(
            code(&count("sources", "targets", Some(&policy("flag = 1", true))).unwrap_err()),
            "\"AUTHORIZATION_MISCONFIGURED\""
        );
    }

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
                table: None,
                policy: raw_policy(&expression),
                unbound: false,
                cost: Cost::default(),
                negation: Negation::Sql,
            };
            assert!(predicate.not().is_err());
        }
    }

    fn sqlite_fragment(clause: &str, values: Vec<SqlValue>) -> ReadPredicate {
        ReadPredicate {
            entity: TypeId::of::<()>(),
            backend: DatabaseBackend::Sqlite,
            table: None,
            policy: Policy::Sql {
                sql: RowBoundSql::sql(clause),
                values,
            },
            unbound: false,
            cost: Cost::of(DatabaseBackend::Sqlite, clause).unwrap(),
            negation: Negation::Sql,
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
                        &predicate.root_expression(),
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
                assert!(matches!(&wide.policy, Policy::Or(children) if children.len() == 50_001));
                let clause = render(&wide);
                assert_eq!(wide.cost.depth, cost_of(&clause).depth);
                assert_eq!(wide.cost.units, cost_of(&clause).units);
                assert_eq!(
                    wide.cost.depth,
                    1 + super::super::query::filter_chain_levels(50_001)
                );
                assert!(!format!("{wide:?}").is_empty());
                let copy = wide.clone();
                assert!(copy.root_expression() == wide.root_expression());
                drop((copy, wide));
                // An alternating `and`/`or` chain is refused at the depth
                // limit; the deepest that builds is measured exactly.
                let (mut deepest, mut levels) = (leaf(), 0);
                loop {
                    let next = if levels % 2 == 0 {
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
                assert_eq!(cost_of(&clause).depth, READ_PREDICATE_MAX_EXPRESSION_DEPTH);
                assert_eq!(deepest.cost.depth, cost_of(&clause).depth);
                assert!(!format!("{deepest:?}").is_empty());
                let copy = deepest.clone();
                assert!(copy.root_expression() == deepest.root_expression());
                drop((copy, deepest));
                // `not` swaps a predicate with its negation, so a `not` chain
                // of any length alternates between two renderings and never
                // deepens.
                let mut negated = leaf();
                for step in 1..=1_000 {
                    negated = negated.not().unwrap();
                    let clause = render(&negated);
                    assert_eq!(
                        clause,
                        if step % 2 == 0 {
                            "x = ?"
                        } else {
                            "NOT (x = ?)"
                        }
                    );
                    assert_eq!(negated.cost.depth, cost_of(&clause).depth);
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn fold_byte_estimates_never_undercount_the_render() {
        let render = |predicate: &ReadPredicate| {
            render_filter_expression(
                DatabaseBackend::Sqlite,
                &predicate.root_expression(),
                &mut 1,
                &mut Vec::new(),
            )
        };
        for and in [true, false] {
            let leaf = || sqlite_fragment("x IS NULL", Vec::new());
            let step = |fold: ReadPredicate| {
                if and {
                    fold.and(leaf())
                } else {
                    fold.or(leaf())
                }
            };
            let mut fold = leaf();
            let mut terms = 1;
            // Grow without copying until close to the budget, checking the
            // estimate on the way, then step carefully up to the refusal.
            while fold.cost.bytes + 64 < READ_PREDICATE_MAX_RENDERED_BYTES {
                fold = step(fold).unwrap();
                terms += 1;
                if terms % 9_973 == 0 {
                    assert!(fold.cost.bytes >= render(&fold).len(), "{terms}");
                }
            }
            while let Ok(next) = step(fold.clone()) {
                fold = next;
            }
            // The last fold that builds renders within the budget.
            let clause = render(&fold);
            assert!(fold.cost.bytes >= clause.len());
            assert!(
                clause.len() <= READ_PREDICATE_MAX_RENDERED_BYTES,
                "{}",
                clause.len()
            );
            assert!(
                clause.len() > READ_PREDICATE_MAX_RENDERED_BYTES * 9 / 10,
                "{}",
                clause.len()
            );
        }
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
        let cost = cost_of(clause);
        assert_eq!(cost.nesting, 2);
        // Three parentheses deep; SQL Server units 11 + 11 + 1.
        assert_eq!((cost.depth, cost.units), (3, 23));
        assert_eq!(cost_of("NOT (NOT (x)) AND KNOT (y)").units, 4);
        assert_eq!(cost.closures, 0);
        assert_eq!(cost.bytes, clause.len() + 6);
        assert_eq!(cost_of("(WITH c AS (SELECT 1) SELECT 1)").nesting, 2);
        // Closure nesting: the distinct levels that open a closure. A name
        // that opens none, bare or delimited, is not one.
        assert_eq!(cost_of("__gom_closure_2_l0 __gom_closure_1").closures, 0);
        assert_eq!(
            cost_of(
                "(WITH RECURSIVE __gom_closure_3 (p) AS (SELECT (WITH RECURSIVE __gom_closure_2 (p) \
                 AS (SELECT 1 FROM \"__gom_closure_1\") SELECT 1)) SELECT 1) AND \
                 (WITH RECURSIVE __gom_closure_2 (p) AS (SELECT 1) SELECT 1)"
            )
            .closures,
            2
        );
        // Closure instances: one per CTE opening or SQL Server walk alias,
        // not per reference.
        let cte = cost_of("(WITH RECURSIVE __gom_closure_1 (__gom_p0) AS (SELECT 1) SELECT 1)");
        assert_eq!(cte.instances, 1);
        let walk = cost_of(
            "(SELECT 1 FROM (SELECT 1) AS __gom_closure_2_g OUTER APPLY (SELECT 1 WHERE \
             __gom_closure_2_g.__gom_p0 = 1) AS __gom_closure_2_r) AS __gom_closure_1_g",
        );
        assert_eq!(walk.instances, 2);
        assert_eq!(
            walk.relation_weight(DatabaseBackend::Mssql),
            2 * READ_PREDICATE_MSSQL_CLOSURE_RELATION_WEIGHT
        );
        assert_eq!(
            cte.relation_weight(DatabaseBackend::Postgres),
            READ_PREDICATE_CLOSURE_RELATION_WEIGHT
        );
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
                table: None,
                policy: raw_policy(&expression),
                unbound: false,
                cost: Cost::default(),
                negation: Negation::Sql,
            };
            assert!(predicate.clone().not().is_err(), "{predicate:?}");
            assert!(predicate.not().is_err());
        }
        for clause in ["\"rank\" = ?", "\"col1\" = 10", "\"x1\" = 0", "\"a\" <> ?"] {
            assert!(sqlite_fragment(clause, vec![]).not().is_ok(), "{clause}");
        }
    }
}
