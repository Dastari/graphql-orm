use super::{DefaultBackend, Entity, EntityAccessKind, EntityAccessSurface, OrmBackend};
use crate::db::Database;
use crate::graphql::errors::OrmPublicError;
use std::marker::PhantomData;

/// Current ORM authorization for a scalar getter owned by `SimpleObject`.
///
/// Prefer [`crate::graphql_policy_object`] to install this guard on the complete
/// object while preserving existing guards and generated relations. The guard
/// checks the current entity, row and selected field decisions on every read;
/// it grants no principal authority or write capability.
pub struct EntityFieldPolicyGuard<'a, T, B: OrmBackend = DefaultBackend> {
    record: &'a T,
    backend: PhantomData<fn() -> B>,
}

impl<'a, T, B: OrmBackend> EntityFieldPolicyGuard<'a, T, B> {
    /// Binds a scalar read to its concrete record and database backend.
    pub fn new(record: &'a T) -> Self {
        Self {
            record,
            backend: PhantomData,
        }
    }
}

impl<T, B> crate::async_graphql::Guard for EntityFieldPolicyGuard<'_, T, B>
where
    T: Entity + Send + Sync + 'static,
    B: OrmBackend + 'static,
{
    async fn check(
        &self,
        context: &crate::async_graphql::Context<'_>,
    ) -> crate::async_graphql::Result<()> {
        let denied = || OrmPublicError::forbidden().into_graphql_error();
        let field = T::repository_field_policies()
            .iter()
            .find(|field| field.api_name == context.item.node.name.node.as_str())
            .ok_or_else(denied)?;
        let subscription = context.query_env.operation.node.ty
            == crate::async_graphql::parser::types::OperationType::Subscription;
        if subscription
            && !T::subscription_field_policy(field.api_name)
                .is_some_and(|(_, available, _)| available)
        {
            return Err(denied());
        }
        let surface = if subscription {
            EntityAccessSurface::GraphqlSubscription
        } else {
            EntityAccessSurface::GraphqlQuery
        };
        let database = context.data::<Database<B>>()?;
        database
            .ensure_entity_access(
                Some(context),
                T::entity_name(),
                T::metadata().read_policy,
                EntityAccessKind::Read,
                surface,
            )
            .await
            .map_err(crate::graphql::errors::graphql_error_from_callback)?;
        if !database
            .can_read_row(
                Some(context),
                T::entity_name(),
                T::metadata().read_policy,
                surface,
                self.record,
            )
            .await
            .map_err(crate::graphql::errors::graphql_error_from_callback)?
        {
            return Err(denied());
        }
        database
            .ensure_readable_field(
                context,
                T::entity_name(),
                field.api_name,
                field.read_policy,
                Some(self.record),
            )
            .await
    }
}

/// One handwritten resolver field hosted by a generated entity object.
///
/// `#[graphql_orm::graphql_object_fields]` emits one per handwritten field of
/// an entity that declares `#[graphql_orm(object_fields)]`. Such a field is
/// computed: it has no column, takes no part in generated filters, ordering,
/// grouping, aggregates or writes, and is read only through its resolver.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ComputedObjectFieldDef {
    /// Rust name of the handwritten resolver method.
    pub rust_name: &'static str,
    /// Public GraphQL field name.
    pub api_name: &'static str,
    /// Field description, as served in the schema.
    pub description: &'static str,
    /// Declared field read policy key, if any.
    pub read_policy: Option<&'static str>,
    /// Whether generated `Changed` subscriptions may select the field.
    pub subscribe: bool,
    /// Qualified GraphQL type of the field, such as `Outcome` or `[Int!]!`.
    pub output_type: fn() -> String,
}

impl ComputedObjectFieldDef {
    /// Used by macro-generated code.
    #[doc(hidden)]
    pub const fn new(
        rust_name: &'static str,
        api_name: &'static str,
        description: &'static str,
        read_policy: Option<&'static str>,
        subscribe: bool,
        output_type: fn() -> String,
    ) -> Self {
        Self {
            rust_name,
            api_name,
            description,
            read_policy,
            subscribe,
            output_type,
        }
    }
}

/// Internal proof that an entity opted into `#[graphql_orm(object_fields)]`.
///
/// This is public only because macro expansion occurs in the consuming crate.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`#[graphql_object_fields]` requires `#[graphql_orm(object_fields)]` on `{Self}`",
    label = "this entity's generated object does not accept handwritten fields",
    note = "add `#[graphql_orm(object_fields)]` to the `GraphQLEntity` struct; an entity that owns its object through `SimpleObject` and `#[graphql(complex)]` uses `#[graphql_orm::graphql_complex_object]` instead"
)]
pub trait GeneratedObjectFieldsHost {}

/// Internal bridge from `#[graphql_object_fields]` to the entity derive: the
/// handwritten fields of a generated object.
///
/// This is public only because macro expansion occurs in the consuming crate.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` declares `#[graphql_orm(object_fields)]`, but no `#[graphql_object_fields]` impl composes its GraphQL object",
    label = "this entity has no GraphQL object yet",
    note = "add `#[graphql_orm::graphql_object_fields] impl {Self} {{ ... }}` in the entity's module, or remove `object_fields`"
)]
pub trait HandwrittenObjectFields {
    /// The handwritten fields, in declaration order.
    const FIELDS: &'static [ComputedObjectFieldDef];
}

/// Authorize one handwritten field of a generated object before it resolves.
///
/// These are the checks a generated column getter runs, in the same order:
/// availability in subscriptions, then the field read decision for `record`
/// with the field's declared policy key or none. A policy callback failure
/// goes through the callback sanitizer, as it does for a generated getter.
/// Any denial or error is returned; nothing is resolved first.
///
/// Like a generated getter, this asks no entity or row decision. Generated
/// read paths make both before the object exists. A host resolver that
/// constructs or loads an entity object itself authorizes it itself.
#[doc(hidden)]
pub async fn authorize_computed_object_field<T, B>(
    context: &crate::async_graphql::Context<'_>,
    record: &T,
    field: &ComputedObjectFieldDef,
) -> crate::async_graphql::Result<()>
where
    T: Entity + Send + Sync + 'static,
    B: OrmBackend + 'static,
{
    let database = context.data_unchecked::<Database<B>>();
    if context.query_env.operation.node.ty
        == crate::async_graphql::parser::types::OperationType::Subscription
        && !field.subscribe
    {
        return Err(crate::async_graphql::Error::new(format!(
            "Field {}.{} is not available in subscriptions",
            T::entity_name(),
            field.api_name,
        )));
    }
    database
        .ensure_readable_field(
            context,
            T::entity_name(),
            field.api_name,
            field.read_policy,
            Some(record as &(dyn std::any::Any + Send + Sync)),
        )
        .await
        .map_err(crate::graphql::errors::graphql_error_from_callback)
}

/// Report the refusal of a handwritten field declared `Option<T>`, so that the
/// resolver can return `None`: the response then carries the field's key with
/// an explicit `null`, and this error with the field's path. An error returned
/// from the resolver instead would leave the key out of the response.
#[doc(hidden)]
pub fn report_computed_object_field_refusal(
    context: &crate::async_graphql::Context<'_>,
    refusal: crate::async_graphql::Error,
) {
    context.add_error(context.set_error_path(refusal.into_server_error(context.item.pos)));
}

/// Output types a handwritten object field may have when it declares
/// `subscribe`: a scalar, an enum, or an `Option` or list of those.
///
/// A generated `Changed` event is released only after every selected field is
/// proved. That proof covers a leaf value; it cannot cover whatever an object
/// would resolve below it. `Kind` only keeps the scalar and enum
/// implementations apart.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "a handwritten object field that declares `subscribe` must return a scalar, an enum, or an `Option` or list of those; `{Self}` is none of them",
    label = "not available in generated `Changed` subscriptions",
    note = "remove `#[graphql_orm(subscribe)]`: clients then refetch the field with an authorized query after a change event"
)]
pub trait SubscribableFieldOutput<Kind> {}

#[doc(hidden)]
pub struct ScalarOutput;
#[doc(hidden)]
pub struct EnumOutput;
#[doc(hidden)]
pub struct WrappedOutput<Kind>(PhantomData<Kind>);

impl<T: crate::async_graphql::ScalarType> SubscribableFieldOutput<ScalarOutput> for T {}
impl<T: crate::async_graphql::EnumType> SubscribableFieldOutput<EnumOutput> for T {}
impl SubscribableFieldOutput<ScalarOutput> for str {}
impl<Kind, T: SubscribableFieldOutput<Kind>> SubscribableFieldOutput<WrappedOutput<Kind>>
    for Option<T>
{
}
impl<Kind, T: SubscribableFieldOutput<Kind>> SubscribableFieldOutput<WrappedOutput<Kind>>
    for Vec<T>
{
}
impl<Kind, T: SubscribableFieldOutput<Kind>> SubscribableFieldOutput<WrappedOutput<Kind>>
    for [T]
{
}
impl<Kind, T: SubscribableFieldOutput<Kind> + ?Sized> SubscribableFieldOutput<WrappedOutput<Kind>>
    for &T
{
}

/// Semantic descriptors of an entity's handwritten object fields, appended to
/// the generated ones. A computed field is selectable and nothing else: it has
/// no filter, ordering, grouping or aggregate, and it is never exported to an
/// external provider.
#[doc(hidden)]
pub fn __gom_with_computed_fields<T: Entity>(
    mut fields: Vec<super::GraphqlSemanticFieldMetadata>,
    classification: super::GraphqlSemanticClassification,
) -> Vec<super::GraphqlSemanticFieldMetadata> {
    fields.extend(T::computed_object_fields().iter().map(|field| {
        super::GraphqlSemanticFieldMetadata {
            field_name: field.api_name.to_owned(),
            description: field.description.to_owned(),
            type_ref: super::parse_graphql_type(&(field.output_type)())
                .expect("handwritten object field type must be a valid GraphQL type"),
            selectable: true,
            filter_operators: Vec::new(),
            filter_input: None,
            sortable: false,
            groupable: false,
            aggregate_operators: Vec::new(),
            aggregate_value_kind: None,
            relationship: None,
            classification,
            export: super::GraphqlSemanticExport::NeverExport,
            has_field_policy: field.read_policy.is_some(),
        }
    }));
    fields
}
