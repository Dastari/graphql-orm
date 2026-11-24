//! A VARCHAR comparison needs both a lossless conversion and the same sort
//! rules as the Unicode comparison. SQL Server supplies the physical collation.
use super::{MssqlClient, StatementFailure, statement_failure};
use crate::graphql::orm::{
    DatabaseBackend, MssqlParameterType, SqlToken, SqlTokenKind, SqlValue, scan_sql,
};
use std::{
    collections::HashMap,
    fmt::Write,
    ops::Range,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tiberius::Query;

const MAX_CACHED_TABLES: usize = 128;
const COLLATION_CACHE_TTL: Duration = Duration::from_secs(300);
// Each operand adds several references and conversions to both proof branches.
const MAX_MEMBERSHIP_OPERANDS: usize = 32;

type Collations = HashMap<String, String>;

struct CachedCollations {
    expires_at: Instant,
    columns: Arc<Collations>,
}

#[derive(Default)]
pub(super) struct CollationCache(Mutex<HashMap<String, CachedCollations>>);

impl CollationCache {
    fn get(&self, table: &str) -> Option<Arc<Collations>> {
        let entries = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let entry = entries.get(table)?;
        (entry.expires_at > Instant::now()).then(|| Arc::clone(&entry.columns))
    }

    fn insert(&self, table: &str, columns: Arc<Collations>) {
        let mut entries = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if entries.len() == MAX_CACHED_TABLES
            && !entries.contains_key(table)
            && let Some((oldest, _)) = entries.iter().min_by_key(|(_, entry)| entry.expires_at)
        {
            let oldest = oldest.clone();
            entries.remove(&oldest);
        }
        entries.insert(
            table.to_owned(),
            CachedCollations {
                expires_at: Instant::now() + COLLATION_CACHE_TTL,
                columns,
            },
        );
    }
}

struct Operand {
    span: Range<usize>,
    width: Option<usize>,
}

struct Comparison {
    span: Range<usize>,
    column: String,
    operands: Vec<Operand>,
}

struct ComparisonSelect<'a> {
    table: &'a str,
    comparisons: Vec<Comparison>,
}

pub(super) async fn render(
    client: &mut MssqlClient,
    cache: &CollationCache,
    sql: &str,
    values: &[SqlValue],
) -> Result<Option<String>, StatementFailure> {
    if !values.iter().any(|value| varchar_width(value).is_some()) {
        return Ok(None);
    }
    let Some(select) = ComparisonSelect::parse(sql, values) else {
        return Ok(None);
    };
    if let Some(collations) = cache.get(select.table) {
        return Ok(select.with_conversion(sql, &collations));
    }

    let mut query = Query::new(
        "SELECT c.name, c.collation_name FROM sys.columns AS c \
         WHERE c.object_id = OBJECT_ID(@P1, N'U') AND c.system_type_id = 167",
    );
    query.bind(select.table);
    let rows = match query.query(client).await {
        Ok(stream) => stream.into_first_result().await,
        Err(error) => Err(error),
    };
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            let failure = statement_failure(error);
            return if failure.reusable {
                Ok(None)
            } else {
                Err(failure)
            };
        }
    };
    let collations = rows
        .iter()
        .filter_map(|row| {
            let column = row.try_get::<&str, _>(0).ok()??;
            let collation = row.try_get::<&str, _>(1).ok()??;
            compatible_collation(collation).then(|| (column.to_owned(), collation.to_owned()))
        })
        .collect();
    let collations = Arc::new(collations);
    cache.insert(select.table, Arc::clone(&collations));
    Ok(select.with_conversion(sql, &collations))
}

fn compatible_collation(name: &str) -> bool {
    // SQL collations can use different comparison rules for VARCHAR and
    // NVARCHAR even when each operand has a perfect code-page round trip.
    // Binary collations can distinguish byte sequences that decode to the
    // same Unicode text. Use Windows linguistic collations.
    !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !name.to_ascii_uppercase().starts_with("SQL_")
        && !name
            .split('_')
            .any(|part| part.eq_ignore_ascii_case("BIN") || part.eq_ignore_ascii_case("BIN2"))
}

fn varchar_width(value: &SqlValue) -> Option<Option<usize>> {
    let SqlValue::MssqlTyped(typed) = value else {
        return None;
    };
    let MssqlParameterType::VarChar(width) = typed.parameter_type() else {
        return None;
    };
    let SqlValue::String(value) = typed.value() else {
        return None;
    };
    if value.chars().any(char::is_control) {
        return None;
    }
    // Allow for operands that exceed the declared width. The server
    // round-trip also catches truncation if the target encoding uses more
    // bytes than the operand's UTF-8 representation.
    Some(
        width
            .map(|width| {
                if value.len() <= usize::from(width) {
                    usize::from(width)
                } else {
                    8000
                }
            })
            .filter(|_| value.len() <= 8000),
    )
}

impl<'a> ComparisonSelect<'a> {
    fn parse(sql: &'a str, values: &[SqlValue]) -> Option<Self> {
        let mut tokens = Vec::new();
        scan_sql(DatabaseBackend::Mssql, sql, |token| tokens.push(token)).ok()?;
        if tokens
            .iter()
            .any(|token| token.kind == SqlTokenKind::Comment)
        {
            return None;
        }
        let word = |token: &SqlToken| &sql[token.start..token.end];
        if !word(tokens.first()?).eq_ignore_ascii_case("SELECT") {
            return None;
        }
        if word(tokens.last()?) == ";" {
            tokens.pop();
        }
        // Recognize a single, direct SELECT. Any SQL with a different name
        // resolution scope (joins, subqueries, CTEs, lateral sources) keeps
        // its original parameters.
        let mut from = None;
        let mut filter_start = None;
        let mut filter_end = None;
        let mut depth = 0usize;
        for (index, token) in tokens.iter().enumerate().skip(1) {
            let text = word(token);
            match text {
                "(" => depth += 1,
                ")" => depth = depth.checked_sub(1)?,
                ";" => return None,
                _ if text.eq_ignore_ascii_case("SELECT") => {
                    // The generated unqualified paging order uses (SELECT 1).
                    let order = filter_end?;
                    if index != order + 3
                        || !word(&tokens[order]).eq_ignore_ascii_case("ORDER")
                        || !word(&tokens[order + 1]).eq_ignore_ascii_case("BY")
                        || word(&tokens[order + 2]) != "("
                        || word(tokens.get(index + 1)?) != "1"
                        || word(tokens.get(index + 2)?) != ")"
                    {
                        return None;
                    }
                }
                _ if ["UNION", "EXCEPT", "INTERSECT", "COLLATE", "CASE"]
                    .iter()
                    .any(|keyword| text.eq_ignore_ascii_case(keyword)) =>
                {
                    return None;
                }
                _ if depth == 0 && text.eq_ignore_ascii_case("FROM") => {
                    if from.replace(index).is_some() {
                        return None;
                    }
                }
                _ if depth == 0 && text.eq_ignore_ascii_case("WHERE") => {
                    if filter_start.replace(index).is_some() {
                        return None;
                    }
                }
                _ if depth == 0
                    && filter_start.is_some()
                    && filter_end.is_none()
                    && ["ORDER", "GROUP", "OPTION", "HAVING", "FOR"]
                        .iter()
                        .any(|keyword| text.eq_ignore_ascii_case(keyword)) =>
                {
                    filter_end = Some(index);
                }
                _ => {}
            }
        }
        if depth != 0 {
            return None;
        }
        let where_index = filter_start?;
        let from_index = from?.checked_add(1)?;
        let table_tokens = tokens.get(from_index..where_index)?;
        match table_tokens {
            [table] => {
                identifier(sql, table)?;
            }
            [schema, dot, table] if word(dot) == "." => {
                identifier(sql, schema)?;
                identifier(sql, table)?;
            }
            _ => return None,
        }
        let start = table_tokens.first()?.start;
        let end = table_tokens.last()?.end;
        let table = &sql[start..end];
        // Only copy a contiguous table name into OBJECT_ID.
        if table_tokens
            .windows(2)
            .any(|pair| pair[0].end != pair[1].start)
        {
            return None;
        }
        let filter_tokens = tokens.get(where_index + 1..filter_end.unwrap_or(tokens.len()))?;
        let comparisons = comparisons(sql, filter_tokens, &tokens, values);
        (!comparisons.is_empty()).then_some(Self { table, comparisons })
    }

    fn with_conversion(self, sql: &str, collations: &Collations) -> Option<String> {
        let mut output = String::new();
        let mut copied = 0;
        let table = self.table.replace('\'', "''");
        for comparison in self.comparisons {
            let Some(collation) = collations.get(comparison.column.as_str()) else {
                continue;
            };
            let column = comparison.column.replace('\'', "''");
            // Check the physical collation again under the query's schema
            // stability lock, since a pool's cached result may predate DDL.
            let mut proof = format!(
                "ISNULL((SELECT TOP (1) c.collation_name FROM sys.columns AS c \
                 WHERE c.object_id = OBJECT_ID(N'{table}', N'U') AND c.system_type_id = 167 \
                 AND c.name COLLATE Latin1_General_100_BIN2 = N'{column}' \
                 AND DATALENGTH(c.name) = DATALENGTH(N'{column}')), N'') \
                 COLLATE Latin1_General_100_BIN2 = N'{collation}'",
            );
            let mut converted = String::new();
            let mut end = comparison.span.start;
            for operand in comparison.operands {
                let parameter = &sql[operand.span.clone()];
                let length = operand
                    .width
                    .map_or_else(|| "max".into(), |n| n.to_string());
                // Set the collation on the NVARCHAR input before converting;
                // setting it afterward would encode under the database default first.
                let conversion =
                    format!("CONVERT(varchar({length}), {parameter} COLLATE {collation})");
                let round_trip = format!("CONVERT(nvarchar(max), {conversion})");
                write!(
                    proof,
                    " AND DATALENGTH({parameter}) = DATALENGTH({round_trip}) AND \
                     CONVERT(varbinary(max), {parameter}) = CONVERT(varbinary(max), {round_trip})",
                )
                .expect("writing to a string");
                converted.push_str(&sql[end..operand.span.start]);
                converted.push_str(&conversion);
                end = operand.span.end;
            }
            converted.push_str(&sql[end..comparison.span.end]);
            output.push_str(&sql[copied..comparison.span.start]);
            let legacy = &sql[comparison.span.clone()];
            // Gate each comparison independently. Keeping the original
            // comparison also preserves SQL's NULL behavior for IN and NOT IN.
            write!(
                output,
                "((({proof}) AND ({converted})) OR (NOT ({proof}) AND ({legacy})))",
            )
            .expect("writing to a string");
            copied = comparison.span.end;
        }
        if copied == 0 {
            return None;
        }
        output.push_str(&sql[copied..]);
        Some(output)
    }
}

fn identifier(sql: &str, token: &SqlToken) -> Option<String> {
    let text = &sql[token.start..token.end];
    match token.kind {
        SqlTokenKind::QuotedIdentifier if text.starts_with('[') => {
            Some(DatabaseBackend::Mssql.identifier_part_name(text))
        }
        SqlTokenKind::Word
            if text.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') =>
        {
            Some(text.to_owned())
        }
        _ => None,
    }
}

fn placeholder_index(sql: &str, token: &SqlToken) -> Option<usize> {
    if token.kind != SqlTokenKind::Placeholder {
        return None;
    }
    let text = &sql[token.start..token.end];
    text.strip_prefix("@P")
        .or_else(|| text.strip_prefix("@p"))?
        .parse::<usize>()
        .ok()?
        .checked_sub(1)
}

fn comparisons(
    sql: &str,
    filter: &[SqlToken],
    tokens: &[SqlToken],
    values: &[SqlValue],
) -> Vec<Comparison> {
    let mut saw = vec![false; values.len()];
    let mut repeated = vec![false; values.len()];
    for index in tokens
        .iter()
        .filter_map(|token| placeholder_index(sql, token))
    {
        if let Some(seen) = saw.get_mut(index) {
            repeated[index] |= *seen;
            *seen = true;
        }
    }
    let token_text = |token: &SqlToken| &sql[token.start..token.end];
    let mut comparisons = Vec::new();
    for index in 0..filter.len() {
        // A generated comparison starts at a Boolean boundary and has a plain
        // identifier on its left. Expressions and explicit COLLATE stay Unicode.
        if index > 0
            && !["(", "AND", "OR", "NOT"]
                .iter()
                .any(|word| token_text(&filter[index - 1]).eq_ignore_ascii_case(word))
        {
            continue;
        }
        let Some(column) = identifier(sql, &filter[index]) else {
            continue;
        };
        let tail = &filter[index + 1..];
        let mut tail_text = tail.iter().map(token_text);
        let params = match tail_text.next().map(str::to_ascii_uppercase).as_deref() {
            Some("=") => scalar_parameter(sql, &tail[1..]),
            Some("!") if tail_text.next() == Some("=") => scalar_parameter(sql, &tail[2..]),
            Some("IN") => list_parameters(sql, &tail[1..]),
            Some("NOT")
                if tail_text
                    .next()
                    .is_some_and(|text| text.eq_ignore_ascii_case("IN")) =>
            {
                list_parameters(sql, &tail[2..])
            }
            _ => None,
        };
        let Some((params, end)) = params else {
            continue;
        };
        let operands = params
            .into_iter()
            .map(|token| {
                let index = placeholder_index(sql, token)?;
                let width = varchar_width(values.get(index)?)?;
                (!repeated[index]).then_some(Operand {
                    span: token.start..token.end,
                    width,
                })
            })
            .collect::<Option<Vec<_>>>();
        if let Some(operands) = operands {
            comparisons.push(Comparison {
                span: filter[index].start..end,
                column,
                operands,
            });
        }
    }
    comparisons
}

fn scalar_parameter<'a>(sql: &str, tokens: &'a [SqlToken]) -> Option<(Vec<&'a SqlToken>, usize)> {
    let (parameter, rest) = tokens.split_first()?;
    boolean_boundary(sql, rest).then(|| (vec![parameter], parameter.end))
}

fn list_parameters<'a>(sql: &str, tokens: &'a [SqlToken]) -> Option<(Vec<&'a SqlToken>, usize)> {
    let text = |token: &SqlToken| &sql[token.start..token.end];
    let (open, mut list) = tokens.split_first()?;
    if text(open) != "(" {
        return None;
    }
    let mut parameters = Vec::new();
    while let [parameter, separator, rest @ ..] = list {
        if parameters.len() == MAX_MEMBERSHIP_OPERANDS {
            return None;
        }
        placeholder_index(sql, parameter)?;
        parameters.push(parameter);
        match text(separator) {
            ")" if boolean_boundary(sql, rest) => return Some((parameters, separator.end)),
            "," => list = rest,
            _ => return None,
        }
    }
    None
}

fn boolean_boundary(sql: &str, tokens: &[SqlToken]) -> bool {
    tokens.first().is_none_or(|token| {
        let text = &sql[token.start..token.end];
        text == ")" || text.eq_ignore_ascii_case("AND") || text.eq_ignore_ascii_case("OR")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::{filters::StringFilter, orm::__gom_render_string_filter};

    fn typed_value() -> SqlValue {
        let mut values = Vec::new();
        __gom_render_string_filter(
            &StringFilter {
                eq: Some("café".into()),
                ..Default::default()
            },
            DatabaseBackend::Mssql,
            "code",
            "VARCHAR(32)",
            &mut Vec::new(),
            &mut values,
        );
        values.pop().unwrap()
    }

    #[test]
    fn only_windows_linguistic_collations_are_eligible() {
        for name in [
            "SQL_Latin1_General_CP1_CI_AS",
            "Latin1_General_BIN",
            "Japanese_XJIS_100_BIN2",
            "Latin1_General_100_BIN2_UTF8",
            "latin1_general_bin2",
        ] {
            assert!(!compatible_collation(name), "{name}");
        }
        for name in ["Latin1_General_100_CI_AS_SC_UTF8", "Greek_100_CI_AS"] {
            assert!(compatible_collation(name), "{name}");
        }
    }

    #[test]
    fn parser_requires_a_direct_unqualified_comparison() {
        let values = [typed_value()];
        for sql in [
            "SELECT id FROM dbo.Items WHERE code = @P1",
            "SELECT [id] FROM [dbo].[Items] WHERE ([code] = @P1) ORDER BY (SELECT 1) OFFSET 0 ROWS",
        ] {
            assert!(ComparisonSelect::parse(sql, &values).is_some(), "{sql}");
        }
        for sql in [
            "SELECT id FROM dbo.Items /* comment */ WHERE code = @P1",
            "SELECT id FROM dbo.Items WHERE code = @P1 -- comment",
            "SELECT id FROM dbo.Items JOIN Extra ON Items.id = Extra.id WHERE code = @P1",
            "SELECT id FROM dbo.Items WHERE code = @P1 AND EXISTS (SELECT 1 FROM Extra)",
            "SELECT id FROM dbo.Items WHERE code = @P1 ORDER BY (SELECT MAX(id) FROM Extra)",
            "SELECT id FROM dbo.Items WHERE code = @P1 UNION SELECT id FROM Extra",
            "SELECT id FROM dbo.Items WHERE Items.code = @P1",
            "SELECT id FROM dbo.Items AS i WHERE i.code = @P1",
            "SELECT id FROM dbo.Items WHERE code COLLATE Latin1_General_BIN = @P1",
            "SELECT id FROM dbo.Items WHERE code = @P1 COLLATE Latin1_General_BIN",
            "SELECT id FROM dbo.Items WHERE CASE WHEN 1 = 1 AND code = @P1 THEN 1 END = 1",
        ] {
            assert!(ComparisonSelect::parse(sql, &values).is_none(), "{sql}");
        }
    }

    #[test]
    fn membership_rewrite_has_an_operand_limit() {
        let values = vec![typed_value(); 2000];
        for operator in ["IN", "NOT IN"] {
            for (count, eligible) in [(32, true), (33, false), (2000, false)] {
                let parameters = (1..=count)
                    .map(|n| format!("@P{n}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!("SELECT id FROM dbo.Items WHERE code {operator} ({parameters})");
                assert_eq!(
                    ComparisonSelect::parse(&sql, &values[..count]).is_some(),
                    eligible,
                    "{operator} with {count} operands",
                );
            }
        }
    }
}
