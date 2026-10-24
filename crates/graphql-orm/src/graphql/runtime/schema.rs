use super::*;
use crate::graphql::orm::*;
use async_graphql::{Value, dynamic::*};
use std::collections::BTreeSet;

// Only the authorized execution path can construct a generated runtime object.
// Host-owned dynamic types retain their own resolver payloads.
struct AuthorizedOutput(Value);

pub(super) fn field_value(value: Value) -> FieldValue<'static> {
    match value {
        Value::Object(_) => FieldValue::owned_any(AuthorizedOutput(value)),
        Value::List(values) => FieldValue::list(values.into_iter().map(field_value)),
        value => FieldValue::value(value),
    }
}
// Colon cannot occur in a GraphQL name. Argument-dependent relation results
// therefore cannot collide with schema-named scalar/helper values.
pub(super) fn relation_output_key(name: &str, response_key: &str) -> String {
    format!("{name}:{response_key}")
}
pub(super) fn output(name: &str, ty: TypeRef) -> Field {
    Field::new(name, ty, |ctx| {
        FieldFuture::new(async move {
            let value = &ctx
                .parent_value
                .downcast_ref::<AuthorizedOutput>()
                .ok_or_else(|| async_graphql::Error::new("projection_mismatch"))?
                .0;
            let selection = ctx.field();
            let key = selection.alias().unwrap_or(selection.name());
            let Value::Object(map) = value else {
                return Err(async_graphql::Error::new("invalid_composition"));
            };
            let value = map
                .get(relation_output_key(selection.name(), key).as_str())
                .or_else(|| map.get(selection.name()))
                .ok_or_else(|| async_graphql::Error::new("projection_mismatch"))?;
            if value == &Value::Null {
                return Ok(None);
            }
            Ok(Some(field_value(value.clone())))
        })
    })
}
pub(super) fn page_args(field: Field, object: &str) -> Field {
    super::semantic::page_arguments(object)
        .into_iter()
        .fold(field, |field, (name, ty)| {
            field.argument(
                InputValue::new(name, type_ref(&ty))
                    .description(super::semantic::argument_description(name)),
            )
        })
}
/// Parse a generated `Name`, `Name!` or `[Name!]` reference.
pub(super) fn type_ref(ty: &str) -> TypeRef {
    if let Some(inner) = ty.strip_prefix('[').and_then(|ty| ty.strip_suffix("!]")) {
        TypeRef::named_nn_list(inner)
    } else if let Some(inner) = ty.strip_suffix('!') {
        TypeRef::named_nn(inner)
    } else {
        TypeRef::named(ty)
    }
}
pub(super) fn root_field(name: &str, object: &str) -> Field {
    page_args(
        Field::new(
            name,
            TypeRef::named_nn(format!("{object}Connection")),
            |ctx| {
                FieldFuture::new(async move {
                    let results = ctx.data::<super::execution::Results>()?;
                    let selection = ctx.field();
                    let key = selection.alias().unwrap_or(selection.name());
                    let value = results
                        .0
                        .get(key)
                        .ok_or_else(|| async_graphql::Error::new("authorization_missing"))?;
                    Ok(Some(field_value(value.clone())))
                })
            },
        )
        .description(super::semantic::list_description(object)),
        object,
    )
}
pub(super) fn helpers(limits: RuntimeScalarLimits) -> Vec<Type> {
    use RuntimeValueKind::*;
    let mut result = Vec::new();
    for kind in [Integer, Uuid, Json, Bytes, DateTime] {
        result.push(
            Scalar::new(scalar::name(kind))
                .description(super::semantic::scalar_description(kind))
                .validator(move |value| scalar::decode(kind, value, limits).is_ok())
                .into(),
        );
    }
    result.push(
        Enum::new("RuntimeOrderDirection")
            .description("Sort direction")
            .item(EnumItem::new("ASC").description("Ascending"))
            .item(EnumItem::new("DESC").description("Descending"))
            .into(),
    );
    result.push(
        Enum::new("RuntimeNullPlacement")
            .description("Placement of null values in an ordering")
            .item(EnumItem::new("FIRST").description("Null values first"))
            .item(EnumItem::new("LAST").description("Null values last"))
            .into(),
    );
    result.push(
        Object::new("RuntimePageInfo")
            .description("Pagination state of one page")
            .field(
                output("hasNextPage", TypeRef::named_nn("Boolean"))
                    .description("Whether more records follow this page"),
            )
            .field(
                output("hasPreviousPage", TypeRef::named_nn("Boolean"))
                    .description("Whether records precede this page"),
            )
            .field(
                output("startCursor", TypeRef::named("String"))
                    .description("Cursor of the first record in this page"),
            )
            .field(
                output("endCursor", TypeRef::named("String"))
                    .description("Cursor of the last record in this page"),
            )
            .into(),
    );
    for kind in [Boolean, Integer, Float, String, Uuid, Json, Bytes, DateTime] {
        result.push(filter(kind).into());
    }
    result
}
/// The `Decimal` scalar and its filter, registered only by compositions that
/// contain a decimal field so other schemas keep their exact SDL.
pub(super) fn decimal_helpers() -> Vec<Type> {
    let kind = RuntimeValueKind::Decimal(DecimalDef::new(18, 0).expect("valid definition"));
    vec![
        Scalar::new(scalar::name(kind))
            .description(super::semantic::scalar_description(kind))
            .validator(|value| match value {
                Value::String(text) => runtime_decimal_syntax(text),
                Value::Number(number) => number.is_i64() || number.is_u64(),
                _ => false,
            })
            .into(),
        filter(kind).into(),
    ]
}
fn filter(kind: RuntimeValueKind) -> InputObject {
    let member = |op: &str, ty: TypeRef| {
        InputValue::new(op, ty).description(super::semantic::filter_member_description(op))
    };
    let mut filter = InputObject::new(filter_name(kind))
        .description(format!("Filter on one {} value", kind.as_str()))
        .field(member("isNull", TypeRef::named("Boolean")));
    if kind != RuntimeValueKind::Json {
        for op in ["eq", "ne"] {
            filter = filter.field(member(op, TypeRef::named(scalar::name(kind))));
        }
        for op in ["in", "notIn"] {
            filter = filter.field(member(op, TypeRef::named_nn_list(scalar::name(kind))));
        }
    }
    if super::semantic::compares(kind) {
        for op in ["lt", "lte", "gt", "gte"] {
            filter = filter.field(member(op, TypeRef::named(scalar::name(kind))));
        }
    }
    if kind == RuntimeValueKind::String {
        for op in ["contains", "startsWith", "endsWith"] {
            filter = filter.field(member(op, TypeRef::named("String")));
        }
    }
    filter
}
pub(super) fn filter_name(kind: RuntimeValueKind) -> String {
    let name = match kind {
        RuntimeValueKind::Boolean => "RuntimeBoolean",
        RuntimeValueKind::Float => "RuntimeFloat",
        RuntimeValueKind::String => "RuntimeString",
        RuntimeValueKind::Decimal(_) => "RuntimeDecimal",
        _ => scalar::name(kind),
    };
    format!("{name}Filter")
}
/// Relation filter input names, one per target object and cardinality in use.
pub(super) fn relation_filter_names(module: &RuntimeGraphqlModule) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let collections = &module.schema.schema().collections;
    for c in collections {
        for relation in &c.relations {
            if !module.options.relation_filters.admits(&relation.id) {
                continue;
            }
            if let Some(target) = collections.iter().find(|t| t.id == relation.target) {
                names.insert(relation_filter_name(
                    &module.object_name(target),
                    relation.cardinality,
                ));
            }
        }
    }
    names
}
fn relation_filter_name(target: &str, cardinality: RelationCardinality) -> String {
    match cardinality {
        RelationCardinality::One => format!("{target}ToOneFilter"),
        RelationCardinality::Many => format!("{target}ToManyFilter"),
    }
}
fn relation_filter_types(module: &RuntimeGraphqlModule) -> Vec<Type> {
    relation_filter_names(module)
        .into_iter()
        .map(|name| {
            let (target, quantifiers): (&str, &[(&str, &str)]) =
                if let Some(target) = name.strip_suffix("ToOneFilter") {
                    (
                        target,
                        &[
                            ("is", "Matches when the related record is visible and matches"),
                            (
                                "isNot",
                                "Matches when no visible related record matches, including when there is none",
                            ),
                        ],
                    )
                } else {
                    (
                        name.strip_suffix("ToManyFilter").unwrap_or(&name),
                        &[
                            ("some", "Matches when at least one visible related record matches"),
                            ("none", "Matches when no visible related record matches"),
                            (
                                "every",
                                "Matches when every visible related record matches, including when there are none",
                            ),
                        ],
                    )
                };
            let mut input = InputObject::new(&name)
                .description(format!("Filter on related {target} records"));
            for (quantifier, description) in quantifiers {
                input = input.field(
                    InputValue::new(*quantifier, TypeRef::named(format!("{target}WhereInput")))
                        .description(*description),
                );
            }
            input.into()
        })
        .collect()
}
pub(super) fn types(module: &RuntimeGraphqlModule) -> Vec<Type> {
    let mut result = relation_filter_types(module);
    for c in &module.schema.schema().collections {
        let name = module.object_name(c);
        let mut object = Object::new(&name).description(c.effective_description());
        let mut filter = InputObject::new(format!("{name}WhereInput"))
            .description(format!("Filter {name} records"))
            .field(
                InputValue::new("and", TypeRef::named_nn_list(format!("{name}WhereInput")))
                    .description("Matches when every filter matches"),
            )
            .field(
                InputValue::new("or", TypeRef::named_nn_list(format!("{name}WhereInput")))
                    .description("Matches when any filter matches"),
            )
            .field(
                InputValue::new("not", TypeRef::named(format!("{name}WhereInput")))
                    .description("Matches when the filter does not match"),
            );
        let mut order_enum =
            Enum::new(format!("{name}OrderField")).description(format!("Sortable {name} fields"));
        for f in &c.fields {
            // A projected field outputs its host object; everything else stays scalar.
            let (name, nullable) = match module.projection(&f.id) {
                Some(binding) => (
                    binding.contract.object.name(),
                    f.nullable || binding.contract.may_be_absent,
                ),
                None => (scalar::name(f.value_kind), f.nullable),
            };
            let ty = if nullable {
                TypeRef::named(name)
            } else {
                TypeRef::named_nn(name)
            };
            let description = f.effective_description();
            object = object.field(output(&f.api_name, ty).description(&description));
            if f.filterable {
                filter = filter.field(
                    InputValue::new(&f.api_name, TypeRef::named(filter_name(f.value_kind)))
                        .description(&description),
                );
            }
            if f.sortable {
                order_enum = order_enum.item(EnumItem::new(&f.api_name).description(description));
            }
        }
        for relation in &c.relations {
            if let Some(target) = module
                .schema
                .schema()
                .collections
                .iter()
                .find(|t| t.id == relation.target)
            {
                let target = module.object_name(target);
                let field = match relation.cardinality {
                    RelationCardinality::One => output(&relation.api_name, TypeRef::named(&target)),
                    RelationCardinality::Many => page_args(
                        output(
                            &relation.api_name,
                            TypeRef::named_nn(format!("{target}Connection")),
                        ),
                        &target,
                    ),
                };
                if module.options.relation_filters.admits(&relation.id) {
                    filter = filter.field(
                        InputValue::new(
                            &relation.api_name,
                            TypeRef::named(relation_filter_name(&target, relation.cardinality)),
                        )
                        .description(relation.effective_description()),
                    );
                }
                object = object.field(field.description(relation.effective_description()));
            }
        }
        result.extend([
            object.into(),
            filter.into(),
            order_enum.into(),
            InputObject::new(format!("{name}OrderInput"))
                .description(format!("One {name} ordering term"))
                .field(
                    InputValue::new("field", TypeRef::named_nn(format!("{name}OrderField")))
                        .description("Field to order by"),
                )
                .field(
                    InputValue::new("direction", TypeRef::named("RuntimeOrderDirection"))
                        .description("Sort direction"),
                )
                .field(
                    InputValue::new("nulls", TypeRef::named("RuntimeNullPlacement"))
                        .description("Placement of null values"),
                )
                .into(),
            Object::new(format!("{name}Edge"))
                .description(format!("One {name} record in a page"))
                .field(
                    output("cursor", TypeRef::named_nn("String"))
                        .description("Opaque cursor of this record"),
                )
                .field(output("node", TypeRef::named_nn(&name)).description("The record"))
                .into(),
            Object::new(format!("{name}Connection"))
                .description(format!("A page of {name} records"))
                .field(
                    output("edges", TypeRef::named_nn_list_nn(format!("{name}Edge")))
                        .description("Records in this page"),
                )
                .field(
                    output("pageInfo", TypeRef::named_nn("RuntimePageInfo"))
                        .description("Pagination state of this page"),
                )
                .field(
                    output("totalCount", TypeRef::named_nn("RuntimeInt64"))
                        .description("Number of records matching the filter"),
                )
                .into(),
        ]);
    }
    result
}

/// ORM-built output type of a projector; fields read only authorized payloads.
pub(super) fn projected_type(object: &super::RuntimeProjectedObject) -> Type {
    let mut result = Object::new(object.name());
    for field in object.fields() {
        let name = scalar::name(field.kind());
        result = result.field(output(
            field.name(),
            if field.nullable() {
                TypeRef::named(name)
            } else {
                TypeRef::named_nn(name)
            },
        ));
    }
    result.into()
}
