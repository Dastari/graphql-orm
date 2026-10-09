use super::*;
use crate::graphql::orm::*;
use async_graphql::{Value, dynamic::*};

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
fn output(name: &str, ty: TypeRef) -> Field {
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
                .get(key)
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
    field
        .argument(InputValue::new(
            "where",
            TypeRef::named(format!("{object}WhereInput")),
        ))
        .argument(InputValue::new(
            "orderBy",
            TypeRef::named_nn_list(format!("{object}OrderInput")),
        ))
        .argument(InputValue::new("first", TypeRef::named("Int")))
        .argument(InputValue::new("after", TypeRef::named("String")))
        .argument(InputValue::new("last", TypeRef::named("Int")))
        .argument(InputValue::new("before", TypeRef::named("String")))
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
        ),
        object,
    )
}
pub(super) fn helpers(limits: RuntimeScalarLimits) -> Vec<Type> {
    use RuntimeValueKind::*;
    let mut result = Vec::new();
    for kind in [Integer, Uuid, Json, Bytes, DateTime] {
        result.push(
            Scalar::new(scalar::name(kind))
                .validator(move |value| scalar::decode(kind, value, limits).is_ok())
                .into(),
        );
    }
    result.push(
        Enum::new("RuntimeOrderDirection")
            .item(EnumItem::new("ASC"))
            .item(EnumItem::new("DESC"))
            .into(),
    );
    result.push(
        Enum::new("RuntimeNullPlacement")
            .item(EnumItem::new("FIRST"))
            .item(EnumItem::new("LAST"))
            .into(),
    );
    result.push(
        Object::new("RuntimePageInfo")
            .field(output("hasNextPage", TypeRef::named_nn("Boolean")))
            .field(output("hasPreviousPage", TypeRef::named_nn("Boolean")))
            .field(output("startCursor", TypeRef::named("String")))
            .field(output("endCursor", TypeRef::named("String")))
            .into(),
    );
    for kind in [Boolean, Integer, Float, String, Uuid, Json, Bytes, DateTime] {
        let mut filter = InputObject::new(filter_name(kind))
            .field(InputValue::new("isNull", TypeRef::named("Boolean")));
        if kind != Json {
            for op in ["eq", "ne"] {
                filter = filter.field(InputValue::new(op, TypeRef::named(scalar::name(kind))));
            }
            for op in ["in", "notIn"] {
                filter = filter.field(InputValue::new(
                    op,
                    TypeRef::named_nn_list(scalar::name(kind)),
                ));
            }
        }
        if matches!(kind, Integer | Float | String | DateTime) {
            for op in ["lt", "lte", "gt", "gte"] {
                filter = filter.field(InputValue::new(op, TypeRef::named(scalar::name(kind))));
            }
        }
        if kind == String {
            for op in ["contains", "startsWith", "endsWith"] {
                filter = filter.field(InputValue::new(op, TypeRef::named("String")));
            }
        }
        result.push(filter.into());
    }
    result
}
pub(super) fn filter_name(kind: RuntimeValueKind) -> String {
    let name = match kind {
        RuntimeValueKind::Boolean => "RuntimeBoolean",
        RuntimeValueKind::Float => "RuntimeFloat",
        RuntimeValueKind::String => "RuntimeString",
        _ => scalar::name(kind),
    };
    format!("{name}Filter")
}
pub(super) fn types(module: &RuntimeGraphqlModule) -> Vec<Type> {
    let mut result = Vec::new();
    for c in &module.schema.schema().collections {
        let name = module.object_name(c);
        let mut object = Object::new(&name);
        let mut filter = InputObject::new(format!("{name}WhereInput"))
            .field(InputValue::new(
                "and",
                TypeRef::named_nn_list(format!("{name}WhereInput")),
            ))
            .field(InputValue::new(
                "or",
                TypeRef::named_nn_list(format!("{name}WhereInput")),
            ))
            .field(InputValue::new(
                "not",
                TypeRef::named(format!("{name}WhereInput")),
            ));
        let mut order_enum = Enum::new(format!("{name}OrderField"));
        for f in &c.fields {
            let ty = if f.nullable {
                TypeRef::named(scalar::name(f.value_kind))
            } else {
                TypeRef::named_nn(scalar::name(f.value_kind))
            };
            object = object.field(output(&f.api_name, ty));
            if f.filterable {
                filter = filter.field(InputValue::new(
                    &f.api_name,
                    TypeRef::named(filter_name(f.value_kind)),
                ));
            }
            if f.sortable {
                order_enum = order_enum.item(EnumItem::new(&f.api_name));
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
                object = object.field(field);
            }
        }
        result.extend([
            object.into(),
            filter.into(),
            order_enum.into(),
            InputObject::new(format!("{name}OrderInput"))
                .field(InputValue::new(
                    "field",
                    TypeRef::named_nn(format!("{name}OrderField")),
                ))
                .field(InputValue::new(
                    "direction",
                    TypeRef::named("RuntimeOrderDirection"),
                ))
                .field(InputValue::new(
                    "nulls",
                    TypeRef::named("RuntimeNullPlacement"),
                ))
                .into(),
            Object::new(format!("{name}Edge"))
                .field(output("cursor", TypeRef::named_nn("String")))
                .field(output("node", TypeRef::named_nn(&name)))
                .into(),
            Object::new(format!("{name}Connection"))
                .field(output(
                    "edges",
                    TypeRef::named_nn_list_nn(format!("{name}Edge")),
                ))
                .field(output("pageInfo", TypeRef::named_nn("RuntimePageInfo")))
                .field(output("totalCount", TypeRef::named_nn("RuntimeInt64")))
                .into(),
        ]);
    }
    result
}
