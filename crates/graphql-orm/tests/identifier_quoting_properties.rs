// One backend per build for the executed round trips; the text properties
// cover every dialect in each build.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]

//! Bounded property suite for SQL identifier quoting (#725, #782, #783).
//!
//! The dialect contract, from [`SqlDialect::quote_identifier`] and
//! [`DatabaseBackend::render_identifier`]:
//!
//! - input that is exactly one well-formed, non-empty delimited identifier
//!   for the dialect is returned unchanged, so quoting is idempotent;
//! - any other input, including text that only starts and ends with
//!   delimiter characters, is written as exactly one delimited identifier,
//!   with the closing delimiter doubled, so it reads back as the same name
//!   (#782);
//! - `render_identifier` writes a name bare only when the dialect's parser
//!   reads it bare as that same name and it is not reserved, and otherwise
//!   delimits it as above. On SQL Server that is the regular-identifier
//!   rule, the Unicode 3.2 identifier classes of the Basic Multilingual Plane
//!   plus `@ $ # _`, checked character by character against the server (#783).
//!
//! The executed properties create, write, read and inspect one table per case
//! on the build's backend: in-memory SQLite, or the PostgreSQL or SQL Server
//! container these tests own. Cases run from a fixed seed; set
//! `PROPTEST_CASES` or `PROPTEST_RNG_SEED` to explore further, and keep any
//! minimized failure as an explicit regression case below.

#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::graphql::orm::{DatabaseBackend, SqlDialect};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence, RngSeed, TestCaseError, TestRunner};

const TEXT_BACKENDS: [DatabaseBackend; 3] = [
    DatabaseBackend::Sqlite,
    DatabaseBackend::Postgres,
    DatabaseBackend::Mssql,
];

/// Seeded, bounded configuration. `PROPTEST_CASES` and `PROPTEST_RNG_SEED`
/// override the defaults for a longer local exploration.
fn config(cases: u32, seed: u64) -> Config {
    let env_u64 = |name: &str| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
    };
    Config {
        cases: env_u64("PROPTEST_CASES").map_or(cases, |value| value as u32),
        rng_seed: RngSeed::Fixed(env_u64("PROPTEST_RNG_SEED").unwrap_or(seed)),
        max_shrink_iters: 1024,
        source_file: Some(file!()),
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource("regressions"))),
        ..Config::default()
    }
}

/// Characters that stress SQL tokenizers: every ASCII delimiter and
/// separator, Unicode spaces, combining marks, bidirectional controls,
/// case-mapping outliers and arbitrary scalar values. NUL is excluded: no
/// backend stores it in a name, and the bound is the database's, not ours.
fn identifier_char() -> impl Strategy<Value = char> {
    prop_oneof![
        6 => proptest::char::range('a', 'z'),
        2 => proptest::char::range('A', 'Z'),
        2 => proptest::char::range('0', '9'),
        4 => proptest::sample::select(vec![
            '"', '[', ']', '`', '.', '\'', ';', ' ', '-', '\\', '_', '$', '#', '@', '(', ')',
            ',', '*', '/', '=', '\t', '\n', '\r', '%', '?', ':',
        ]),
        2 => proptest::sample::select(vec![
            'é', '\u{301}', '雪', '🦀', 'İ', 'ı', 'ß', 'Ω', '\u{2126}', '\u{a0}', '\u{3000}',
            '\u{2028}', '\u{200b}', '\u{200d}', '\u{200f}', '\u{202e}', '\u{feff}', '\u{ff02}',
            '\u{ff3d}', '\u{2019}',
        ]),
        1 => any::<char>().prop_filter("NUL is never a name character", |c| *c != '\0'),
    ]
}

/// Words that are, or are close to, reserved in at least one dialect, in
/// random ASCII case, so the bare rendering is checked against each parser.
const KEYWORDS: &[&str] = &[
    "abort",
    "action",
    "add",
    "all",
    "alter",
    "always",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "asymmetric",
    "attach",
    "authorization",
    "autoincrement",
    "backup",
    "before",
    "begin",
    "between",
    "binary",
    "both",
    "break",
    "browse",
    "bulk",
    "by",
    "cascade",
    "case",
    "cast",
    "check",
    "checkpoint",
    "close",
    "clustered",
    "coalesce",
    "collate",
    "collation",
    "column",
    "commit",
    "compute",
    "concurrently",
    "conflict",
    "constraint",
    "contains",
    "containstable",
    "continue",
    "convert",
    "create",
    "cross",
    "current",
    "current_catalog",
    "current_date",
    "current_role",
    "current_schema",
    "current_time",
    "current_timestamp",
    "current_user",
    "cursor",
    "database",
    "dbcc",
    "deallocate",
    "declare",
    "default",
    "deferrable",
    "deferred",
    "delete",
    "deny",
    "desc",
    "detach",
    "disk",
    "distinct",
    "distributed",
    "do",
    "double",
    "drop",
    "dump",
    "each",
    "else",
    "end",
    "errlvl",
    "escape",
    "except",
    "exclude",
    "exclusive",
    "exec",
    "execute",
    "exists",
    "exit",
    "explain",
    "external",
    "fail",
    "false",
    "fetch",
    "file",
    "fillfactor",
    "filter",
    "first",
    "following",
    "for",
    "foreign",
    "freetext",
    "freetexttable",
    "freeze",
    "from",
    "full",
    "function",
    "generated",
    "glob",
    "goto",
    "grant",
    "group",
    "groups",
    "having",
    "holdlock",
    "identity",
    "identity_insert",
    "identitycol",
    "if",
    "ignore",
    "ilike",
    "immediate",
    "in",
    "index",
    "indexed",
    "initially",
    "inner",
    "insert",
    "instead",
    "int",
    "integer",
    "intersect",
    "into",
    "is",
    "isnull",
    "join",
    "key",
    "kill",
    "last",
    "lateral",
    "leading",
    "left",
    "like",
    "limit",
    "lineno",
    "load",
    "localtime",
    "localtimestamp",
    "match",
    "materialized",
    "merge",
    "national",
    "natural",
    "no",
    "nocheck",
    "nonclustered",
    "not",
    "nothing",
    "notnull",
    "null",
    "nullif",
    "nulls",
    "of",
    "off",
    "offset",
    "offsets",
    "on",
    "only",
    "open",
    "opendatasource",
    "openquery",
    "openrowset",
    "openxml",
    "option",
    "or",
    "order",
    "others",
    "outer",
    "over",
    "overlaps",
    "partition",
    "percent",
    "pivot",
    "placing",
    "plan",
    "position",
    "pragma",
    "preceding",
    "primary",
    "print",
    "proc",
    "procedure",
    "public",
    "query",
    "raise",
    "raiserror",
    "range",
    "read",
    "readtext",
    "reconfigure",
    "recursive",
    "references",
    "regexp",
    "reindex",
    "release",
    "rename",
    "replace",
    "replication",
    "restore",
    "restrict",
    "returning",
    "revert",
    "revoke",
    "right",
    "rollback",
    "row",
    "rowcount",
    "rowguidcol",
    "rows",
    "rule",
    "save",
    "savepoint",
    "schema",
    "securityaudit",
    "select",
    "session_user",
    "set",
    "setuser",
    "shutdown",
    "similar",
    "some",
    "statistics",
    "symmetric",
    "system_user",
    "table",
    "tablesample",
    "temp",
    "temporary",
    "textsize",
    "then",
    "ties",
    "time",
    "timestamp",
    "to",
    "top",
    "trailing",
    "tran",
    "transaction",
    "trigger",
    "true",
    "truncate",
    "try_convert",
    "tsequal",
    "type",
    "unbounded",
    "union",
    "unique",
    "unpivot",
    "update",
    "updatetext",
    "use",
    "user",
    "using",
    "vacuum",
    "value",
    "values",
    "variadic",
    "verbose",
    "view",
    "virtual",
    "waitfor",
    "when",
    "where",
    "while",
    "window",
    "with",
    "within",
    "without",
    "writetext",
];

fn keyword() -> impl Strategy<Value = String> {
    (proptest::sample::select(KEYWORDS), any::<u64>()).prop_map(|(word, case)| {
        word.chars()
            .enumerate()
            .map(|(index, character)| {
                if case >> (index % 64) & 1 == 1 {
                    character.to_ascii_uppercase()
                } else {
                    character
                }
            })
            .collect()
    })
}

fn identifier(max_chars: usize) -> impl Strategy<Value = String> {
    prop_oneof![
        6 => vec(identifier_char(), 1..=max_chars).prop_map(|chars| chars.into_iter().collect()),
        2 => keyword(),
        1 => (keyword(), vec(identifier_char(), 0..4)).prop_map(|(word, tail)| {
            word.chars().chain(tail).collect()
        }),
    ]
}

/// Opening delimiters each dialect accepts, with their closing delimiter and
/// whether a doubled closing delimiter escapes it inside the name.
fn delimiters(backend: DatabaseBackend) -> &'static [(char, char, bool)] {
    match backend {
        // SQLite reads `[...]` up to the first `]`, with no escape.
        DatabaseBackend::Sqlite => &[('"', '"', true), ('`', '`', true), ('[', ']', false)],
        DatabaseBackend::Postgres => &[('"', '"', true)],
        DatabaseBackend::Mssql => &[('[', ']', true), ('"', '"', true)],
        DatabaseBackend::Mysql => &[],
    }
}

/// The name `text` denotes when it is exactly one well-formed, non-empty
/// delimited identifier: the only input `quote_identifier` returns unchanged.
fn well_formed_delimited(backend: DatabaseBackend, text: &str) -> Option<String> {
    read_one_delimited(backend, text).filter(|name| !name.is_empty())
}

/// Reads `text` as exactly one delimited identifier and returns the name it
/// denotes, or `None` when it is not exactly one well-formed delimited token.
fn read_one_delimited(backend: DatabaseBackend, text: &str) -> Option<String> {
    let mut characters = text.chars();
    let first = characters.next()?;
    let (_, close, doubled) = *delimiters(backend)
        .iter()
        .find(|(open, _, _)| *open == first)?;
    let mut name = String::new();
    let mut characters = characters.peekable();
    while let Some(character) = characters.next() {
        if character != close {
            name.push(character);
            continue;
        }
        if doubled && characters.peek() == Some(&close) {
            characters.next();
            name.push(close);
            continue;
        }
        // The closing delimiter must be the token's last character.
        return characters.next().is_none().then_some(name);
    }
    None
}

/// The primary delimiting `quote_identifier` writes for undelimited input.
fn expected_quoted(backend: DatabaseBackend, name: &str) -> String {
    match backend {
        DatabaseBackend::Mssql => format!("[{}]", name.replace(']', "]]")),
        _ => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// A bare rendering may contain only plain identifier characters, so it can
/// never introduce a delimiter, separator, operator or whitespace. SQL Server
/// additionally reads only Basic Multilingual Plane characters bare, starting
/// with a letter (#783); its exact Unicode 3.2 tables are checked against the
/// server itself in the SQL Server lane.
fn is_plain_bare(backend: DatabaseBackend, text: &str) -> bool {
    let mut characters = text.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    let sql_server_non_ascii =
        |c: char| !c.is_ascii() && u32::from(c) <= 0xFFFF && !c.is_whitespace();
    let start = |c: char| {
        c.is_ascii_alphabetic()
            || c == '_'
            || match backend {
                DatabaseBackend::Mssql => {
                    matches!(c, '#' | '@')
                        || (sql_server_non_ascii(c) && (c.is_alphabetic() || c == '\u{ff3f}'))
                }
                _ => !c.is_ascii(),
            }
    };
    let rest = |c: char| {
        start(c)
            || c.is_ascii_digit()
            || c == '$'
            || (backend == DatabaseBackend::Mssql && sql_server_non_ascii(c))
    };
    // SQLite reads a byte-order mark that starts a token as whitespace.
    start(first)
        && !(backend == DatabaseBackend::Sqlite && first == '\u{feff}')
        && characters.all(rest)
}

/// Input biased towards text that starts and ends with a delimiter character.
fn any_input() -> impl Strategy<Value = String> {
    let delimited_looking = (
        proptest::sample::select(vec![('"', '"'), ('[', ']'), ('`', '`')]),
        identifier(12),
    )
        .prop_map(|((open, close), inner)| format!("{open}{inner}{close}"));
    let doubled = (
        proptest::sample::select(vec![('"', '"'), ('[', ']'), ('`', '`')]),
        identifier(12),
    )
        .prop_map(|((open, close), inner)| {
            format!(
                "{open}{}{close}",
                inner.replace(close, &format!("{close}{close}"))
            )
        });
    prop_oneof![4 => identifier(24), 2 => delimited_looking, 1 => doubled]
}

/// The strict contract for *any* input, including text that only starts and
/// ends with delimiter characters (#782): the result is exactly one
/// well-formed delimited identifier, which is the input itself when the input
/// already was one and otherwise names the input literally.
#[test]
fn every_input_quotes_to_exactly_one_identifier() {
    let mut runner = TestRunner::new(config(2048, 0x7250_0004));
    runner
        .run(&any_input(), |name| {
            for backend in TEXT_BACKENDS {
                let quoted = backend.quote_identifier(&name);
                if well_formed_delimited(backend, &name).is_some() {
                    prop_assert_eq!(&quoted, &name, "{:?}", backend);
                } else {
                    prop_assert_eq!(&quoted, &expected_quoted(backend, &name), "{:?}", backend);
                    prop_assert_eq!(
                        read_one_delimited(backend, &quoted),
                        Some(name.clone()),
                        "{:?} {:?} -> {:?}",
                        backend,
                        name,
                        quoted
                    );
                }
                // Idempotent: a delimited identifier is never delimited twice.
                prop_assert_eq!(&backend.quote_identifier(&quoted), &quoted);
                prop_assert_eq!(&backend.quote_identifier_path(&quoted), &quoted);
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn rendered_identifiers_are_bare_and_plain_or_delimited_and_exact() {
    let mut runner = TestRunner::new(config(2048, 0x7250_0002));
    runner
        .run(&any_input(), |name| {
            for backend in TEXT_BACKENDS {
                let rendered = backend.render_identifier(&name);
                if well_formed_delimited(backend, &name).is_some() {
                    prop_assert_eq!(rendered.as_ref(), name.as_str(), "{:?}", backend);
                } else if rendered == name.as_str() {
                    prop_assert!(is_plain_bare(backend, &name), "{:?} {:?}", backend, name);
                } else {
                    prop_assert_eq!(rendered.as_ref(), backend.quote_identifier(&name));
                }
                prop_assert!(
                    rendered == name.as_str() || read_one_delimited(backend, &rendered).is_some(),
                    "{:?} {:?} -> {:?}",
                    backend,
                    name,
                    rendered
                );
                // Rendering is idempotent too.
                prop_assert_eq!(backend.render_identifier(&rendered), rendered.clone());
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn quoted_paths_keep_each_undelimited_part_as_one_identifier() {
    let part = identifier(12).prop_filter("dots separate path parts", |part| !part.contains('.'));
    let mut runner = TestRunner::new(config(512, 0x7250_0003));
    runner
        .run(&vec(part, 1..4), |parts| {
            for backend in TEXT_BACKENDS {
                // The path splitter reads `"`, `` ` `` and `[` as opening
                // delimiters on every backend, so a part that starts with
                // one is a delimited part rather than undelimited input.
                if parts.iter().any(|part| part.starts_with(['"', '`', '['])) {
                    continue;
                }
                let quoted = backend.quote_identifier_path(&parts.join("."));
                let expected = parts
                    .iter()
                    .map(|part| expected_quoted(backend, part))
                    .collect::<Vec<_>>()
                    .join(".");
                prop_assert_eq!(&quoted, &expected, "{:?}", backend);
                prop_assert_eq!(&backend.quote_identifier_path(&quoted), &quoted);
            }
            Ok(())
        })
        .unwrap();
}

/// Minimized counterexamples from #782, with the exact text each one now
/// renders: a literal name, never a pass-through.
#[test]
fn malformed_delimited_input_is_not_passed_through() {
    for (backend, name, expected) in [
        (DatabaseBackend::Sqlite, "\"a\"b\"", "\"\"\"a\"\"b\"\"\""),
        (DatabaseBackend::Sqlite, "\"\"\"", "\"\"\"\"\"\"\"\""),
        (DatabaseBackend::Sqlite, "[a]b]", "\"[a]b]\""),
        (DatabaseBackend::Sqlite, "`a`b`", "\"`a`b`\""),
        (DatabaseBackend::Sqlite, "\"\"", "\"\"\"\"\"\""),
        (DatabaseBackend::Postgres, "\"\"", "\"\"\"\"\"\""),
        (DatabaseBackend::Postgres, "\"", "\"\"\"\""),
        (
            DatabaseBackend::Postgres,
            "\"a\" , \"b\"",
            "\"\"\"a\"\" , \"\"b\"\"\"",
        ),
        (DatabaseBackend::Mssql, "[a]]", "[[a]]]]]"),
        (DatabaseBackend::Mssql, "[a] , [b]", "[[a]] , [b]]]"),
        (DatabaseBackend::Mssql, "[]", "[[]]]"),
        (DatabaseBackend::Mssql, "\"a\"b\"", "[\"a\"b\"]"),
    ] {
        let quoted = backend.quote_identifier(name);
        assert_eq!(quoted, expected, "{backend:?} {name:?}");
        assert_eq!(
            read_one_delimited(backend, &quoted).as_deref(),
            Some(name),
            "{backend:?} {name:?} -> {quoted:?}"
        );
        assert_eq!(
            backend.render_identifier(name),
            expected,
            "{backend:?} {name:?}"
        );
        assert_eq!(
            backend.quote_identifier_path(name),
            expected,
            "{backend:?} {name:?}"
        );
        assert_eq!(
            backend.render_identifier_path(name),
            expected,
            "{backend:?} {name:?}"
        );
    }
}

/// Well-formed delimited identifiers and paths keep their exact text, so
/// declared names that were already accepted render byte-identically.
#[test]
fn well_formed_delimited_input_is_unchanged() {
    for (backend, name) in [
        (DatabaseBackend::Sqlite, "\"a\"\"b\""),
        (DatabaseBackend::Sqlite, "[a b]"),
        (DatabaseBackend::Sqlite, "`a``b`"),
        (DatabaseBackend::Postgres, "\"Mixed\"\"Case\""),
        (DatabaseBackend::Mssql, "[a]]b]"),
        (DatabaseBackend::Mssql, "\"a\"\"b\""),
    ] {
        assert_eq!(backend.quote_identifier(name), name, "{backend:?}");
        assert_eq!(backend.render_identifier(name), name, "{backend:?}");
    }
    for (backend, path) in [
        (DatabaseBackend::Sqlite, "\"app\".\"users\""),
        (DatabaseBackend::Postgres, "\"app\".\"users\""),
        (DatabaseBackend::Mssql, "[app].[users]"),
    ] {
        assert_eq!(backend.quote_identifier_path(path), path, "{backend:?}");
        assert_eq!(backend.render_identifier_path(path), path, "{backend:?}");
    }
}

/// Minimized names from #783: SQL Server refuses each of them bare, so each
/// renders bracketed; names inside SQL Server's regular-identifier rule keep
/// their bare text.
#[test]
fn sql_server_renders_names_outside_the_regular_identifier_rule_bracketed() {
    let mssql = DatabaseBackend::Mssql;
    for name in [
        "雪🦀",
        "x²",
        "x¹",
        "x³",
        "x¼",
        "x½",
        "x¾",
        "x\u{221}",
        "x\u{234}",
        "x\u{24f}",
        "x\u{3000}y",
        "\u{301}x",
        "\u{663}x",
        "\u{200d}x",
        "x\u{10400}",
    ] {
        assert_eq!(
            mssql.render_identifier(name),
            format!("[{name}]"),
            "{name:?}"
        );
    }
    for name in [
        "雪",
        "café",
        "Ωmega",
        "名前_1",
        "ÿ$#@",
        "x\u{663}",
        "e\u{301}",
        "x\u{200d}y",
        "\u{216b}x",
        "\u{ff3f}x",
        "x\u{30fb}y",
    ] {
        assert_eq!(mssql.render_identifier(name), name, "{name:?}");
    }
    // PostgreSQL and SQLite read any of these bare.
    for backend in [DatabaseBackend::Sqlite, DatabaseBackend::Postgres] {
        assert_eq!(backend.render_identifier("x²"), "x²", "{backend:?}");
    }
}

// ----- executed round trips ---------------------------------------------------

#[cfg(feature = "sqlite")]
const BACKEND: DatabaseBackend = DatabaseBackend::Sqlite;
#[cfg(feature = "postgres")]
const BACKEND: DatabaseBackend = DatabaseBackend::Postgres;
#[cfg(feature = "mssql")]
const BACKEND: DatabaseBackend = DatabaseBackend::Mssql;

/// Whether the backend can hold `name` as written, under limits that are the
/// database's rather than the quoting contract's.
fn storable(name: &str) -> bool {
    match BACKEND {
        // Longer PostgreSQL names are truncated to NAMEDATALEN - 1 bytes.
        DatabaseBackend::Postgres => name.len() <= 63,
        // `sysname` is `nvarchar(128)`.
        DatabaseBackend::Mssql => name.encode_utf16().count() <= 128,
        // `sqlite_` names are reserved for SQLite's own tables.
        _ => !name.to_ascii_lowercase().starts_with("sqlite_"),
    }
}

/// One identifier as written in a statement, with the catalog name it must
/// produce.
#[derive(Clone, Debug)]
struct Written {
    sql: String,
    catalog: String,
}

fn written(name: &str, delimit_always: bool) -> Option<Written> {
    if name.contains('\0') {
        return None;
    }
    // Input that is already one well-formed delimited identifier is written
    // unchanged and names what is inside it.
    if let Some(inner) = well_formed_delimited(BACKEND, name) {
        let sql = if delimit_always {
            BACKEND.quote_identifier(name)
        } else {
            BACKEND.render_identifier(name).into_owned()
        };
        return storable(&inner).then_some(Written {
            sql,
            catalog: inner,
        });
    }
    if !storable(name) {
        return None;
    }
    let sql = if delimit_always {
        BACKEND.quote_identifier(name)
    } else {
        BACKEND.render_identifier(name).into_owned()
    };
    let bare = sql == name;
    // SQL Server keeps `#` and `@` prefixes bare: they name temporary tables
    // and variables, which is the documented meaning rather than a table.
    if bare && BACKEND == DatabaseBackend::Mssql && name.starts_with(['#', '@']) {
        return None;
    }
    // PostgreSQL folds bare ASCII letters to lowercase; this is the contract
    // that keeps earlier releases' names unchanged.
    let catalog = if bare && BACKEND == DatabaseBackend::Postgres {
        name.to_ascii_lowercase()
    } else {
        name.to_owned()
    };
    Some(Written { sql, catalog })
}

/// Two names and whether each is always delimited or rendered as DDL writes it.
fn table_and_column() -> impl Strategy<Value = (Written, Written)> {
    (any_input(), any::<bool>(), any_input(), any::<bool>()).prop_filter_map(
        "outside the backend's storable names",
        |(t, tq, c, cq)| {
            // SQL Server reads a table name that starts with `#` as a
            // temporary table, delimited or not.
            let table = written(&t, tq)?;
            if BACKEND == DatabaseBackend::Mssql && table.catalog.starts_with('#') {
                return None;
            }
            Some((table, written(&c, cq)?))
        },
    )
}

fn db_config(cases: u32, seed: u64) -> Config {
    Config {
        max_shrink_iters: 256,
        max_global_rejects: 100_000,
        ..config(cases, seed)
    }
}

/// The statements one case runs, in order, then the cleanup.
///
/// No identifier ends a statement: drivers trim Unicode whitespace from the
/// ends of SQL text, which would otherwise change a trailing name.
fn statements(table: &Written, column: &Written) -> [String; 4] {
    let value_type = if BACKEND == DatabaseBackend::Mssql {
        "int"
    } else {
        "INTEGER"
    };
    [
        format!("CREATE TABLE {} ({} {value_type});", table.sql, column.sql),
        format!("INSERT INTO {} ({}) VALUES (7);", table.sql, column.sql),
        format!("SELECT {} FROM {} WHERE 1 = 1;", column.sql, table.sql),
        format!("DROP TABLE IF EXISTS {};", table.sql),
    ]
}

/// Explicit regression cases: names minimized from earlier runs and names
/// at each dialect's edges.
fn retained_cases() -> Vec<(String, String)> {
    [
        ("plain", "value"),
        ("order", "select"),
        ("Mixed Case", "createdAt"),
        ("a\"b", "c]d"),
        ("a.b", "c;d"),
        ("雪🦀", "e\u{301}"),
        ("\u{3000}x", "y\u{a0}z"),
        ("\u{feff}t", "\u{200f}c"),
        ("x'", "--"),
        ("[t", "\"c"),
        ("t]", "c\""),
        ("`t", "c`"),
        // #782: text that only starts and ends with delimiters.
        ("\"a\"b\"", "\"\"\""),
        ("\"", "\"\""),
        ("[a]]", "[a] , [b]"),
        ("[a]b]", "`a`b`"),
        ("\"a\" , \"b\"", "[]"),
        // Well-formed delimited input is passed through.
        ("\"Mixed\"\"Case\"", "[c]]d]"),
        // #783: names SQL Server refuses bare.
        ("雪🦀", "x²"),
        ("x\u{221}", "x\u{3000}y"),
        ("x¼", "\u{301}x"),
        ("\u{663}x", "x\u{10400}"),
        // Names inside SQL Server's regular-identifier rule stay bare.
        ("café", "x\u{663}"),
        ("\u{216b}x", "e\u{301}\u{200d}"),
    ]
    .into_iter()
    .map(|(table, column)| (table.to_owned(), column.to_owned()))
    .collect()
}

fn retained_written() -> Vec<(Written, Written)> {
    let mut cases = Vec::new();
    for (table, column) in retained_cases() {
        for (table_quoted, column_quoted) in [(true, true), (false, false)] {
            if let (Some(table), Some(column)) = (
                written(&table, table_quoted),
                written(&column, column_quoted),
            ) {
                cases.push((table, column));
            }
        }
    }
    cases
}

fn fail(error: impl std::fmt::Display) -> TestCaseError {
    TestCaseError::fail(error.to_string())
}

#[cfg(feature = "sqlite")]
#[tokio::test(flavor = "multi_thread")]
async fn identifiers_round_trip_through_sqlite() -> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::sqlx::{self, sqlite::SqlitePoolOptions};

    // One connection: every in-memory connection is its own database.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    let handle = tokio::runtime::Handle::current();
    let case = |(table, column): (Written, Written)| -> Result<(), TestCaseError> {
        tokio::task::block_in_place(|| {
            handle.block_on(async {
                let [create, insert, select, drop] = statements(&table, &column);
                let outcome = async {
                    sqlx::query(&create).execute(&pool).await?;
                    sqlx::query(&insert).execute(&pool).await?;
                    let value: i64 = sqlx::query_scalar(&select).fetch_one(&pool).await?;
                    let tables: Vec<String> = sqlx::query_scalar(
                        "SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name;",
                    )
                    .fetch_all(&pool)
                    .await?;
                    let columns: Vec<String> =
                        sqlx::query_scalar("SELECT name FROM pragma_table_info(?1);")
                            .bind(&table.catalog)
                            .fetch_all(&pool)
                            .await?;
                    Ok::<_, sqlx::Error>((value, tables, columns))
                }
                .await;
                let dropped = sqlx::query(&drop).execute(&pool).await;
                let remaining: Vec<String> =
                    sqlx::query_scalar("SELECT name FROM sqlite_schema WHERE type = 'table';")
                        .fetch_all(&pool)
                        .await
                        .map_err(fail)?;
                let (value, tables, columns) =
                    outcome.map_err(|error| fail(format!("{create}: {error}")))?;
                dropped.map_err(|error| fail(format!("{drop}: {error}")))?;
                prop_assert!(remaining.is_empty(), "{} left {:?}", drop, remaining);
                prop_assert_eq!(value, 7);
                prop_assert_eq!(tables, vec![table.catalog.clone()]);
                prop_assert_eq!(columns, vec![column.catalog.clone()]);
                Ok(())
            })
        })
    };
    for retained in retained_written() {
        case(retained.clone()).map_err(|error| format!("{retained:?}: {error}"))?;
    }
    TestRunner::new(db_config(256, 0x7250_0101))
        .run(&table_and_column(), case)
        .map_err(|error| error.to_string())?;
    pool.close().await;
    Ok(())
}

#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread")]
async fn identifiers_round_trip_through_owned_postgres() -> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::sqlx::{self, postgres::PgPoolOptions};

    let mut postgres = owned_postgres::OwnedPostgres::start("identifier-quoting")?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&postgres.url)
        .await?;
    let handle = tokio::runtime::Handle::current();
    let case = |(table, column): (Written, Written)| -> Result<(), TestCaseError> {
        tokio::task::block_in_place(|| {
            handle.block_on(async {
                let [create, insert, select, drop] = statements(&table, &column);
                let catalog = "SELECT table_name::text, column_name::text \
                               FROM information_schema.columns \
                               WHERE table_schema = current_schema() ORDER BY 1, 2;";
                let outcome = async {
                    sqlx::query(&create).execute(&pool).await?;
                    sqlx::query(&insert).execute(&pool).await?;
                    let value: i32 = sqlx::query_scalar(&select).fetch_one(&pool).await?;
                    let names: Vec<(String, String)> =
                        sqlx::query_as(catalog).fetch_all(&pool).await?;
                    Ok::<_, sqlx::Error>((value, names))
                }
                .await;
                let dropped = sqlx::query(&drop).execute(&pool).await;
                let remaining: Vec<(String, String)> = sqlx::query_as(catalog)
                    .fetch_all(&pool)
                    .await
                    .map_err(fail)?;
                let (value, names) = outcome.map_err(|error| fail(format!("{create}: {error}")))?;
                dropped.map_err(|error| fail(format!("{drop}: {error}")))?;
                prop_assert!(remaining.is_empty(), "{} left {:?}", drop, remaining);
                prop_assert_eq!(value, 7);
                prop_assert_eq!(names, vec![(table.catalog.clone(), column.catalog.clone())]);
                Ok(())
            })
        })
    };
    let result = (|| {
        for retained in retained_written() {
            case(retained.clone()).map_err(|error| format!("{retained:?}: {error}"))?;
        }
        TestRunner::new(db_config(128, 0x7250_0102))
            .run(&table_and_column(), case)
            .map_err(|error| error.to_string())
    })();
    pool.close().await;
    postgres.cleanup()?;
    result?;
    Ok(())
}

#[cfg(feature = "mssql")]
#[tokio::test(flavor = "multi_thread")]
async fn identifiers_round_trip_through_owned_sql_server() -> Result<(), Box<dyn std::error::Error>>
{
    use graphql_orm::tokio_util::compat::TokioAsyncWriteCompatExt;

    let mut server = owned_mssql::OwnedSqlServer::start().await?;
    let config = graphql_orm::tiberius::Config::from_ado_string(&server.connection_string)?;
    let tcp = tokio::net::TcpStream::connect(config.get_addr()).await?;
    tcp.set_nodelay(true)?;
    let mut client = graphql_orm::tiberius::Client::connect(config, tcp.compat_write()).await?;
    // Tables the owned server already holds are not part of any case.
    let baseline = client
        .simple_query("SELECT object_id FROM sys.tables WHERE is_ms_shipped = 0;")
        .await?
        .into_first_result()
        .await?
        .into_iter()
        .filter_map(|row| row.get::<i32, _>(0))
        .chain([0])
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let user_tables = format!("t.is_ms_shipped = 0 AND t.object_id NOT IN ({baseline})");
    let catalog = format!(
        "SELECT t.name, c.name FROM sys.tables t \
         JOIN sys.columns c ON c.object_id = t.object_id \
         WHERE {user_tables} ORDER BY t.name, c.name;"
    );
    let count = format!("SELECT COUNT(*) FROM sys.tables t WHERE {user_tables};");
    let client = tokio::sync::Mutex::new(client);
    let handle = tokio::runtime::Handle::current();
    let case = |(table, column): (Written, Written)| -> Result<(), TestCaseError> {
        tokio::task::block_in_place(|| {
            handle.block_on(async {
                let mut client = client.lock().await;
                let [create, insert, select, drop] = statements(&table, &column);
                let outcome = async {
                    client.simple_query(&create).await?.into_results().await?;
                    client.simple_query(&insert).await?.into_results().await?;
                    let value = client
                        .simple_query(&select)
                        .await?
                        .into_row()
                        .await?
                        .and_then(|row| row.get::<i32, _>(0));
                    let names = client
                        .simple_query(catalog.as_str())
                        .await?
                        .into_first_result()
                        .await?
                        .into_iter()
                        .map(|row| {
                            (
                                row.get::<&str, _>(0).unwrap_or_default().to_owned(),
                                row.get::<&str, _>(1).unwrap_or_default().to_owned(),
                            )
                        })
                        .collect::<Vec<_>>();
                    Ok::<_, graphql_orm::tiberius::error::Error>((value, names))
                }
                .await;
                let dropped = match client.simple_query(&drop).await {
                    Ok(stream) => stream.into_results().await.map(|_| ()),
                    Err(error) => Err(error),
                };
                let remaining = client
                    .simple_query(count.as_str())
                    .await
                    .map_err(fail)?
                    .into_row()
                    .await
                    .map_err(fail)?
                    .and_then(|row| row.get::<i32, _>(0));
                let (value, names) = outcome.map_err(|error| fail(format!("{create}: {error}")))?;
                dropped.map_err(|error| fail(format!("{drop}: {error}")))?;
                prop_assert_eq!(remaining, Some(0), "{} left tables", drop);
                prop_assert_eq!(value, Some(7));
                prop_assert_eq!(names, vec![(table.catalog.clone(), column.catalog.clone())]);
                Ok(())
            })
        })
    };
    let result = (|| {
        for retained in retained_written() {
            case(retained.clone()).map_err(|error| format!("{retained:?}: {error}"))?;
        }
        TestRunner::new(db_config(96, 0x7250_0103))
            .run(&table_and_column(), case)
            .map_err(|error| error.to_string())
    })();
    drop(client);
    server.cleanup()?;
    result?;
    Ok(())
}

/// #783: the characters SQL Server reads as part of a regular identifier,
/// checked for every non-ASCII Basic Multilingual Plane character against the
/// owned server. A character the renderer leaves bare must be read bare by
/// the server as that same name, and a character the server reads bare must
/// stay bare, so statement text for every name that worked is unchanged.
#[cfg(feature = "mssql")]
#[tokio::test(flavor = "multi_thread")]
async fn sql_server_regular_identifier_tables_match_the_server()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::tokio_util::compat::TokioAsyncWriteCompatExt;

    let mut server = owned_mssql::OwnedSqlServer::start().await?;
    let result = async {
        let config = graphql_orm::tiberius::Config::from_ado_string(&server.connection_string)?;
        let tcp = tokio::net::TcpStream::connect(config.get_addr()).await?;
        tcp.set_nodelay(true)?;
        let mut client =
            graphql_orm::tiberius::Client::connect(config, tcp.compat_write()).await?;
        // `sys.dm_exec_describe_first_result_set` parses each probe without
        // running it and reports a syntax error as a row, so one set-based
        // query covers every code point. A probe passes only when the server
        // names the column exactly as written.
        let probe = |sql: &str, name: &str| {
            format!(
                "CASE WHEN EXISTS (SELECT 1 FROM sys.dm_exec_describe_first_result_set({sql}, NULL, 0) d \
                 WHERE d.error_number IS NULL AND d.name COLLATE Latin1_General_BIN2 = {name} COLLATE Latin1_General_BIN2) \
                 THEN 1 ELSE 0 END"
            )
        };
        let query = format!(
            "WITH h AS (SELECT v FROM (VALUES (0),(1),(2),(3),(4),(5),(6),(7),(8),(9),(10),(11),(12),(13),(14),(15)) h(v)), \
             c AS (SELECT a.v * 4096 + b.v * 256 + d.v * 16 + e.v AS cp \
                   FROM h a CROSS JOIN h b CROSS JOIN h d CROSS JOIN h e) \
             SELECT cp, {start}, {part} FROM c \
             WHERE cp >= 128 AND (cp < 55296 OR cp > 57343) ORDER BY cp;",
            start = probe("N'SELECT 1 AS ' + NCHAR(cp) + N'x'", "NCHAR(cp) + N'x'"),
            part = probe("N'SELECT 1 AS x' + NCHAR(cp) + N'y'", "N'x' + NCHAR(cp) + N'y'"),
        );
        let rows = client.simple_query(query).await?.into_first_result().await?;
        let mut mismatches = Vec::new();
        let mut checked = 0;
        for row in rows {
            let code_point = row.get::<i32, _>(0).ok_or("code point")?;
            let server_start = row.get::<i32, _>(1) == Some(1);
            let server_part = row.get::<i32, _>(2) == Some(1);
            let character = char::from_u32(code_point as u32).ok_or("scalar value")?;
            let start_name = format!("{character}x");
            let part_name = format!("x{character}y");
            let rendered_start = BACKEND.render_identifier(&start_name) == start_name.as_str();
            let rendered_part = BACKEND.render_identifier(&part_name) == part_name.as_str();
            if (server_start, server_part) != (rendered_start, rendered_part) {
                mismatches.push(format!(
                    "U+{code_point:04X} server start/part {server_start}/{server_part}, \
                     rendered bare {rendered_start}/{rendered_part}"
                ));
            }
            checked += 1;
        }
        assert_eq!(checked, 0x10000 - 0x80 - 0x800, "every non-ASCII BMP scalar value");
        assert!(
            mismatches.is_empty(),
            "{} mismatches, first: {:#?}",
            mismatches.len(),
            &mismatches[..mismatches.len().min(40)]
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    server.cleanup()?;
    result
}
