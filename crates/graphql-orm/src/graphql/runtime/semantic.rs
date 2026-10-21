//! Semantic catalogue entries and SDL descriptions of a runtime module.
//!
//! Every generated type, field, relation and root is described. Collection,
//! field and relation text is the IR's effective description, so SDL,
//! introspection and the semantic catalogue carry the same text. Root
//! operation and argument descriptions follow the static derive wording.
use super::*;
use crate::graphql::orm::*;
use std::collections::BTreeSet;

/// Static-derive argument description: `orderBy` becomes `Order by`.
pub(super) fn argument_description(name: &str) -> String {
    let mut output = String::new();
    for (index, character) in name.chars().enumerate() {
        if index > 0 && character.is_uppercase() {
            output.push(' ');
        }
        if index == 0 {
            output.extend(character.to_uppercase());
        } else {
            output.extend(character.to_lowercase());
        }
    }
    output
}

pub(super) fn list_description(object: &str) -> String {
    format!("Page through {object} records")
}

pub(super) fn mutation_description(action: RuntimeMutationAction, object: &str) -> String {
    match action {
        RuntimeMutationAction::Create => format!("Create one {object} records"),
        RuntimeMutationAction::Update => format!("Update one {object} records"),
        RuntimeMutationAction::Delete => format!("Delete one {object} records"),
    }
}

pub(super) fn scalar_description(kind: RuntimeValueKind) -> &'static str {
    match kind {
        RuntimeValueKind::Integer => "Signed 64-bit integer encoded as a decimal string",
        RuntimeValueKind::Uuid => "UUID; output in lowercase hyphenated form",
        RuntimeValueKind::Json => "JSON value encoded as a JSON text string",
        RuntimeValueKind::Bytes => "Bytes encoded as padded standard base64",
        RuntimeValueKind::DateTime => {
            "UTC timestamp; output as YYYY-MM-DDTHH:MM:SS.ffffffZ with microsecond precision"
        }
        RuntimeValueKind::Boolean => "Boolean value",
        RuntimeValueKind::Float => "Finite double-precision number",
        RuntimeValueKind::String => "UTF-8 text",
        RuntimeValueKind::Decimal(_) => {
            "Exact decimal; output as a string with exactly the field's scale of fractional \
             digits. Input is a decimal string or an integer and is never rounded"
        }
    }
}

pub(super) fn filter_member_description(operator: &str) -> &'static str {
    match operator {
        "isNull" => "Matches null values when true and non-null values when false",
        "eq" => "Equal to the value",
        "ne" => "Not equal to the value",
        "in" => "Equal to one of the values",
        "notIn" => "Equal to none of the values",
        "lt" => "Less than the value",
        "lte" => "Less than or equal to the value",
        "gt" => "Greater than the value",
        "gte" => "Greater than or equal to the value",
        "contains" => "Contains the text",
        "startsWith" => "Starts with the text",
        "endsWith" => "Ends with the text",
        _ => "Filter operand",
    }
}

/// Comparison operators exposed by the runtime filter input of a kind.
pub(super) fn compares(kind: RuntimeValueKind) -> bool {
    matches!(
        kind,
        RuntimeValueKind::Integer
            | RuntimeValueKind::Float
            | RuntimeValueKind::String
            | RuntimeValueKind::DateTime
            | RuntimeValueKind::Decimal(_)
    )
}

/// Semantic filter operators matching the runtime filter input of a kind.
/// `notIn` has no semantic operator and is not advertised.
fn filter_operators(kind: RuntimeValueKind) -> Vec<GraphqlSemanticFilterOperator> {
    use GraphqlSemanticFilterOperator::*;
    let mut operators = BTreeSet::from([IsNull]);
    if kind != RuntimeValueKind::Json {
        operators.extend([Equal, NotEqual, In]);
    }
    if compares(kind) {
        operators.extend([LessThan, LessThanOrEqual, GreaterThan, GreaterThanOrEqual]);
    }
    if kind == RuntimeValueKind::String {
        operators.extend([Contains, StartsWith, EndsWith]);
    }
    operators.into_iter().collect()
}

fn argument(name: &str, ty: &str) -> GraphqlSemanticArgumentDescriptor {
    GraphqlSemanticArgumentDescriptor {
        graphql_name: name.to_owned(),
        description: argument_description(name),
        type_ref: parse_graphql_type(ty).expect("generated runtime argument types parse"),
    }
}

/// Root list arguments, in SDL order.
pub(super) fn page_arguments(object: &str) -> [(&'static str, String); 6] {
    [
        ("where", format!("{object}WhereInput")),
        ("orderBy", format!("[{object}OrderInput!]")),
        ("first", "Int".to_owned()),
        ("after", "String".to_owned()),
        ("last", "Int".to_owned()),
        ("before", "String".to_owned()),
    ]
}

/// Mutation root arguments of one action, in SDL order.
pub(super) fn mutation_arguments(
    action: RuntimeMutationAction,
    object: &str,
) -> Vec<(&'static str, String)> {
    let mut arguments = vec![("expectedSchema", "String!".to_owned())];
    if action != RuntimeMutationAction::Create {
        arguments.push(("key", format!("{object}KeyInput!")));
        arguments.push(("expected", format!("{object}WhereInput")));
    }
    match action {
        RuntimeMutationAction::Create => arguments.push(("input", format!("Create{object}Input!"))),
        RuntimeMutationAction::Update => arguments.push(("input", format!("Update{object}Input!"))),
        RuntimeMutationAction::Delete => {}
    }
    arguments
}

impl RuntimeGraphqlModule {
    /// Semantic entities of every collection, named by their GraphQL object.
    ///
    /// Descriptions, classification and export come from the runtime IR; field
    /// facts describe exactly what the generated runtime GraphQL exposes. There
    /// are no runtime aggregates, so no field is groupable or aggregatable.
    /// To-many relations and projector-served fields are omitted: their result
    /// shapes (a connection, a host object) have no semantic representation.
    pub fn semantic_entities(&self) -> Vec<GraphqlEntitySemanticMetadata> {
        let collections = &self.schema.schema().collections;
        collections
            .iter()
            .map(|c| {
                let mut fields = Vec::new();
                for f in &c.fields {
                    if self.projection(&f.id).is_some() {
                        continue;
                    }
                    fields.push(GraphqlSemanticFieldMetadata {
                        field_name: f.api_name.clone(),
                        description: f.effective_description(),
                        type_ref: GraphqlSemanticTypeRef::named(
                            scalar::name(f.value_kind),
                            GraphqlSemanticTypeKind::Scalar,
                            f.nullable,
                        ),
                        selectable: true,
                        filter_operators: if f.filterable {
                            filter_operators(f.value_kind)
                        } else {
                            Vec::new()
                        },
                        sortable: f.sortable,
                        groupable: false,
                        aggregate_operators: Vec::new(),
                        aggregate_value_kind: None,
                        relationship: None,
                        classification: f.effective_classification(c),
                        export: f.effective_export(c),
                        // Runtime read authorities grant projections per request.
                        has_field_policy: true,
                    });
                }
                for relation in &c.relations {
                    if relation.cardinality != RelationCardinality::One {
                        continue;
                    }
                    let Some(target) = collections.iter().find(|t| t.id == relation.target) else {
                        continue;
                    };
                    let target = self.object_name(target);
                    fields.push(GraphqlSemanticFieldMetadata {
                        field_name: relation.api_name.clone(),
                        description: relation.effective_description(),
                        type_ref: GraphqlSemanticTypeRef::named(
                            &target,
                            GraphqlSemanticTypeKind::Object,
                            true,
                        ),
                        selectable: true,
                        filter_operators: Vec::new(),
                        sortable: false,
                        groupable: false,
                        aggregate_operators: Vec::new(),
                        aggregate_value_kind: None,
                        relationship: Some(GraphqlSemanticRelationshipDescriptor {
                            target,
                            cardinality: GraphqlSemanticRelationshipCardinality::One,
                            arguments: Vec::new(),
                            collection_bound: None,
                        }),
                        classification: relation.effective_classification(c),
                        export: relation.effective_export(c),
                        has_field_policy: true,
                    });
                }
                GraphqlEntitySemanticMetadata {
                    entity_name: self.object_name(c),
                    description: c.effective_description(),
                    default_classification: c.effective_classification(),
                    fields: fields.into_boxed_slice(),
                }
            })
            .collect()
    }

    /// Semantic descriptors of every root this module installs: one keyset list
    /// query per collection and, when mutations are enabled, each enabled
    /// mutation. Mutations are [`AiMutationExecutionPolicy::Prohibited`].
    pub fn semantic_operations(
        &self,
    ) -> Result<Vec<GraphqlSemanticOperationDescriptor>, RuntimeGraphqlError> {
        let invalid = |_| RuntimeGraphqlError::new("invalid_semantic_catalog");
        let mut operations = Vec::new();
        for c in &self.schema.schema().collections {
            let object = self.object_name(c);
            operations.push(
                GraphqlSemanticOperationDescriptor::generated(
                    GraphqlOperationKind::Query,
                    GeneratedGraphqlOperationCategory::KeysetList,
                    &object,
                    self.root_name(c),
                    list_description(&object),
                    page_arguments(&object)
                        .iter()
                        .map(|(name, ty)| argument(name, ty))
                        .collect(),
                    GraphqlSemanticTypeRef::named(
                        format!("{object}Connection"),
                        GraphqlSemanticTypeKind::Object,
                        false,
                    ),
                    true,
                )
                .map_err(invalid)?,
            );
            for (prefix, action) in mutation::actions(self) {
                let category = match action {
                    RuntimeMutationAction::Create => GeneratedGraphqlOperationCategory::Create,
                    RuntimeMutationAction::Update => GeneratedGraphqlOperationCategory::Update,
                    RuntimeMutationAction::Delete => GeneratedGraphqlOperationCategory::Delete,
                };
                operations.push(
                    GraphqlSemanticOperationDescriptor::generated(
                        GraphqlOperationKind::Mutation,
                        category,
                        &object,
                        format!("{prefix}{object}"),
                        mutation_description(action, &object),
                        mutation_arguments(action, &object)
                            .iter()
                            .map(|(name, ty)| argument(name, ty))
                            .collect(),
                        GraphqlSemanticTypeRef::named(
                            format!("{object}MutationPayload"),
                            GraphqlSemanticTypeKind::Object,
                            false,
                        ),
                        true,
                    )
                    .map_err(invalid)?,
                );
            }
        }
        Ok(operations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argument_descriptions_follow_the_static_derive() {
        assert_eq!(argument_description("orderBy"), "Order by");
        assert_eq!(argument_description("expectedSchema"), "Expected schema");
        assert_eq!(argument_description("first"), "First");
    }

    #[test]
    fn advertised_operators_are_canonical() {
        for kind in [
            RuntimeValueKind::Boolean,
            RuntimeValueKind::Integer,
            RuntimeValueKind::String,
            RuntimeValueKind::Json,
        ] {
            let operators = filter_operators(kind);
            assert!(operators.windows(2).all(|pair| pair[0] < pair[1]));
        }
        assert_eq!(
            filter_operators(RuntimeValueKind::Json),
            vec![GraphqlSemanticFilterOperator::IsNull]
        );
    }
}
