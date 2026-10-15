use graphql_orm::graphql::orm::*;
use runtime_public_api_boundary_probe::schema;
#[test]
fn complete_identity_is_valid_without_public_filter_permission() {
    let schema = schema();
    let collection = schema.resolve_collection(&CollectionId::new("records").unwrap()).unwrap();
    let id = schema.resolve_field(&collection, &FieldId::new("id").unwrap()).unwrap();
    let key = schema.runtime_key(&collection, &[(id.clone(), RuntimeValue::Integer(7))]).unwrap();
    assert_eq!(key.fields().len(), 1);
    let error = schema.runtime_compare(&collection, &id, RuntimeScalarOperator::Eq,
        RuntimeValue::Integer(7), RuntimeQueryLimits::default()).unwrap_err();
    assert_eq!(error.code(), RuntimeQueryErrorCode::InvalidFilter);
}
#[test]
fn referenced_fields_are_not_original_empty_membership_intent() {
    let schema = schema();
    let collection = schema.resolve_collection(&CollectionId::new("records").unwrap()).unwrap();
    let tenant = schema.resolve_field(&collection, &FieldId::new("tenant").unwrap()).unwrap();
    let predicate = schema.runtime_list(&collection, &tenant, RuntimeListOperator::In,
        vec![], RuntimeQueryLimits::default()).unwrap();
    // Existing documented constant lowering is valid SQL behavior; this public
    // dependency accessor cannot replace the separate private GraphQL intent.
    assert!(predicate.referenced_fields().is_empty());
}
