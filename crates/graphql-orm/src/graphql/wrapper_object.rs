//! Shared output-object glue for the generated wrapper types.
//!
//! The entity derives emit seven plain wrapper objects per entity (edges,
//! connections, mutation results, change events, bulk results and authorized
//! scan pages). They used to derive `async_graphql::SimpleObject`, whose
//! expansion type-checks one nested `async` resolver arm per field in the host
//! crate. The derives now emit thin `OutputType`/`ContainerType`/`ObjectType`
//! impls that pass a static field table to the helpers below, so that glue is
//! type-checked once, here.
//!
//! The helpers reproduce what the `SimpleObject` derive emits for a struct
//! with only `name`, `rename_fields`, field `name` and doc comments: the same
//! registry entry and the same field resolution. These types carry no guards,
//! visibility, federation or complexity attributes, and no authorization runs
//! in them; their fields are already-authorized values.
//!
//! Reached by generated code through this absolute path only.

use async_graphql::parser::types::Field;
use async_graphql::registry::{Deprecation, MetaField, MetaType, MetaTypeId, Registry};
use async_graphql::{
    CacheControl, Context, ContextSelectionSet, OutputType, Positioned, ServerResult, Value,
};
use futures::future::BoxFuture;

/// One output field of a generated wrapper object.
pub struct WrapperField {
    /// GraphQL field name after the type's rename rule.
    pub name: &'static str,
    /// Field description from the field's doc comment, as the derive trims it.
    pub description: Option<&'static str>,
    /// `<FieldType as OutputType>::create_type_info`.
    pub type_info: fn(&mut Registry) -> String,
}

/// Register a generated wrapper object as `SimpleObject` would.
///
/// `T` is the wrapper type itself; its Rust type name is recorded exactly as
/// the derive records `Self`.
pub fn create_type_info<T: OutputType + ?Sized>(
    registry: &mut Registry,
    name: &'static str,
    description: Option<&'static str>,
    fields: &[WrapperField],
) -> String {
    registry.create_output_type::<T, _>(MetaTypeId::Object, |registry| MetaType::Object {
        name: name.to_owned(),
        description: description.map(ToString::to_string),
        fields: {
            let mut map = async_graphql::indexmap::IndexMap::new();
            for field in fields {
                map.insert(
                    field.name.to_owned(),
                    MetaField {
                        name: field.name.to_owned(),
                        description: field.description.map(ToString::to_string),
                        args: Default::default(),
                        ty: (field.type_info)(registry),
                        deprecation: Deprecation::NoDeprecated,
                        cache_control: CacheControl {
                            public: true,
                            max_age: 0,
                        },
                        external: false,
                        provides: None,
                        requires: None,
                        shareable: false,
                        inaccessible: false,
                        tags: Vec::new(),
                        override_from: None,
                        visible: None,
                        compute_complexity: None,
                        directive_invocations: Vec::new(),
                        requires_scopes: Vec::new(),
                    },
                );
            }
            map
        },
        cache_control: CacheControl {
            public: true,
            max_age: 0,
        },
        extends: false,
        shareable: false,
        resolvable: true,
        inaccessible: false,
        interface_object: false,
        tags: Vec::new(),
        keys: None,
        visible: None,
        is_subscription: false,
        rust_typename: Some(std::any::type_name::<T>()),
        directive_invocations: Vec::new(),
        requires_scopes: Vec::new(),
    })
}

/// A wrapper field value resolved through its own `OutputType`.
///
/// Object-safe, so generated code hands field references to the shared
/// resolver without a per-field `async` body.
pub trait WrapperFieldValue: Send + Sync {
    /// `OutputType::resolve` of the value.
    fn resolve_value<'a>(
        &'a self,
        ctx: &'a ContextSelectionSet<'a>,
        field: &'a Positioned<Field>,
    ) -> BoxFuture<'a, ServerResult<Value>>;
}

impl<T: OutputType> WrapperFieldValue for T {
    fn resolve_value<'a>(
        &'a self,
        ctx: &'a ContextSelectionSet<'a>,
        field: &'a Positioned<Field>,
    ) -> BoxFuture<'a, ServerResult<Value>> {
        Box::pin(T::resolve(self, ctx, field))
    }
}

/// The value of the first field whose GraphQL name is the selected one.
///
/// Fields are listed in declaration order, which is the order the derive
/// compares them in.
pub fn select<'a>(
    ctx: &Context<'_>,
    fields: &[(&'static str, &'a dyn WrapperFieldValue)],
) -> Option<&'a dyn WrapperFieldValue> {
    let selected = ctx.item.node.name.node.as_str();
    fields
        .iter()
        .find(|(name, _)| *name == selected)
        .map(|(_, value)| *value)
}

/// Resolve the selected field value, or `None` when the field is not one of
/// the wrapper's own, as `ContainerType::resolve_field` of the derive does.
pub async fn resolve_field(
    ctx: &Context<'_>,
    value: Option<&dyn WrapperFieldValue>,
) -> ServerResult<Option<Value>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let ctx_obj = ctx.with_selection_set(&ctx.item.node.selection_set);
    value.resolve_value(&ctx_obj, ctx.item).await.map(Some)
}
