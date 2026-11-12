//! Shared authorization for the fields a read argument references.
//!
//! A `where` predicate, an `orderBy` term and a full-text search source each
//! reveal information about a field even when the response never selects it.
//! Every generated surface therefore authorizes those fields with the same
//! field read policy as a selected field, before any SQL runs. Relation
//! traversal additionally enforces the target entity's own auth mode and
//! operation scopes.

use super::query::{DatabaseFilter, DatabaseOrderBy, Entity};
use super::search::SearchableEntity;
use super::{EntityAccessKind, EntityAccessSurface, OrmBackend};
use crate::db::Database;
use crate::graphql::errors::{
    OrmErrorCode, OrmPublicError, graphql_error_from_callback, sqlx_error_from_graphql,
};

fn forbidden() -> async_graphql::Error {
    OrmPublicError::forbidden().into_graphql_error()
}

fn misconfigured(detail: &'static str) -> OrmPublicError {
    OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured).with_internal(detail)
}

/// Authorize the fields of `T` that a read argument references.
///
/// A filter predicate, an order term or a search source reveals a field's
/// value even when the response never selects the field. Call this before
/// running any SQL built from such an argument, with the public field
/// identities the argument names: [`DatabaseFilter::referenced_fields`] for a
/// generated filter and [`DatabaseOrderBy::referenced_fields`] for a generated
/// order input. Generated resolvers call it for every `where`, `orderBy` and
/// bulk-write filter.
///
/// Each field is checked with [`Database::can_read_field`], with the field's
/// declared read policy key and no record (`record` is `None`), exactly as a
/// selected field is checked before rows are read. Duplicate names are
/// checked once. An empty list authorizes nothing and succeeds.
///
/// # Errors
///
/// Returns a `FORBIDDEN` error when a field is denied or is not declared in
/// `T`'s field policy metadata ([`Entity::repository_field_policies`]). A
/// field policy callback failure is passed through the callback sanitizer,
/// so its detail never reaches the client; an authored refusal keeps its
/// public code. Every failure refuses the whole argument.
///
/// # Example
///
/// ```ignore
/// use graphql_orm::graphql::orm::{authorize_read_fields, DatabaseFilter, DatabaseOrderBy};
///
/// // In a host resolver that accepts a generated `TicketWhereInput` and
/// // `TicketOrderByInput`, before building the query:
/// let mut fields = filter.referenced_fields();
/// fields.extend(order.referenced_fields());
/// authorize_read_fields::<Ticket, _>(db, ctx, fields).await?;
/// ```
pub async fn authorize_read_fields<T, B>(
    db: &Database<B>,
    ctx: &async_graphql::Context<'_>,
    fields: impl IntoIterator<Item = &'static str>,
) -> async_graphql::Result<()>
where
    T: Entity,
    B: OrmBackend,
{
    let mut fields = fields.into_iter().collect::<Vec<_>>();
    fields.sort_unstable();
    fields.dedup();
    for field_name in fields {
        let field = T::repository_field_policies()
            .iter()
            .find(|field| field.api_name == field_name)
            .ok_or_else(forbidden)?;
        let allowed = db
            .can_read_field(
                ctx,
                T::entity_name(),
                field.api_name,
                field.read_policy,
                None,
            )
            .await
            .map_err(graphql_error_from_callback)?;
        if !allowed {
            return Err(forbidden());
        }
    }
    Ok(())
}

/// Authorize every field referenced by a generated filter and order terms.
///
/// Callers prepare related-row filters and relation count terms first; this
/// helper covers the scalar fields those inputs name.
#[doc(hidden)]
pub async fn authorize_read_arguments<T, B, F, O>(
    db: &Database<B>,
    ctx: &async_graphql::Context<'_>,
    filter: Option<&F>,
    orders: &[O],
) -> async_graphql::Result<()>
where
    T: Entity,
    B: OrmBackend,
    F: DatabaseFilter,
    O: DatabaseOrderBy,
{
    let mut fields = Vec::new();
    if let Some(filter) = filter {
        fields.extend(filter.referenced_fields());
    }
    for order in orders {
        fields.extend(order.referenced_fields());
    }
    authorize_read_fields::<T, B>(db, ctx, fields).await
}

/// One authorization requirement contributed by a full-text search source.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchSourcePolicy {
    /// Read access to an entity whose rows contribute search text.
    Entity {
        entity_name: &'static str,
        read_policy: Option<&'static str>,
    },
    /// Read access to one field under one policy key.
    Field {
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
    },
    /// A source whose policy metadata could not be resolved.
    Unresolved,
}

/// How a search request is authorized.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum SearchAccess<'a, 'ctx> {
    /// A generated GraphQL search root.
    Graphql(&'a async_graphql::Context<'ctx>),
    /// A repository search without a GraphQL request.
    Repository,
}

/// Collect the authorization requirements of every search source of `T`.
///
/// Local fields and JSON paths require the field's read policy and, when
/// declared, its `searchable(policy)`/`search_json(policy)` key. Relation
/// sources require what the entity's generated metadata reports for them.
#[doc(hidden)]
pub fn search_source_policies<T>() -> Vec<SearchSourcePolicy>
where
    T: Entity + SearchableEntity,
{
    let Some(index) = T::search_index() else {
        return Vec::new();
    };
    let entity_name = T::entity_name();
    let mut sources = Vec::new();
    let local = index
        .fields
        .iter()
        .map(|field| (field.field_name, field.policy))
        .chain(
            index
                .json_paths
                .iter()
                .map(|path| (path.field_name, path.policy)),
        );
    for (rust_name, search_policy) in local {
        let Some(field) = T::repository_field_policies()
            .iter()
            .find(|field| field.rust_name == rust_name)
        else {
            sources.push(SearchSourcePolicy::Unresolved);
            continue;
        };
        sources.push(SearchSourcePolicy::Field {
            entity_name,
            field_name: field.api_name,
            policy_key: field.read_policy,
        });
        if search_policy.is_some() {
            sources.push(SearchSourcePolicy::Field {
                entity_name,
                field_name: field.api_name,
                policy_key: search_policy,
            });
        }
    }
    if !index.relations.is_empty() {
        match T::search_relation_source_policies() {
            Some(relations) => sources.extend(relations),
            None => sources.push(SearchSourcePolicy::Unresolved),
        }
    }
    let mut unique = Vec::with_capacity(sources.len());
    for source in sources {
        if !unique.contains(&source) {
            unique.push(source);
        }
    }
    unique
}

/// Authorize every full-text search source of `T` before any search SQL.
///
/// A caller denied any source cannot search the entity at all: the search
/// document combines every source, so a partial search would still match
/// text from the denied one.
#[doc(hidden)]
pub async fn authorize_search_sources<T, B>(
    db: &Database<B>,
    access: SearchAccess<'_, '_>,
) -> async_graphql::Result<()>
where
    T: Entity + SearchableEntity,
    B: OrmBackend,
{
    // Relation text is a traversal into the target: its auth mode and
    // operation scopes apply as they do to the relation field itself.
    if let SearchAccess::Graphql(ctx) = access {
        T::authorize_search_relation_targets(ctx)?;
    }
    for source in search_source_policies::<T>() {
        let allowed = match (source, access) {
            (SearchSourcePolicy::Unresolved, _) => {
                return Err(
                    misconfigured("search source policy metadata could not be resolved")
                        .into_graphql_error(),
                );
            }
            (
                SearchSourcePolicy::Entity {
                    entity_name,
                    read_policy,
                },
                SearchAccess::Graphql(ctx),
            ) => {
                db.ensure_entity_access(
                    Some(ctx),
                    entity_name,
                    read_policy,
                    EntityAccessKind::Read,
                    EntityAccessSurface::GraphqlQuery,
                )
                .await
                .map_err(graphql_error_from_callback)?;
                true
            }
            (
                SearchSourcePolicy::Entity {
                    entity_name,
                    read_policy,
                },
                SearchAccess::Repository,
            ) => {
                db.ensure_entity_access(
                    None,
                    entity_name,
                    read_policy,
                    EntityAccessKind::Read,
                    EntityAccessSurface::Repository,
                )
                .await
                .map_err(graphql_error_from_callback)?;
                true
            }
            (
                SearchSourcePolicy::Field {
                    entity_name,
                    field_name,
                    policy_key,
                },
                SearchAccess::Graphql(ctx),
            ) => db
                .can_read_field(ctx, entity_name, field_name, policy_key, None)
                .await
                .map_err(graphql_error_from_callback)?,
            (
                SearchSourcePolicy::Field {
                    entity_name,
                    field_name,
                    policy_key,
                },
                SearchAccess::Repository,
            ) => {
                db.ensure_repository_readable_field(
                    None,
                    entity_name,
                    field_name,
                    policy_key,
                    None,
                )
                .await
                .map_err(OrmPublicError::into_graphql_error)?;
                true
            }
        };
        if !allowed {
            return Err(forbidden());
        }
    }
    Ok(())
}

/// Repository form of [`authorize_search_sources`].
pub(crate) async fn authorize_repository_search_sources<T, B>(db: &Database<B>) -> crate::Result<()>
where
    T: Entity + SearchableEntity,
    B: OrmBackend,
{
    authorize_search_sources::<T, B>(db, SearchAccess::Repository)
        .await
        .map_err(sqlx_error_from_graphql)
}

/// Request authorization required to traverse a relation into this entity.
///
/// `GraphQLEntity` and `RepositoryEntity` implement this from the entity's
/// `auth` mode and its `operation_authorization` declarations: a to-many traversal requires the
/// `list` category and a to-one traversal the `single_read` category. A
/// category declared with scope templates cannot be bound during traversal
/// and refuses it.
#[doc(hidden)]
pub trait RelationTraversalAuthorization {
    fn authorize_relation_traversal(
        ctx: &async_graphql::Context<'_>,
        multiple: bool,
    ) -> async_graphql::Result<()>;
}

/// Enforce the target entity's auth mode and operation scopes for one
/// relation traversal. Failures are sanitized and fail closed.
#[doc(hidden)]
pub fn authorize_relation_target<T>(
    ctx: &async_graphql::Context<'_>,
    multiple: bool,
) -> async_graphql::Result<()>
where
    T: RelationTraversalAuthorization,
{
    T::authorize_relation_traversal(ctx, multiple)
}
