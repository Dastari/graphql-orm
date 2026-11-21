use graphql_orm::graphql::loaders::{CompositeRelationQueryKey, RelationKey, RelationKeyPartKind};
use graphql_orm::graphql::orm::SqlValue;

// A relation key carries the relation's authorized read; only generated
// relation resolvers build one (#445). A host cannot write one by hand.
fn key() -> CompositeRelationQueryKey {
    let values = vec![SqlValue::String("a".into())];
    CompositeRelationQueryKey {
        relation: "Parent::child",
        parent_key: RelationKey::from_sql_values(&values),
        parent_values: values,
        fk_columns: vec!["\"id\""],
        key_part_kinds: vec![RelationKeyPartKind::String],
        where_signature: None,
        order_signature: None,
        page_signature: None,
        filter: None,
        sorts: vec![],
        pagination: None,
        auth_context: None,
    }
}

fn main() {}
