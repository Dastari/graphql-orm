#!/usr/bin/env python3
"""Generate a disposable, application-neutral production-runtime compile fixture."""
import argparse
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('output', type=Path)
p.add_argument('--entities', type=int, default=56)
a = p.parse_args()
if a.entities < 2 or a.entities % 2:
    p.error('--entities must be positive and even')
root = Path(__file__).resolve().parents[1]
a.output.mkdir(parents=True, exist_ok=True)
src = a.output / 'src'
src.mkdir(exist_ok=True)
manifest = (root / 'crates/graphql-orm-macros/fixtures/full-stack/Cargo.toml').read_text()
manifest = manifest.replace('full-stack-fixture', 'compile-memory-fixture')
manifest = manifest.replace('path = "../.."', f'path = "{root}/crates/graphql-orm-macros"')
manifest = manifest.replace('path = "../../../graphql-orm"', f'path = "{root}/crates/graphql-orm"')
(a.output / 'Cargo.toml').write_text(manifest)
for i in range(a.entities):
    name, peer = f'Record{i:02}', f'Record{i ^ 1:02}'
    lines = ['use super::*;', '', '#[derive(GraphQLEntity, GraphQLRelations, GraphQLOperations,',
             '    SimpleObject, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]',
             '#[graphql(complex)]',
             f'#[graphql_entity(table = "records_{i:02}", plural = "Records{i:02}", aggregate = true,',
             '    read_policy = "record.read", write_policy = "record.write")]',
             f'pub struct {name} {{', '    #[primary_key]', '    pub id: String,',
             '    #[filterable(type = "string")]', '    pub peer_id: String,']
    for j in range(24):
        typ, filter_type = [('String', 'string'), ('i64', 'number'), ('bool', 'boolean')][j % 3]
        lines += [f'    /// Persisted attribute {j}; validated by the host before writes.',
                  f'    #[filterable(type = "{filter_type}")]', '    #[sortable]',
                  f'    pub field_{j:02}: {typ},', '']
    lines += ['    #[graphql(skip)]', f'    #[relation(target = "{peer}", from = "peer_id", to = "id", multiple)]',
              f'    pub related: Vec<{peer}>,', '}', '', f'impl {name} {{']
    for j in range(24):
        expr = [f'!self.field_{j:02}.trim().is_empty()', f'self.field_{j:02} >= 0', f'self.field_{j:02}'][j % 3]
        lines += [f'    /// Validate attribute {j} before accepting an imported record.',
                  f'    pub fn valid_{j:02}(&self) -> bool {{', f'        {expr}', '    }', '']
    lines += ['}', '', f'impl BatchLoadEntity for {name} {{',
              '    fn batch_column() -> &\'static str { "id" }',
              '    fn batch_key_from_row(row: &DbRow) -> Result<String, sqlx::Error> {',
              '        use sqlx::Row;', '        row.try_get("id")', '    }', '}']
    (src / f'record_{i:02}.rs').write_text('\n'.join(lines) + '\n')
lib = ['use async_graphql::SimpleObject;', 'use graphql_orm::prelude::*;',
       '#[cfg(feature = "sqlite")]', 'type DbRow = sqlx::sqlite::SqliteRow;',
       '#[cfg(feature = "postgres")]', 'type DbRow = sqlx::postgres::PgRow;']
for i in range(a.entities):
    lib += [f'mod record_{i:02};', f'pub use record_{i:02}::*;']
lib += ['schema_roots! { query_custom_ops: [], entities: [' + ', '.join(f'Record{i:02}' for i in range(a.entities)) + '] }',
        'pub fn export_sdl() -> String {',
        '    async_graphql::Schema::build(QueryRoot::default(), MutationRoot::default(), SubscriptionRoot::default()).finish().sdl()', '}']
(src / 'lib.rs').write_text('\n'.join(lib) + '\n')
print(f'{a.entities} entity modules; {sum(len(f.read_text().splitlines()) for f in src.glob("*.rs"))} source lines')
