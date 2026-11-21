#[doc = r" Scalar-only runtime keyset order. Each list entry must select"]
#[doc = r" exactly one field; NULL values sort last in either direction."]
#[derive(:: graphql_orm :: async_graphql :: InputObject, Default, Clone, Debug)]
pub struct RecordKeysetOrderByInput {
    #[graphql(name = "id")]
    pub id: ::std::option::Option<::graphql_orm::graphql::orm::OrderDirection>,
}
impl RecordKeysetOrderByInput {
    #[doc = r" Build an ordered keyset sort from typed `(field, direction)`"]
    #[doc = r" pairs: the first pair is the primary sort key, and the cursor's"]
    #[doc = r" key order follows the list. The complete primary key is still"]
    #[doc = r" appended as the tiebreak. A field that cannot be a keyset key"]
    #[doc = r" is refused with `INVALID_INPUT`."]
    #[allow(unreachable_patterns)]
    pub fn ordered(
        fields: impl ::std::iter::IntoIterator<
            Item = (
                RecordOrderField,
                ::graphql_orm::graphql::orm::OrderDirection,
            ),
        >,
    ) -> ::std::result::Result<::std::vec::Vec<Self>, ::graphql_orm::graphql::errors::OrmPublicError>
    {
        fields
            .into_iter()
            .map(|(field, direction)| {
                ::std::result::Result::Ok(match field {
                    RecordOrderField::Id => {
                        let mut order = Self::default();
                        order.id = ::std::option::Option::Some(direction);
                        order
                    }
                    _ => {
                        return ::std::result::Result::Err(
                            ::graphql_orm::graphql::errors::OrmPublicError::with_message(
                                ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                                "this field cannot be a keyset sort key",
                            ),
                        );
                    }
                })
            })
            .collect()
    }
    fn __gom_columns(
        items: ::std::vec::Vec<Self>,
    ) -> ::graphql_orm::async_graphql::Result<
        ::std::vec::Vec<::graphql_orm::graphql::orm::KeysetOrderColumn>,
    > {
        let mut columns = ::std::vec::Vec::new();
        for item in items {
            let mut selected = 0;
            if let ::std::option::Option::Some(direction) = item.id {
                selected += 1;
                columns.push(match direction {
                    ::graphql_orm::graphql::orm::OrderDirection::Asc => {
                        ::graphql_orm::graphql::orm::KeysetOrderColumn::asc("id")
                    }
                    ::graphql_orm::graphql::orm::OrderDirection::Desc => {
                        ::graphql_orm::graphql::orm::KeysetOrderColumn::desc("id")
                    }
                });
            }
            if selected != 1 {
                return ::std::result::Result::Err(
                    ::graphql_orm::graphql::errors::OrmPublicError::new(
                        ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                    )
                    .into_graphql_error(),
                );
            }
        }
        ::std::result::Result::Ok(columns)
    }
}
#[doc = r" Generated queries for this entity"]
#[derive(Default)]
pub struct RecordQueries;
impl RecordQueries {
    #[doc = "Aggregate Record records"]
    async fn aggregate(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        where_input: ::std::option::Option<RecordWhereInput>,
        group_by: ::std::vec::Vec<RecordAggregateField>,
        metrics: ::std::vec::Vec<RecordAggregateMetricInput>,
        group_limit: i32,
    ) -> ::graphql_orm::async_graphql::Result<::std::vec::Vec<RecordAggregateResultRow>> {
        ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
        let _auth_subject =
            ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, ::std::option::Option::None)?;
        ::graphql_orm::graphql::orm::ensure_rls_request_context::<
            Record,
            ::graphql_orm::SqliteBackend,
        >(
            ctx,
            <Record as ::graphql_orm::graphql::orm::Entity>::entity_name(),
        )?;
        ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
            ctx,
            ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
        )?;
        let db = ::graphql_orm::db::request_database::<::graphql_orm::SqliteBackend>(ctx)?;
        let auth_context = ctx
            .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
            .cloned();
        let mut where_input = where_input;
        let mut __gom_statement_budget = ::graphql_orm::graphql::orm::RelationFilterBudget::new();
        if let ::std::option::Option::Some(filter) = where_input.as_mut() {
            filter
                .__gom_prepare_read(ctx, 0, &mut __gom_statement_budget)
                .await?;
        }
        if let ::std::option::Option::Some(filter) = where_input.as_ref() {
            ::graphql_orm::graphql::orm::authorize_read_fields::<
                Record,
                ::graphql_orm::SqliteBackend,
            >(
                db,
                ctx,
                ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter),
            )
            .await?;
        }
        let mut query = Record::aggregate(db).with_auth(auth_context.as_ref());
        if let ::std::option::Option::Some(filter) = where_input {
            query = query.filter(filter);
        }
        for field in group_by {
            query = query
                .group_by(field)
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
        }
        for metric in metrics {
            use ::graphql_orm::graphql::orm::GraphqlAggregateInputOperator as Operator;
            query = match (metric.operator, metric.field) {
                (Operator::Count, ::std::option::Option::Some(field)) => query.count(field),
                (Operator::Count, ::std::option::Option::None) => query.count_rows(),
                (Operator::Min, ::std::option::Option::Some(field)) => query.min(field),
                (Operator::Max, ::std::option::Option::Some(field)) => query.max(field),
                (Operator::Sum, ::std::option::Option::Some(field)) => query.sum(field),
                (_, ::std::option::Option::None) => {
                    ::std::result::Result::Err(::graphql_orm::sqlx::Error::Protocol(
                        "aggregate metric field is required for this operator".to_owned(),
                    ))
                }
            }
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
        }
        query = query
            .group_limit(i64::from(group_limit))
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
        let rows = query
            .fetch_with_graphql_context(ctx)
            .await
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
        ::std::result::Result::Ok(
            rows.into_iter()
                .map(|row| {
                    RecordAggregateResultRow::from(
                        ::graphql_orm::graphql::orm::GraphqlAggregateResultRow::from(row),
                    )
                })
                .collect(),
        )
    }
    #[doc = "List Record records"]
    async fn list(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        where_input: ::std::option::Option<RecordWhereInput>,
        order_by: ::std::option::Option<::std::vec::Vec<RecordOrderByInput>>,
        page: ::std::option::Option<::graphql_orm::graphql::orm::PageInput>,
    ) -> ::graphql_orm::async_graphql::Result<RecordConnection> {
        ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
        use ::graphql_orm::graphql::orm::{DatabaseOrderBy, EntityQuery};
        let _auth_subject =
            ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, ::std::option::Option::None)?;
        ::graphql_orm::graphql::orm::ensure_rls_request_context::<
            Record,
            ::graphql_orm::SqliteBackend,
        >(
            ctx,
            <Record as ::graphql_orm::graphql::orm::Entity>::entity_name(),
        )?;
        ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
            ctx,
            ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
        )?;
        let db = ::graphql_orm::db::request_database::<::graphql_orm::SqliteBackend>(ctx)?;
        let auth_context = ctx
            .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
            .cloned();
        db.ensure_entity_access(
            ::std::option::Option::Some(ctx),
            "Record",
            <Record as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
            ::graphql_orm::graphql::orm::EntityAccessKind::Read,
            ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
        )
        .await
        .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
        let mut where_input = where_input;
        let mut __gom_statement_budget = ::graphql_orm::graphql::orm::RelationFilterBudget::new();
        if let ::std::option::Option::Some(filter) = where_input.as_mut() {
            filter
                .__gom_prepare_read(ctx, 0, &mut __gom_statement_budget)
                .await?;
        }
        if let ::std::option::Option::Some(filter) = where_input.as_ref() {
            ::graphql_orm::graphql::orm::authorize_read_fields::<
                Record,
                ::graphql_orm::SqliteBackend,
            >(
                db,
                ctx,
                ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter),
            )
            .await?;
        }
        let mut order_by = order_by;
        if let ::std::option::Option::Some(orders) = order_by.as_mut() {
            if orders.len() > ::graphql_orm::graphql::orm::MAX_ORDER_BY_TERMS {
                return ::std::result::Result::Err(
                    ::graphql_orm::graphql::errors::OrmPublicError::new(
                        ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                    )
                    .into_graphql_error(),
                );
            }
            for order in orders.iter_mut() {
                order
                    .__gom_prepare_read(ctx, &mut __gom_statement_budget)
                    .await?;
            }
            ::graphql_orm::graphql::orm::authorize_read_fields::<
                Record,
                ::graphql_orm::SqliteBackend,
            >(
                db,
                ctx,
                orders
                    .iter()
                    .flat_map(::graphql_orm::graphql::orm::DatabaseOrderBy::referenced_fields),
            )
            .await?;
        }
        let mut query = EntityQuery::<Record, ::graphql_orm::SqliteBackend>::new();
        if let ::std::option::Option::Some(ref filter) = where_input {
            query = query.filter_with_entity_matching(filter);
        }
        if let ::std::option::Option::Some(ref orders) = order_by {
            for order in orders {
                query = query.order_by_with_context(order, ctx)?;
            }
        }
        if query.order_clauses.is_empty() {
            query = query.default_order();
        }
        let visibility = db
            .read_visibility::<Record>(
                ::std::option::Option::Some(ctx),
                ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
            )
            .await
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
        if visibility.requires_residual_checks() {
            let resolved_page = db
                .client_pagination_config()
                .resolve_client_page(page.as_ref(), true)
                .map_err(|error| error.into_graphql_error())?;
            let authorized_page = ::graphql_orm::graphql::loaders::scan_authorized_query(
                db,
                ctx,
                query,
                auth_context.as_ref(),
                &visibility,
                resolved_page,
                ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
            )
            .await?;
            return ::std::result::Result::Ok(RecordConnection::from_generic(
                authorized_page.into_offset_connection(),
            ));
        }
        query = query.with_read_visibility(&visibility)?;
        if page.is_some() {
            let resolved_page = db
                .client_pagination_config()
                .resolve_client_page(page.as_ref(), true)
                .map_err(|error| error.into_graphql_error())?;
            query = query.paginate(&::graphql_orm::graphql::orm::PageInput {
                limit: resolved_page.limit,
                offset: ::std::option::Option::Some(resolved_page.offset),
            });
        }
        let generic_conn = query
            .fetch_connection_selected_with_auth(
                db,
                auth_context.as_ref(),
                ::graphql_orm::graphql::pagination::ConnectionSelection::from_context(ctx),
            )
            .await
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
        ::std::result::Result::Ok(RecordConnection::from_generic(generic_conn))
    }
    #[doc = "Search Record records"]
    async fn search(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        search: ::graphql_orm::graphql::filters::SearchInput,
        where_input: ::std::option::Option<RecordWhereInput>,
        page: ::std::option::Option<::graphql_orm::graphql::orm::PageInput>,
    ) -> ::graphql_orm::async_graphql::Result<RecordSearchConnection> {
        ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
        let _auth_subject =
            ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, ::std::option::Option::None)?;
        ::graphql_orm::graphql::orm::ensure_rls_request_context::<
            Record,
            ::graphql_orm::SqliteBackend,
        >(
            ctx,
            <Record as ::graphql_orm::graphql::orm::Entity>::entity_name(),
        )?;
        ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
            ctx,
            ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
        )?;
        let db = ::graphql_orm::db::request_database::<::graphql_orm::SqliteBackend>(ctx)?;
        let pool = db.pool();
        let auth_context = ctx
            .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
            .cloned();
        db.ensure_entity_access(
            ::std::option::Option::Some(ctx),
            "Record",
            <Record as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
            ::graphql_orm::graphql::orm::EntityAccessKind::Read,
            ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
        )
        .await
        .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
        let mut where_input = where_input;
        let mut __gom_statement_budget = ::graphql_orm::graphql::orm::RelationFilterBudget::new();
        if let ::std::option::Option::Some(filter) = where_input.as_mut() {
            filter
                .__gom_prepare_read(ctx, 0, &mut __gom_statement_budget)
                .await?;
        }
        if let ::std::option::Option::Some(filter) = where_input.as_ref() {
            ::graphql_orm::graphql::orm::authorize_read_fields::<
                Record,
                ::graphql_orm::SqliteBackend,
            >(
                db,
                ctx,
                ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter),
            )
            .await?;
        }
        let mut query =
            Record::search(pool, search).client_pagination_config(db.client_pagination_config());
        if let ::std::option::Option::Some(ref filter) = where_input {
            query = query.filter(filter.clone());
        }
        query = query.default_order();
        let generic_conn = query
            .fetch_authorized_connection(db, ctx, auth_context.as_ref(), page)
            .await?;
        ::std::result::Result::Ok(RecordSearchConnection::from_generic(generic_conn))
    }
    #[doc = "Page through Record records"]
    async fn keyset(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        where_input: ::std::option::Option<RecordWhereInput>,
        order_by: ::std::option::Option<::std::vec::Vec<RecordKeysetOrderByInput>>,
        page: ::graphql_orm::graphql::pagination::KeysetPageInput,
    ) -> ::graphql_orm::async_graphql::Result<RecordConnection> {
        ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
        let _auth_subject =
            ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, ::std::option::Option::None)?;
        ::graphql_orm::graphql::orm::ensure_rls_request_context::<
            Record,
            ::graphql_orm::SqliteBackend,
        >(
            ctx,
            <Record as ::graphql_orm::graphql::orm::Entity>::entity_name(),
        )?;
        ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
            ctx,
            ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
        )?;
        let db = ::graphql_orm::db::request_database::<::graphql_orm::SqliteBackend>(ctx)?;
        let auth = ctx.data_opt::<::graphql_orm::graphql::orm::DbAuthContext>();
        let mut where_input = where_input;
        let mut __gom_statement_budget = ::graphql_orm::graphql::orm::RelationFilterBudget::new();
        if let ::std::option::Option::Some(filter) = where_input.as_mut() {
            filter
                .__gom_prepare_read(ctx, 0, &mut __gom_statement_budget)
                .await?;
        }
        if let ::std::option::Option::Some(filter) = where_input.as_ref() {
            ::graphql_orm::graphql::orm::authorize_read_fields::<
                Record,
                ::graphql_orm::SqliteBackend,
            >(
                db,
                ctx,
                ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter),
            )
            .await?;
        }
        let filter = where_input.unwrap_or_default();
        let order = order_by
            .map(RecordKeysetOrderByInput::__gom_columns)
            .transpose()?;
        let connection = ::graphql_orm::graphql::orm::EntityQuery::<
            Record,
            ::graphql_orm::SqliteBackend,
        >::new()
        .filter_with_entity_matching(&filter)
        .fetch_authorized_keyset_ordered(db, ::std::option::Option::Some(ctx), auth, page, order)
        .await?;
        ::std::result::Result::Ok(RecordConnection::from_generic(connection))
    }
    #[doc = "Bounded forward authorization scan; continuation does not promise another visible row."]
    async fn authorized_scan(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        where_input: ::std::option::Option<RecordWhereInput>,
        page: ::graphql_orm::graphql::orm::AuthorizedScanInput,
    ) -> ::graphql_orm::async_graphql::Result<RecordAuthorizedScanPage> {
        ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
        let _auth_subject =
            ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, ::std::option::Option::None)?;
        ::graphql_orm::graphql::orm::ensure_rls_request_context::<
            Record,
            ::graphql_orm::SqliteBackend,
        >(
            ctx,
            <Record as ::graphql_orm::graphql::orm::Entity>::entity_name(),
        )?;
        ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
            ctx,
            ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
        )?;
        let db = ::graphql_orm::db::request_database::<::graphql_orm::SqliteBackend>(ctx)?;
        let auth = ctx.data_opt::<::graphql_orm::graphql::orm::DbAuthContext>();
        let mut where_input = where_input;
        let mut __gom_statement_budget = ::graphql_orm::graphql::orm::RelationFilterBudget::new();
        if let ::std::option::Option::Some(filter) = where_input.as_mut() {
            filter
                .__gom_prepare_read(ctx, 0, &mut __gom_statement_budget)
                .await?;
        }
        if let ::std::option::Option::Some(filter) = where_input.as_ref() {
            ::graphql_orm::graphql::orm::authorize_read_fields::<
                Record,
                ::graphql_orm::SqliteBackend,
            >(
                db,
                ctx,
                ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter),
            )
            .await?;
        }
        let result =
            ::graphql_orm::graphql::orm::EntityQuery::<Record, ::graphql_orm::SqliteBackend>::new()
                .filter_with_entity_matching(&where_input.unwrap_or_default())
                .fetch_authorized_scan(db, ::std::option::Option::Some(ctx), auth, page)
                .await?;
        ::std::result::Result::Ok(RecordAuthorizedScanPage {
            nodes: result.nodes,
            page_info: result.page_info,
        })
    }
    #[doc = "Read one Record record"]
    async fn get_by_id(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        id: ::std::string::String,
    ) -> ::graphql_orm::async_graphql::Result<::std::option::Option<Record>> {
        use ::graphql_orm::graphql::orm::EntityQuery;
        let _auth_subject =
            ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, ::std::option::Option::None)?;
        ::graphql_orm::graphql::orm::ensure_rls_request_context::<
            Record,
            ::graphql_orm::SqliteBackend,
        >(
            ctx,
            <Record as ::graphql_orm::graphql::orm::Entity>::entity_name(),
        )?;
        let key = id;
        ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
            ctx,
            ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
        )?;
        let db = ::graphql_orm::db::request_database::<::graphql_orm::SqliteBackend>(ctx)?;
        db.ensure_entity_access(
            ::std::option::Option::Some(ctx),
            "Record",
            <Record as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
            ::graphql_orm::graphql::orm::EntityAccessKind::Read,
            ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
        )
        .await
        .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
        ::graphql_orm::graphql::orm::fetch_authorized_point_row(
            db,
            ctx,
            EntityQuery::<Record, ::graphql_orm::SqliteBackend>::new().where_values(
                &Record::__gom_key_where_clause(),
                Record::__gom_key_values(&key),
            ),
        )
        .await
    }
    #[doc = "Resolve one Record from a Federation entity representation."]
    async fn __gom_federation_entity_0(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        __gom_federation_key_0_0: ::std::string::String,
    ) -> ::graphql_orm::async_graphql::Result<::std::option::Option<Record>> {
        let _auth_subject =
            ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, ::std::option::Option::None)?;
        ::graphql_orm::graphql::orm::ensure_rls_request_context::<
            Record,
            ::graphql_orm::SqliteBackend,
        >(
            ctx,
            <Record as ::graphql_orm::graphql::orm::Entity>::entity_name(),
        )?;
        ::graphql_orm::graphql::assurance::enforce_operation_assurance(
            ctx,
            ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
            "record",
        )?;
        ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
            ctx,
            ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
        )?;
        let db = ::graphql_orm::db::request_database::<::graphql_orm::SqliteBackend>(ctx)?;
        let auth_context = ctx
            .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
            .cloned();
        db.ensure_entity_access(
            ::std::option::Option::Some(ctx),
            "Record",
            <Record as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
            ::graphql_orm::graphql::orm::EntityAccessKind::Read,
            ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
        )
        .await
        .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
        let mut __gom_federation_values = ::std::vec::Vec::new();
        let mut __gom_federation_parts = ::std::vec::Vec::new();
        __gom_federation_values.push(::graphql_orm::graphql::orm::SqlValue::String(
            __gom_federation_key_0_0.clone(),
        ));
        __gom_federation_parts.push(__gom_federation_key_0_0.clone());
        let __gom_federation_loader = ctx . data :: < :: graphql_orm :: async_graphql :: dataloader :: DataLoader < :: graphql_orm :: graphql :: loaders :: FederationKeyLoader < Record , :: graphql_orm :: SqliteBackend , > , > , > () . map_err (| _ | :: graphql_orm :: async_graphql :: Error :: new ("graphql-orm Federation loader not registered; build the schema with schema_builder(database)" ,)) ? ;
        let __gom_federation_loaded = __gom_federation_loader
            .load_one(::graphql_orm::graphql::loaders::IsolatedLoad(
                ::graphql_orm::graphql::loaders::FederationEntityKey {
                    key: "Record::@key(id)",
                    columns: ::std::vec!["id"],
                    key_part_kinds: ::std::vec![
                        ::graphql_orm::graphql::loaders::RelationKeyPartKind::String
                    ],
                    values: __gom_federation_values.clone(),
                    representation:
                        ::graphql_orm::graphql::loaders::RelationKey::from_projected_parts(
                            __gom_federation_parts,
                            &[::graphql_orm::graphql::loaders::RelationKeyPartKind::String],
                        ),
                    auth_context: auth_context.clone(),
                },
            ))
            .await;
        let __gom_federation_loaded =
            ::graphql_orm::graphql::loaders::isolated_load_result(__gom_federation_loaded)
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_loader)?;
        let entity = ::graphql_orm::graphql::orm::authorize_loaded_point_row(
            db,
            ctx,
            &["id"],
            __gom_federation_values,
            __gom_federation_loaded.flatten(),
        )
        .await?;
        ::std::result::Result::Ok(entity)
    }
    #[doc = "Resolve one Record from its global NodeId."]
    async fn __gom_federation_node_id(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        __gom_node_id: ::graphql_orm::async_graphql::ID,
    ) -> ::graphql_orm::async_graphql::Result<::std::option::Option<Record>> {
        let _auth_subject =
            ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, ::std::option::Option::None)?;
        ::graphql_orm::graphql::orm::ensure_rls_request_context::<
            Record,
            ::graphql_orm::SqliteBackend,
        >(
            ctx,
            <Record as ::graphql_orm::graphql::orm::Entity>::entity_name(),
        )?;
        let ::std::option::Option::Some(__gom_node_parts) =
            ::graphql_orm::graphql::federation::decode_node_id(&__gom_node_id, "Record", 1usize)
        else {
            return ::std::result::Result::Ok(::std::option::Option::None);
        };
        let mut __gom_node_parts = __gom_node_parts.into_iter();
        let ::std::option::Option::Some(__gom_node_part_0) =
            __gom_node_parts.next().and_then(|part| {
                ::graphql_orm::serde_json::from_value::<::std::string::String>(part).ok()
            })
        else {
            return ::std::result::Result::Ok(::std::option::Option::None);
        };
        if ::graphql_orm::graphql::federation::encode_node_id(
            "Record",
            ::std::vec![::graphql_orm::serde_json::Value::String(
                ::std::clone::Clone::clone(&__gom_node_part_0)
            )],
        )
        .0 != __gom_node_id.0
        {
            return ::std::result::Result::Ok(::std::option::Option::None);
        }
        ::graphql_orm::graphql::assurance::enforce_operation_assurance(
            ctx,
            ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
            "record",
        )?;
        ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
            ctx,
            ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
        )?;
        let db = ::graphql_orm::db::request_database::<::graphql_orm::SqliteBackend>(ctx)?;
        let auth_context = ctx
            .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
            .cloned();
        db.ensure_entity_access(
            ::std::option::Option::Some(ctx),
            "Record",
            <Record as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
            ::graphql_orm::graphql::orm::EntityAccessKind::Read,
            ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
        )
        .await
        .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
        let mut __gom_federation_values = ::std::vec::Vec::new();
        let mut __gom_federation_parts = ::std::vec::Vec::new();
        __gom_federation_values.push(::graphql_orm::graphql::orm::SqlValue::String(
            __gom_node_part_0.clone(),
        ));
        __gom_federation_parts.push(__gom_node_part_0.clone());
        let __gom_federation_loader = ctx . data :: < :: graphql_orm :: async_graphql :: dataloader :: DataLoader < :: graphql_orm :: graphql :: loaders :: FederationKeyLoader < Record , :: graphql_orm :: SqliteBackend , > , > , > () . map_err (| _ | :: graphql_orm :: async_graphql :: Error :: new ("graphql-orm Federation loader not registered; build the schema with schema_builder(database)" ,)) ? ;
        let __gom_federation_loaded = __gom_federation_loader
            .load_one(::graphql_orm::graphql::loaders::IsolatedLoad(
                ::graphql_orm::graphql::loaders::FederationEntityKey {
                    key: "Record::@key(NodeId)",
                    columns: ::std::vec!["id"],
                    key_part_kinds: ::std::vec![
                        ::graphql_orm::graphql::loaders::RelationKeyPartKind::String
                    ],
                    values: __gom_federation_values.clone(),
                    representation:
                        ::graphql_orm::graphql::loaders::RelationKey::from_projected_parts(
                            __gom_federation_parts,
                            &[::graphql_orm::graphql::loaders::RelationKeyPartKind::String],
                        ),
                    auth_context: auth_context.clone(),
                },
            ))
            .await;
        let __gom_federation_loaded =
            ::graphql_orm::graphql::loaders::isolated_load_result(__gom_federation_loaded)
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_loader)?;
        let entity = ::graphql_orm::graphql::orm::authorize_loaded_point_row(
            db,
            ctx,
            &["id"],
            __gom_federation_values,
            __gom_federation_loaded.flatten(),
        )
        .await?;
        ::std::result::Result::Ok(entity)
    }
}
#[doc = r" Generated mutations for this entity"]
#[derive(Default)]
pub struct RecordMutations;
impl RecordMutations {
    #[doc = "Create one Record record"]
    async fn create(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        input: GraphQLCreateRecordInput,
    ) -> ::graphql_orm::async_graphql::Result<RecordResult> {
        let __gom_outcome = ::graphql_orm::graphql::errors::MutationOutcomeTracker::new();
        let __gom_result: ::graphql_orm::async_graphql::Result<RecordResult> = async {
            ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
            use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};
            let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(
                ctx,
                ::std::option::Option::None,
            )?;
            ::graphql_orm::graphql::orm::ensure_rls_request_context::<
                Record,
                ::graphql_orm::SqliteBackend,
            >(
                ctx,
                <Record as ::graphql_orm::graphql::orm::Entity>::entity_name(),
            )?;
            ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                ctx,
                ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation,
            )?;
            let db = ::graphql_orm::db::request_database::<::graphql_orm::SqliteBackend>(ctx)?;
            let pool = db.pool();
            let auth_context = ctx
                .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                .cloned();
            db.ensure_entity_access(
                ::std::option::Option::Some(ctx),
                "Record",
                <Record as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
            )
            .await
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            db.ensure_row_write_provider(
                "Record",
                <Record as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
            )
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            let mut input: CreateRecordInput = input.into();
            #[allow(unused_mut)]
            let mut __gom_client_supplied: ::std::collections::HashSet<&'static str> =
                ::std::collections::HashSet::new();
            db.run_before_create(
                ::std::option::Option::Some(ctx),
                "Record",
                &mut input
                    as &mut (dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
            )
            .await
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            let created_pk = input.id.clone();
            let sql = Record::__gom_rebind_sql("INSERT INTO records (id, title) VALUES (?, ?)", 1);
            let mut bind_values: ::std::vec::Vec<::graphql_orm::graphql::orm::SqlValue> =
                ::std::vec::Vec::new();
            if true {
                db.ensure_writable_field(
                    ctx,
                    "Record",
                    "id",
                    ::std::option::Option::None,
                    ::std::option::Option::None,
                    ::std::option::Option::Some(
                        &input.id
                            as &(dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
                    ),
                )
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            }
            if true {
                db.ensure_writable_field(
                    ctx,
                    "Record",
                    "title",
                    ::std::option::Option::None,
                    ::std::option::Option::None,
                    ::std::option::Option::Some(
                        &input.title
                            as &(dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
                    ),
                )
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            }
            bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(
                (&input.id).to_string(),
            ));
            bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(
                (&input.title).to_string(),
            ));
            let mutation_fields = ["id", "title"];
            let mutation_changes =
                ::graphql_orm::graphql::orm::mutation_changes(&mutation_fields, &bind_values);
            let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<
                ::graphql_orm::SqliteBackend,
            >::begin_with_auth(
                db,
                ::graphql_orm::graphql::orm::TransactionMode::Default,
                auth_context.as_ref(),
            )
            .await
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            hook_ctx
                .run_mutation_hook(
                    ::std::option::Option::Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: "Record",
                        table_name: "records",
                        metadata: <Record as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: created_pk.to_string(),
                        changes: mutation_changes.clone(),
                        before_state: ::std::option::Option::None,
                        after_state: ::std::option::Option::None,
                    },
                )
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            __gom_outcome.write_attempted();
            hook_ctx
                .execute(&sql, &bind_values)
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            let entity = Record::__gom_fetch_created_by_id_on(&mut hook_ctx, &created_pk)
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?
                .ok_or_else(|| {
                    ::graphql_orm::async_graphql::Error::new("Entity not found after creation")
                })?;
            hook_ctx
                .__ensure_written_row(
                    ::std::option::Option::Some(ctx),
                    "Record",
                    <Record as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                    &entity as &(dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
                )
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            let after_state = ::std::option::Option::Some(
                ::graphql_orm::graphql::orm::entity_state(&entity)
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?,
            );
            hook_ctx
                .run_mutation_hook(
                    ::std::option::Option::Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: "Record",
                        table_name: "records",
                        metadata: <Record as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: entity.id.to_string(),
                        changes: mutation_changes.clone(),
                        before_state: ::std::option::Option::None,
                        after_state,
                    },
                )
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            Record::__gom_queue_changed_event(
                &mut hook_ctx,
                ::graphql_orm::graphql::orm::ChangeAction::Created,
                ::std::option::Option::Some(&entity),
            )
            .await
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            hook_ctx
                .commit_and_emit_tracked(::std::option::Option::Some(&__gom_outcome))
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            let result = {
                let entity = db
                    .__release_graphql_mutation_output(
                        ctx,
                        "record",
                        ::std::option::Option::Some(entity),
                        &Record::__gom_key_where_clause(),
                        Record::__gom_subscription_key_values,
                    )
                    .await?;
                RecordResult {
                    success: true,
                    error: ::std::option::Option::None,
                    entity,
                }
            };
            ::std::result::Result::Ok(result)
        }
        .await;
        __gom_outcome.finish::<_, ::graphql_orm::SqliteBackend>(ctx, __gom_result)
    }
    #[doc = "Create or update one Record record"]
    async fn upsert(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        input: GraphQLCreateRecordInput,
    ) -> ::graphql_orm::async_graphql::Result<UpsertRecordResult> {
        let __gom_outcome = ::graphql_orm::graphql::errors::MutationOutcomeTracker::new();
        let __gom_result: ::graphql_orm::async_graphql::Result<UpsertRecordResult> = async {
            ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
            let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(
                ctx,
                ::std::option::Option::None,
            )?;
            ::graphql_orm::graphql::orm::ensure_rls_request_context::<
                Record,
                ::graphql_orm::SqliteBackend,
            >(
                ctx,
                <Record as ::graphql_orm::graphql::orm::Entity>::entity_name(),
            )?;
            ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                ctx,
                ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation,
            )?;
            let db = ::graphql_orm::db::request_database::<::graphql_orm::SqliteBackend>(ctx)?;
            let pool = db.pool();
            let auth_context = ctx
                .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                .cloned();
            db.ensure_entity_access(
                ::std::option::Option::Some(ctx),
                "Record",
                <Record as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
            )
            .await
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            db.ensure_row_write_provider(
                "Record",
                <Record as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
            )
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            let mut input: CreateRecordInput = input.into();
            #[allow(unused_mut)]
            let mut __gom_client_supplied: ::std::collections::HashSet<&'static str> =
                ::std::collections::HashSet::new();
            let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<
                ::graphql_orm::SqliteBackend,
            >::begin_with_auth(
                db,
                ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                auth_context.as_ref(),
            )
            .await
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            {
                let mut write_ctx =
                    ::graphql_orm::graphql::orm::WriteInputContext::graphql_in_transaction(
                        ctx,
                        "Record",
                        &mut hook_ctx,
                    );
                db.run_before_upsert_with_context(
                    &mut write_ctx,
                    &mut input
                        as &mut (dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
                )
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            }
            let current_entity = Record::__gom_fetch_by_upsert_on(&mut hook_ctx, &input)
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            if let ::std::option::Option::Some(current_entity) = current_entity.as_ref() {
                db.ensure_writable_row(
                    ::std::option::Option::Some(ctx),
                    "Record",
                    <Record as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                    current_entity
                        as &(dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
                )
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            }
            #[allow(unused_mut)]
            let mut __gom_denied_upsert_columns: ::std::vec::Vec<&'static str> =
                ::std::vec::Vec::new();
            if true {
                if current_entity.is_none() {
                    db.ensure_writable_field(
                        ctx,
                        "Record",
                        "id",
                        ::std::option::Option::None,
                        ::std::option::Option::None,
                        ::std::option::Option::Some(
                            &input.id
                                as &(
                                     dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync
                                 ),
                        ),
                    )
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                }
            }
            if true {
                if let ::std::option::Option::Some(current_entity) = current_entity.as_ref() {
                    db.ensure_writable_field(
                        ctx,
                        "Record",
                        "title",
                        ::std::option::Option::None,
                        ::std::option::Option::Some(
                            current_entity
                                as &(
                                     dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync
                                 ),
                        ),
                        ::std::option::Option::Some(
                            &input.title
                                as &(
                                     dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync
                                 ),
                        ),
                    )
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                } else {
                    db.ensure_writable_field(
                        ctx,
                        "Record",
                        "title",
                        ::std::option::Option::None,
                        ::std::option::Option::None,
                        ::std::option::Option::Some(
                            &input.title
                                as &(
                                     dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync
                                 ),
                        ),
                    )
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                }
            }
            let created_pk = input.id.clone();
            let action = if current_entity.is_some() {
                ::graphql_orm::graphql::orm::ChangeAction::Updated
            } else {
                ::graphql_orm::graphql::orm::ChangeAction::Created
            };
            let before_state = current_entity
                .as_ref()
                .map(Record::__gom_capture_entity_state)
                .transpose()
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            let event_id = current_entity
                .as_ref()
                .map(|entity| entity.id.to_string())
                .unwrap_or_else(|| created_pk.to_string());
            let lookup_input = input.clone();
            let change_input = input.clone();
            let sql = Record::__gom_rebind_sql(
                &{
                    let __gom_update_candidates: &[(&'static str, &'static str)] =
                        &[("title", "title")];
                    let __gom_update_columns: ::std::vec::Vec<&'static str> =
                        __gom_update_candidates
                            .iter()
                            .filter(|(column, _)| !__gom_denied_upsert_columns.contains(column))
                            .map(|(_, rendered)| *rendered)
                            .collect();
                    ::graphql_orm::graphql::orm::build_upsert_sql_with_managed_version(
                        "records",
                        &["id", "title"],
                        &["?", "?"],
                        &["id"],
                        &__gom_update_columns,
                        false,
                        ::std::option::Option::None,
                    )
                },
                1,
            );
            let mut bind_values: ::std::vec::Vec<::graphql_orm::graphql::orm::SqlValue> =
                ::std::vec::Vec::new();
            bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(
                (&input.id).to_string(),
            ));
            bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(
                (&input.title).to_string(),
            ));
            let mutation_changes = if action == ::graphql_orm::graphql::orm::ChangeAction::Updated {
                let input = change_input;
                let mut bind_values: ::std::vec::Vec<::graphql_orm::graphql::orm::SqlValue> =
                    ::std::vec::Vec::new();
                bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(
                    (&(input.title.clone())).to_string(),
                ));
                let mut changes =
                    ::graphql_orm::graphql::orm::mutation_changes(&["title"], &bind_values);
                changes
                    .retain(|change| !__gom_denied_upsert_columns.contains(&change.field.as_str()));
                changes
            } else {
                ::graphql_orm::graphql::orm::mutation_changes(&["id", "title"], &bind_values)
            };
            hook_ctx
                .run_mutation_hook(
                    ::std::option::Option::Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action,
                        entity_name: "Record",
                        table_name: "records",
                        metadata: <Record as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: event_id,
                        changes: mutation_changes.clone(),
                        before_state: before_state.clone(),
                        after_state: ::std::option::Option::None,
                    },
                )
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            if let ::std::option::Option::Some(current_entity) = current_entity.as_ref() {
                Record::__gom_require_managed_version_headroom(current_entity)
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            }
            __gom_outcome.write_attempted();
            hook_ctx
                .execute(&sql, &bind_values)
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            let entity = Record::__gom_fetch_by_upsert_on(&mut hook_ctx, &lookup_input)
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?
                .ok_or_else(|| {
                    ::graphql_orm::async_graphql::Error::new("Entity not found after upsert")
                })?;
            hook_ctx
                .__ensure_written_row(
                    ::std::option::Option::Some(ctx),
                    "Record",
                    <Record as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                    &entity as &(dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
                )
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            Record::__gom_require_managed_version_advanced(current_entity.as_ref(), &entity)
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            let after_state = ::std::option::Option::Some(
                Record::__gom_capture_entity_state(&entity)
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?,
            );
            hook_ctx
                .run_mutation_hook(
                    ::std::option::Option::Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action,
                        entity_name: "Record",
                        table_name: "records",
                        metadata: <Record as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: entity.id.to_string(),
                        changes: mutation_changes.clone(),
                        before_state,
                        after_state,
                    },
                )
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
            Record::__gom_queue_changed_event(
                &mut hook_ctx,
                action,
                ::std::option::Option::Some(&entity),
            )
            .await
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            hook_ctx
                .commit_and_emit_tracked(::std::option::Option::Some(&__gom_outcome))
                .await
                .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            match action {
                ::graphql_orm::graphql::orm::ChangeAction::Created => {}
                ::graphql_orm::graphql::orm::ChangeAction::Updated => {}
                ::graphql_orm::graphql::orm::ChangeAction::Deleted => {}
            }
            let mut result = UpsertRecordResult::ok(action, entity);
            result.entity = db
                .__release_graphql_mutation_output(
                    ctx,
                    "record",
                    result.entity.take(),
                    &Record::__gom_key_where_clause(),
                    Record::__gom_subscription_key_values,
                )
                .await?;
            ::std::result::Result::Ok(result)
        }
        .await;
        __gom_outcome.finish::<_, ::graphql_orm::SqliteBackend>(ctx, __gom_result)
    }
    #[doc = "Update one Record record"]
    async fn update(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        id: ::std::string::String,
        input: GraphQLUpdateRecordInput,
    ) -> ::graphql_orm::async_graphql::Result<RecordResult> {
        let __gom_outcome = ::graphql_orm::graphql::errors::MutationOutcomeTracker::new();
        let __gom_result : :: graphql_orm :: async_graphql :: Result < RecordResult > = async { :: graphql_orm :: graphql :: input_validation :: require_generated_input_validation (ctx) ? ; use :: graphql_orm :: graphql :: orm :: { DatabaseEntity , EntityQuery , FromSqlRow , SqlValue } ; let __gom_expected_version : :: std :: option :: Option < i64 > = :: std :: option :: Option :: None ; let _auth_subject = :: graphql_orm :: graphql :: auth :: enforce_resolver_auth (ctx , :: std :: option :: Option :: None) ? ; :: graphql_orm :: graphql :: orm :: ensure_rls_request_context :: < Record , :: graphql_orm :: SqliteBackend > (ctx , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: entity_name () ,) ? ; :: graphql_orm :: graphql :: assurance :: enforce_resolver_assurance (ctx , :: graphql_orm :: graphql :: orm :: GraphqlOperationKind :: Mutation ,) ? ; let db = :: graphql_orm :: db :: request_database :: < :: graphql_orm :: SqliteBackend > (ctx) ? ; let pool = db . pool () ; let auth_context = ctx . data_opt :: < :: graphql_orm :: graphql :: orm :: DbAuthContext > () . cloned () ; db . ensure_entity_access (:: std :: option :: Option :: Some (ctx) , "Record" , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () . write_policy , :: graphql_orm :: graphql :: orm :: EntityAccessKind :: Write , :: graphql_orm :: graphql :: orm :: EntityAccessSurface :: GraphqlMutation ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? ; db . ensure_row_write_provider ("Record" , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () . write_policy) . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? ; let mut input : UpdateRecordInput = input . into () ; # [allow (unused_mut)] let mut __gom_client_supplied : :: std :: collections :: HashSet < & 'static str > = :: std :: collections :: HashSet :: new () ; { let __gom_client_input = & input ; if __gom_client_input . title . is_some () { __gom_client_supplied . insert ("title") ; } } let mut hook_ctx = :: graphql_orm :: graphql :: orm :: MutationContext :: < :: graphql_orm :: SqliteBackend > :: begin_with_auth (db , :: graphql_orm :: graphql :: orm :: TransactionMode :: StateMachine , auth_context . as_ref () ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ? ; let current_entity = :: std :: boxed :: Box :: pin (:: graphql_orm :: graphql :: orm :: with_write_target_prefilter (db , :: std :: option :: Option :: Some (ctx) , EntityQuery :: < Record , :: graphql_orm :: SqliteBackend > :: new () . where_clause (& :: std :: format ! ("{} = {}" , :: graphql_orm :: graphql :: orm :: DatabaseBackend :: Sqlite . render_identifier_path (Record :: PRIMARY_KEY) , Record :: __gom_placeholder (1)) , Record :: __gom_key_values (& id) . remove (0)) ,)) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? . fetch_one_for_write_in_transaction (& mut hook_ctx) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ? ; if current_entity . is_none () { return :: std :: result :: Result :: Ok (RecordResult :: __gom_refusal (:: graphql_orm :: graphql :: errors :: OrmErrorCode :: NotFound , "Entity not found")) ; } let current_entity = current_entity . expect ("checked above") ; if ! db . authorize_write_target (:: std :: option :: Option :: Some (ctx) , "Record" , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () . read_policy , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () . write_policy , :: graphql_orm :: graphql :: orm :: EntityAccessSurface :: GraphqlMutation , & current_entity as & (dyn :: std :: any :: Any + :: std :: marker :: Send + :: std :: marker :: Sync) ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? { return :: std :: result :: Result :: Ok (RecordResult :: __gom_refusal (:: graphql_orm :: graphql :: errors :: OrmErrorCode :: NotFound , "Entity not found")) ; } if __gom_expected_version . is_some () && Record :: __gom_managed_version_of (& current_entity) != __gom_expected_version { return :: std :: result :: Result :: Err (:: graphql_orm :: graphql :: errors :: OrmPublicError :: new (:: graphql_orm :: graphql :: errors :: OrmErrorCode :: Conflict ,) . into_graphql_error ()) ; } { let mut write_ctx = :: graphql_orm :: graphql :: orm :: WriteInputContext :: graphql_in_transaction (ctx , "Record" , & mut hook_ctx ,) ; db . run_before_update_with_context (& mut write_ctx , :: std :: option :: Option :: Some (& current_entity as & (dyn :: std :: any :: Any + :: std :: marker :: Send + :: std :: marker :: Sync)) , & mut input as & mut (dyn :: std :: any :: Any + :: std :: marker :: Send + :: std :: marker :: Sync) ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? ; } let mut set_clauses : :: std :: vec :: Vec < :: std :: string :: String > = :: std :: vec :: Vec :: new () ; let mut changed_fields : :: std :: vec :: Vec < & str > = :: std :: vec :: Vec :: new () ; let mut values : :: std :: vec :: Vec < :: graphql_orm :: graphql :: orm :: SqlValue > = :: std :: vec :: Vec :: new () ; if __gom_client_supplied . contains ("title") { let __gom_field_write_allowed = if let :: std :: option :: Option :: Some (val) = & input . title { db . check_field_write (ctx , "Record" , "title" , :: std :: option :: Option :: None , :: std :: option :: Option :: Some (& current_entity as & (dyn :: std :: any :: Any + :: std :: marker :: Send + :: std :: marker :: Sync)) , :: std :: option :: Option :: Some (val as & (dyn :: std :: any :: Any + :: std :: marker :: Send + :: std :: marker :: Sync)) ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? } else { true } ; if ! __gom_field_write_allowed { input . title = :: std :: option :: Option :: None ; } } Record :: __gom_push_update_assignments (& input , & mut set_clauses , & mut changed_fields , & mut values ,) . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ? ; if false { set_clauses . push (:: std :: format ! ("\"updated_at\" = {}" , Record :: __gom_current_updated_at_expr ())) ; } if let :: std :: option :: Option :: Some (assignment) = Record :: __gom_managed_version_assignment () { set_clauses . push (assignment) ; } if set_clauses . is_empty () { return :: std :: result :: Result :: Ok (RecordResult :: __gom_refusal (:: graphql_orm :: graphql :: errors :: OrmErrorCode :: InvalidInput , "No fields to update")) ; } Record :: __gom_require_managed_version_headroom (& current_entity) . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ? ; let before_state = Record :: __gom_capture_entity_state (& current_entity) . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ? ; let mutation_changes = :: graphql_orm :: graphql :: orm :: mutation_changes (& changed_fields , & values) ; hook_ctx . run_mutation_hook (:: std :: option :: Option :: Some (ctx) , & :: graphql_orm :: graphql :: orm :: MutationEvent { phase : :: graphql_orm :: graphql :: orm :: MutationPhase :: Before , action : :: graphql_orm :: graphql :: orm :: ChangeAction :: Updated , entity_name : "Record" , table_name : "records" , metadata : < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () , id : id . to_string () , changes : mutation_changes . clone () , before_state : :: std :: option :: Option :: Some (before_state . clone ()) , after_state : :: std :: option :: Option :: None , } ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? ; let sql = Record :: __gom_rebind_sql (& :: std :: format ! ("UPDATE {} SET {} WHERE {} = ?{}" , "records" , set_clauses . join (", ") , :: graphql_orm :: graphql :: orm :: DatabaseBackend :: Sqlite . render_identifier_path (Record :: PRIMARY_KEY) , :: std :: format ! ("{}{}" , Record :: __gom_managed_version_guard () , Record :: __gom_managed_version_expectation (__gom_expected_version) ,) ,) , 1) ; values . push (Record :: __gom_key_values (& id) . remove (0)) ; __gom_outcome . write_attempted () ; let result = hook_ctx . execute (& sql , & values) . await ; match result { :: std :: result :: Result :: Ok (r) if < :: graphql_orm :: SqliteBackend as :: graphql_orm :: graphql :: orm :: WriteBackend > :: rows_affected (& r) == 1 => { let entity = Record :: __gom_fetch_by_id_on (& mut hook_ctx , & id) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ? ; match entity { :: std :: option :: Option :: Some (entity) => { hook_ctx . __ensure_written_row (:: std :: option :: Option :: Some (ctx) , "Record" , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () . write_policy , :: graphql_orm :: graphql :: orm :: EntityAccessSurface :: GraphqlMutation , & entity as & (dyn :: std :: any :: Any + :: std :: marker :: Send + :: std :: marker :: Sync) ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? ; let after_state = :: std :: option :: Option :: Some (Record :: __gom_capture_entity_state (& entity) . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ?) ; hook_ctx . run_mutation_hook (:: std :: option :: Option :: Some (ctx) , & :: graphql_orm :: graphql :: orm :: MutationEvent { phase : :: graphql_orm :: graphql :: orm :: MutationPhase :: After , action : :: graphql_orm :: graphql :: orm :: ChangeAction :: Updated , entity_name : "Record" , table_name : "records" , metadata : < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () , id : entity . id . to_string () , changes : mutation_changes . clone () , before_state : :: std :: option :: Option :: Some (before_state) , after_state , } ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? ; Record :: __gom_queue_changed_event (& mut hook_ctx , :: graphql_orm :: graphql :: orm :: ChangeAction :: Updated , :: std :: option :: Option :: Some (& entity) ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ? ; hook_ctx . commit_and_emit_tracked (:: std :: option :: Option :: Some (& __gom_outcome)) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ? ; let mut result = RecordResult :: ok (entity) ; result . entity = db . __release_graphql_mutation_output (ctx , "record" , result . entity . take () , & Record :: __gom_key_where_clause () , Record :: __gom_subscription_key_values ,) . await ? ; :: std :: result :: Result :: Ok (result) } , :: std :: option :: Option :: None => :: std :: result :: Result :: Ok (RecordResult :: __gom_refusal (:: graphql_orm :: graphql :: errors :: OrmErrorCode :: NotFound , "Entity not found after update")) , } } :: std :: result :: Result :: Ok (_) => :: std :: result :: Result :: Err (:: graphql_orm :: async_graphql :: Error :: new ("authorized update did not affect the exact locked row")) , :: std :: result :: Result :: Err (e) => :: std :: result :: Result :: Ok (RecordResult :: __gom_from_graphql_error (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx (e))) , } } . await ;
        __gom_outcome.finish::<_, ::graphql_orm::SqliteBackend>(ctx, __gom_result)
    }
    #[doc = "Delete one Record record"]
    async fn delete(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        id: ::std::string::String,
    ) -> ::graphql_orm::async_graphql::Result<RecordResult> {
        let __gom_outcome = ::graphql_orm::graphql::errors::MutationOutcomeTracker::new();
        let __gom_result : :: graphql_orm :: async_graphql :: Result < RecordResult > = async { use :: graphql_orm :: graphql :: orm :: { DatabaseEntity , EntityQuery , SqlValue } ; let __gom_expected_version : :: std :: option :: Option < i64 > = :: std :: option :: Option :: None ; if ctx . data_opt :: < :: graphql_orm :: graphql :: orm :: DependentDeleteGuard > () . is_some () { return :: std :: result :: Result :: Err (:: graphql_orm :: async_graphql :: Error :: new ("Dependent delete effect plan unavailable")) ; } let _auth_subject = :: graphql_orm :: graphql :: auth :: enforce_resolver_auth (ctx , :: std :: option :: Option :: None) ? ; :: graphql_orm :: graphql :: orm :: ensure_rls_request_context :: < Record , :: graphql_orm :: SqliteBackend > (ctx , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: entity_name () ,) ? ; :: graphql_orm :: graphql :: assurance :: enforce_resolver_assurance (ctx , :: graphql_orm :: graphql :: orm :: GraphqlOperationKind :: Mutation ,) ? ; let db = :: graphql_orm :: db :: request_database :: < :: graphql_orm :: SqliteBackend > (ctx) ? ; let pool = db . pool () ; let auth_context = ctx . data_opt :: < :: graphql_orm :: graphql :: orm :: DbAuthContext > () . cloned () ; db . ensure_entity_access (:: std :: option :: Option :: Some (ctx) , "Record" , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () . write_policy , :: graphql_orm :: graphql :: orm :: EntityAccessKind :: Write , :: graphql_orm :: graphql :: orm :: EntityAccessSurface :: GraphqlMutation ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? ; db . ensure_row_write_provider ("Record" , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () . write_policy) . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? ; let mut hook_ctx = :: graphql_orm :: graphql :: orm :: MutationContext :: < :: graphql_orm :: SqliteBackend > :: begin_with_auth (db , :: graphql_orm :: graphql :: orm :: TransactionMode :: StateMachine , auth_context . as_ref () ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ? ; let entity = :: std :: boxed :: Box :: pin (:: graphql_orm :: graphql :: orm :: with_write_target_prefilter (db , :: std :: option :: Option :: Some (ctx) , EntityQuery :: < Record , :: graphql_orm :: SqliteBackend > :: new () . where_clause (& :: std :: format ! ("{} = {}" , :: graphql_orm :: graphql :: orm :: DatabaseBackend :: Sqlite . render_identifier_path (Record :: PRIMARY_KEY) , Record :: __gom_placeholder (1)) , Record :: __gom_key_values (& id) . remove (0)) ,)) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? . fetch_one_for_write_in_transaction (& mut hook_ctx) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ? ; if entity . is_none () { return :: std :: result :: Result :: Ok (RecordResult :: __gom_refusal (:: graphql_orm :: graphql :: errors :: OrmErrorCode :: NotFound , "Entity not found")) ; } let entity = entity . unwrap () ; if ! db . authorize_write_target (:: std :: option :: Option :: Some (ctx) , "Record" , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () . read_policy , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () . write_policy , :: graphql_orm :: graphql :: orm :: EntityAccessSurface :: GraphqlMutation , & entity as & (dyn :: std :: any :: Any + :: std :: marker :: Send + :: std :: marker :: Sync) ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? { return :: std :: result :: Result :: Ok (RecordResult :: __gom_refusal (:: graphql_orm :: graphql :: errors :: OrmErrorCode :: NotFound , "Entity not found")) ; } if __gom_expected_version . is_some () && Record :: __gom_managed_version_of (& entity) != __gom_expected_version { return :: std :: result :: Result :: Err (:: graphql_orm :: graphql :: errors :: OrmPublicError :: new (:: graphql_orm :: graphql :: errors :: OrmErrorCode :: Conflict ,) . into_graphql_error ()) ; } let before_state = Record :: __gom_capture_entity_state (& entity) . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ? ; hook_ctx . run_mutation_hook (:: std :: option :: Option :: Some (ctx) , & :: graphql_orm :: graphql :: orm :: MutationEvent { phase : :: graphql_orm :: graphql :: orm :: MutationPhase :: Before , action : :: graphql_orm :: graphql :: orm :: ChangeAction :: Deleted , entity_name : "Record" , table_name : "records" , metadata : < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () , id : entity . id . to_string () , changes : :: std :: vec :: Vec :: new () , before_state : :: std :: option :: Option :: Some (before_state . clone ()) , after_state : :: std :: option :: Option :: None , } ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? ; let sql = Record :: __gom_rebind_sql (& :: std :: format ! ("DELETE FROM {} WHERE {} = ?{}" , "records" , :: graphql_orm :: graphql :: orm :: DatabaseBackend :: Sqlite . render_identifier_path (Record :: PRIMARY_KEY) , Record :: __gom_managed_version_expectation (__gom_expected_version) ,) , 1) ; let values = [Record :: __gom_key_values (& id) . remove (0)] ; __gom_outcome . write_attempted () ; let result = hook_ctx . execute (& sql , & values) . await ; match result { :: std :: result :: Result :: Ok (r) if < :: graphql_orm :: SqliteBackend as :: graphql_orm :: graphql :: orm :: WriteBackend > :: rows_affected (& r) == 1 => { hook_ctx . run_mutation_hook (:: std :: option :: Option :: Some (ctx) , & :: graphql_orm :: graphql :: orm :: MutationEvent { phase : :: graphql_orm :: graphql :: orm :: MutationPhase :: After , action : :: graphql_orm :: graphql :: orm :: ChangeAction :: Deleted , entity_name : "Record" , table_name : "records" , metadata : < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () , id : entity . id . to_string () , changes : :: std :: vec :: Vec :: new () , before_state : :: std :: option :: Option :: Some (before_state) , after_state : :: std :: option :: Option :: None , } ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? ; Record :: __gom_queue_changed_event (& mut hook_ctx , :: graphql_orm :: graphql :: orm :: ChangeAction :: Deleted , :: std :: option :: Option :: Some (& entity) ,) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ? ; hook_ctx . commit_and_emit_tracked (:: std :: option :: Option :: Some (& __gom_outcome)) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ? ; :: std :: result :: Result :: Ok (RecordResult :: __gom_deleted ()) } , :: std :: result :: Result :: Ok (_) => :: std :: result :: Result :: Err (:: graphql_orm :: async_graphql :: Error :: new ("authorized delete did not affect the exact locked row")) , :: std :: result :: Result :: Err (e) => :: std :: result :: Result :: Ok (RecordResult :: __gom_from_graphql_error (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx (e))) , } } . await ;
        __gom_outcome.finish::<_, ::graphql_orm::SqliteBackend>(ctx, __gom_result)
    }
    #[doc = "Update matching Record records"]
    async fn update_many(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        where_input: ::std::option::Option<RecordWhereInput>,
        input: GraphQLUpdateRecordInput,
    ) -> ::graphql_orm::async_graphql::Result<UpdateRecordsResult> {
        let __gom_outcome = ::graphql_orm::graphql::errors::MutationOutcomeTracker::new();
        let __gom_result: ::graphql_orm::async_graphql::Result<UpdateRecordsResult> = async {
            ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
            use ::graphql_orm::graphql::orm::{DatabaseFilter, EntityQuery};
            let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(
                ctx,
                ::std::option::Option::None,
            )?;
            ::graphql_orm::graphql::orm::ensure_rls_request_context::<
                Record,
                ::graphql_orm::SqliteBackend,
            >(
                ctx,
                <Record as ::graphql_orm::graphql::orm::Entity>::entity_name(),
            )?;
            ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                ctx,
                ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation,
            )?;
            let db = ::graphql_orm::db::request_database::<::graphql_orm::SqliteBackend>(ctx)?;
            let auth_context = ctx
                .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                .cloned();
            if let ::std::result::Result::Err(error) = db
                .ensure_entity_access(
                    ::std::option::Option::Some(ctx),
                    "Record",
                    <Record as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                )
                .await
            {
                return ::std::result::Result::Ok(UpdateRecordsResult::from_graphql_error(
                    ::graphql_orm::graphql::errors::graphql_error_from_callback(error),
                ));
            }
            if let ::std::result::Result::Err(error) = db.ensure_row_write_provider(
                "Record",
                <Record as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
            ) {
                return ::std::result::Result::Ok(UpdateRecordsResult::from_graphql_error(error));
            }
            let input: UpdateRecordInput = input.into();
            let filter = match where_input {
                ::std::option::Option::Some(ref f) if !f.is_empty() => f,
                _ => {
                    return ::std::result::Result::Ok(UpdateRecordsResult::__gom_refusal(
                        ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                        "Where filter is required for bulk update and must not be empty",
                    ));
                }
            };
            if let ::std::result::Result::Err(error) = filter.__gom_require_scalar_write_filter() {
                return ::std::result::Result::Ok(UpdateRecordsResult::from_graphql_error(
                    error.into_graphql_error(),
                ));
            }
            if let ::std::result::Result::Err(error) =
                ::graphql_orm::graphql::orm::authorize_read_fields::<
                    Record,
                    ::graphql_orm::SqliteBackend,
                >(
                    db,
                    ctx,
                    ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter),
                )
                .await
            {
                return ::std::result::Result::Ok(UpdateRecordsResult::from_graphql_error(error));
            }
            let mut notification_rows = ::std::vec::Vec::new();
            match Record::__gom_update_where_with_auth(
                db,
                filter.clone(),
                input,
                auth_context.as_ref(),
                ::std::option::Option::Some(ctx),
                ::std::option::Option::Some(&mut notification_rows),
                ::std::option::Option::Some(&__gom_outcome),
            )
            .await
            {
                ::std::result::Result::Ok(affected_count) => {
                    #[allow(unused_mut)]
                    let mut result = UpdateRecordsResult::ok(affected_count);
                    ::std::result::Result::Ok(result)
                }
                ::std::result::Result::Err(e) => {
                    let error = ::graphql_orm::graphql::errors::graphql_error_from_sqlx(e);
                    if __gom_outcome.outcome()
                        == ::graphql_orm::graphql::errors::MutationOutcome::Unknown
                        && ::graphql_orm::graphql::errors::MutationOutcomeTracker::enabled::<
                            ::graphql_orm::SqliteBackend,
                        >(ctx)
                    {
                        return ::std::result::Result::Err(error);
                    }
                    ::std::result::Result::Ok(UpdateRecordsResult::from_graphql_error(error))
                }
            }
        }
        .await;
        __gom_outcome.finish::<_, ::graphql_orm::SqliteBackend>(ctx, __gom_result)
    }
    #[doc = "Delete matching Record records"]
    async fn delete_many(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        where_input: ::std::option::Option<RecordWhereInput>,
    ) -> ::graphql_orm::async_graphql::Result<DeleteRecordsResult> {
        let __gom_outcome = ::graphql_orm::graphql::errors::MutationOutcomeTracker::new();
        let __gom_result: ::graphql_orm::async_graphql::Result<DeleteRecordsResult> = async {
            ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
            use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery};
            if ctx
                .data_opt::<::graphql_orm::graphql::orm::DependentDeleteGuard>()
                .is_some()
            {
                return ::std::result::Result::Err(::graphql_orm::async_graphql::Error::new(
                    "Dependent delete effect plan unavailable",
                ));
            }
            let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(
                ctx,
                ::std::option::Option::None,
            )?;
            ::graphql_orm::graphql::orm::ensure_rls_request_context::<
                Record,
                ::graphql_orm::SqliteBackend,
            >(
                ctx,
                <Record as ::graphql_orm::graphql::orm::Entity>::entity_name(),
            )?;
            ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                ctx,
                ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation,
            )?;
            let db = ::graphql_orm::db::request_database::<::graphql_orm::SqliteBackend>(ctx)?;
            let auth_context = ctx
                .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                .cloned();
            if let ::std::result::Result::Err(error) = db
                .ensure_entity_access(
                    ::std::option::Option::Some(ctx),
                    "Record",
                    <Record as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                )
                .await
            {
                return ::std::result::Result::Ok(DeleteRecordsResult::from_graphql_error(
                    ::graphql_orm::graphql::errors::graphql_error_from_callback(error),
                ));
            }
            if let ::std::result::Result::Err(error) = db.ensure_row_write_provider(
                "Record",
                <Record as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
            ) {
                return ::std::result::Result::Ok(DeleteRecordsResult::from_graphql_error(error));
            }
            let filter = match where_input {
                ::std::option::Option::Some(ref f) if !f.is_empty() => f,
                _ => {
                    return ::std::result::Result::Ok(DeleteRecordsResult::__gom_refusal(
                        ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                        "Where filter is required for bulk delete and must not be empty",
                    ));
                }
            };
            if let ::std::result::Result::Err(error) = filter.__gom_require_scalar_write_filter() {
                return ::std::result::Result::Ok(DeleteRecordsResult::from_graphql_error(
                    error.into_graphql_error(),
                ));
            }
            if let ::std::result::Result::Err(error) =
                ::graphql_orm::graphql::orm::authorize_read_fields::<
                    Record,
                    ::graphql_orm::SqliteBackend,
                >(
                    db,
                    ctx,
                    ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter),
                )
                .await
            {
                return ::std::result::Result::Ok(DeleteRecordsResult::from_graphql_error(error));
            }
            let mut notification_rows = ::std::vec::Vec::new();
            match Record::__gom_delete_where_with_auth(
                db,
                filter.clone(),
                auth_context.as_ref(),
                ::std::option::Option::Some(ctx),
                ::std::option::Option::Some(&mut notification_rows),
                ::std::option::Option::Some(&__gom_outcome),
            )
            .await
            {
                ::std::result::Result::Ok(deleted_count) => {
                    #[allow(unused_mut)]
                    let mut result = DeleteRecordsResult::ok(deleted_count);
                    ::std::result::Result::Ok(result)
                }
                ::std::result::Result::Err(e) => {
                    let error = ::graphql_orm::graphql::errors::graphql_error_from_sqlx(e);
                    if __gom_outcome.outcome()
                        == ::graphql_orm::graphql::errors::MutationOutcome::Unknown
                        && ::graphql_orm::graphql::errors::MutationOutcomeTracker::enabled::<
                            ::graphql_orm::SqliteBackend,
                        >(ctx)
                    {
                        return ::std::result::Result::Err(error);
                    }
                    ::std::result::Result::Ok(DeleteRecordsResult::from_graphql_error(error))
                }
            }
        }
        .await;
        __gom_outcome.finish::<_, ::graphql_orm::SqliteBackend>(ctx, __gom_result)
    }
}
#[doc = r" Generated subscriptions for this entity"]
#[derive(Default)]
pub struct RecordSubscriptions;
impl RecordSubscriptions {
    #[doc = "Observe changes to Record records Payload selections support persisted subscribable scalars only; relation/computed/custom selections are rejected. Subscriber authority is rechecked on an event-independent jittered timer: definite denial ends with FORBIDDEN and host failures with SUBSCRIPTION_CONTINUITY_LOST. Per-event denials silently suppress events. Select exactly one root occurrence per operation. Refresh authorized queries for richer data."]
    async fn on_changed(
        &self,
        ctx: &::graphql_orm::async_graphql::Context<'_>,
        _filter: ::std::option::Option<::graphql_orm::graphql::orm::SubscriptionFilterInput>,
        key: ::std::option::Option<RecordKey>,
        keys: ::std::option::Option<::std::vec::Vec<RecordKey>>,
    ) -> ::graphql_orm::async_graphql::Result<
        impl ::graphql_orm::futures::Stream<
            Item = ::graphql_orm::async_graphql::Result<
                ::graphql_orm::graphql::subscriptions::ChangedPayload<RecordChangedEvent>,
            >,
        >,
    > {
        use ::graphql_orm::futures::StreamExt;
        ::graphql_orm::graphql::subscriptions::admit_generated_operation(ctx)?;
        ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
        let _auth_subject =
            ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, ::std::option::Option::None)?;
        ::graphql_orm::graphql::orm::ensure_rls_request_context::<
            Record,
            ::graphql_orm::SqliteBackend,
        >(
            ctx,
            <Record as ::graphql_orm::graphql::orm::Entity>::entity_name(),
        )?;
        ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
            ctx,
            ::graphql_orm::graphql::orm::GraphqlOperationKind::Subscription,
        )?;
        let db = ::graphql_orm::db::request_database::<::graphql_orm::SqliteBackend>(ctx)?;
        db.ensure_entity_access(
            ::std::option::Option::Some(ctx),
            "Record",
            <Record as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
            ::graphql_orm::graphql::orm::EntityAccessKind::Read,
            ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlSubscription,
        )
        .await
        .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
        if _filter
            .as_ref()
            .is_some_and(|filter| filter.dummy.is_some())
        {
            return ::std::result::Result::Err(
                ::graphql_orm::graphql::errors::OrmPublicError::new(
                    ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                )
                .into_graphql_error(),
            );
        }
        ::graphql_orm::graphql::subscriptions::validate_subscription_selection::<
            Record,
            ::graphql_orm::SqliteBackend,
        >(ctx, "record")?;
        let keys = ::graphql_orm::graphql::subscriptions::admit_subscription_keys(
            db,
            key.is_some(),
            keys.map(|keys| keys.iter().map(Record::__gom_key_values).collect()),
        )?;
        let actions = _filter.and_then(|filter| filter.actions);
        let rx = db
            .ensure_event_sender::<::graphql_orm::graphql::subscriptions::SubscriptionEvent<
                RecordChangedEvent,
                ::graphql_orm::SqliteBackend,
            >>()
            .subscribe();
        let db = db.clone();
        let interval = db.subscription_authority_recheck_interval();
        let timer_db = db.clone();
        let events = :: graphql_orm :: graphql :: subscriptions :: continuity_events (rx) . filter_map (move | result | { let db = db . clone () ; let actions = actions . clone () ; let key = key . clone () ; let keys = keys . clone () ; async move { let witness = match result { :: std :: result :: Result :: Ok (witness) => witness , :: std :: result :: Result :: Err (error) => return :: std :: option :: Option :: Some (:: std :: result :: Result :: Err (error)) , } ; let errors_before = match :: graphql_orm :: graphql :: errors :: buffered_errors :: count (ctx) { :: std :: result :: Result :: Ok (count) => count , :: std :: result :: Result :: Err (error) => return :: std :: option :: Option :: Some (:: std :: result :: Result :: Err (error)) , } ; let delivered : :: std :: option :: Option < :: graphql_orm :: graphql :: subscriptions :: ChangedPayload < RecordChangedEvent > > = async move { let event = witness . event . as_ref () ; if key . as_ref () . is_some_and (| key | :: graphql_orm :: graphql :: loaders :: RelationKey :: from_sql_values (& Record :: __gom_key_values (key)) != :: graphql_orm :: graphql :: loaders :: RelationKey :: from_sql_values (& witness . keys)) { return :: std :: option :: Option :: None ; } if keys . as_ref () . is_some_and (| keys | ! keys . contains (& :: graphql_orm :: graphql :: loaders :: RelationKey :: from_sql_values (& witness . keys))) { return :: std :: option :: Option :: None ; } if :: graphql_orm :: graphql :: loaders :: RelationKey :: from_sql_values (& witness . keys) != :: graphql_orm :: graphql :: loaders :: RelationKey :: from_sql_values (& Record :: __gom_key_values (& event . id)) { return :: std :: option :: Option :: None ; } if actions . as_ref () . is_some_and (| actions | ! actions . contains (& event . action)) { return :: std :: option :: Option :: None ; } let authorize_current = | | async { let _auth_subject = :: graphql_orm :: graphql :: auth :: enforce_resolver_auth (ctx , :: std :: option :: Option :: None) ? ; :: graphql_orm :: graphql :: orm :: ensure_rls_request_context :: < Record , :: graphql_orm :: SqliteBackend > (ctx , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: entity_name () ,) ? ; :: graphql_orm :: graphql :: assurance :: enforce_resolver_assurance (ctx , :: graphql_orm :: graphql :: orm :: GraphqlOperationKind :: Subscription) ? ; witness . authorize_provenance (& db , ctx , event . source_entity . as_deref () , event . source_id . as_deref () , & event . path , event . change_kind == :: graphql_orm :: graphql :: orm :: ChangeKind :: Propagated) . await ? ; if witness . trusted_delete && event . entity . as_ref () . is_some_and (| entity | :: graphql_orm :: graphql :: loaders :: RelationKey :: from_sql_values (& Record :: __gom_subscription_key_values (entity)) != :: graphql_orm :: graphql :: loaders :: RelationKey :: from_sql_values (& witness . keys)) { return :: std :: result :: Result :: Err (:: graphql_orm :: graphql :: errors :: OrmPublicError :: forbidden () . into_graphql_error ()) ; } let preimage = if witness . trusted_delete && event . action == :: graphql_orm :: graphql :: orm :: ChangeAction :: Deleted { event . entity . as_ref () } else { :: std :: option :: Option :: None } ; let row = :: graphql_orm :: graphql :: subscriptions :: authorize_subscription_row :: < Record , :: graphql_orm :: SqliteBackend > (& db , ctx , & witness . keys , preimage) . await ? . ok_or_else (| | :: graphql_orm :: graphql :: errors :: OrmPublicError :: forbidden () . into_graphql_error ()) ? ; if key . is_some () || keys . is_some () { :: graphql_orm :: graphql :: subscriptions :: authorize_subscription_keys :: < Record , :: graphql_orm :: SqliteBackend > (& db , ctx , & row) . await ? ; } :: graphql_orm :: graphql :: subscriptions :: authorize_subscription_fields (& db , ctx , & row , "record") . await ? ; db . ensure_entity_access (:: std :: option :: Option :: Some (ctx) , "Record" , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () . read_policy , :: graphql_orm :: graphql :: orm :: EntityAccessKind :: Read , :: graphql_orm :: graphql :: orm :: EntityAccessSurface :: GraphqlSubscription) . await . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_callback) ? ; let _auth_subject = :: graphql_orm :: graphql :: auth :: enforce_resolver_auth (ctx , :: std :: option :: Option :: None) ? ; :: graphql_orm :: graphql :: orm :: ensure_rls_request_context :: < Record , :: graphql_orm :: SqliteBackend > (ctx , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: entity_name () ,) ? ; :: graphql_orm :: graphql :: assurance :: enforce_resolver_assurance (ctx , :: graphql_orm :: graphql :: orm :: GraphqlOperationKind :: Subscription) ? ; witness . authorize_provenance (& db , ctx , event . source_entity . as_deref () , event . source_id . as_deref () , & event . path , event . change_kind == :: graphql_orm :: graphql :: orm :: ChangeKind :: Propagated) . await ? ; Record :: __gom_authorize_changed_operation (ctx) ? ; :: std :: result :: Result :: Ok :: < Record , :: graphql_orm :: async_graphql :: Error > (row) } ; match authorize_current () . await { :: std :: result :: Result :: Ok (row) => { let __gom_event = RecordChangedEvent { action : event . action , change_kind : event . change_kind , id : event . id . clone () , source_entity : event . source_entity . clone () , source_id : event . source_id . clone () , path : event . path . clone () , entity : :: std :: option :: Option :: Some (row) , } ; let prepared = :: graphql_orm :: graphql :: subscriptions :: ChangedPayload :: prepare (__gom_event , ctx) . await ? ; match authorize_current () . await { :: std :: result :: Result :: Ok (_) => :: std :: option :: Option :: Some (prepared) , :: std :: result :: Result :: Err (error) => { let _ = :: graphql_orm :: graphql :: loaders :: relation_authorization_error (error) ; :: std :: option :: Option :: None } } } , :: std :: result :: Result :: Err (error) => { let _ = :: graphql_orm :: graphql :: loaders :: relation_authorization_error (error) ; :: std :: option :: Option :: None } } } . await ; if delivered . is_none () { if let :: std :: result :: Result :: Err (error) = :: graphql_orm :: graphql :: errors :: buffered_errors :: truncate (ctx , errors_before) { return :: std :: option :: Option :: Some (:: std :: result :: Result :: Err (error)) ; } } delivered . map (:: std :: result :: Result :: Ok) } }) ;
        ::std::result::Result::Ok(
            ::graphql_orm::graphql::subscriptions::watch_subscription_authority(
                events,
                interval,
                move || {
                    let db = timer_db.clone();
                    async move {
                        let result = async { Record :: __gom_authorize_changed_operation (ctx) ? ; :: graphql_orm :: graphql :: assurance :: enforce_resolver_assurance (ctx , :: graphql_orm :: graphql :: orm :: GraphqlOperationKind :: Subscription) ? ; if ! db . can_access_entity (:: std :: option :: Option :: Some (ctx) , "Record" , < Record as :: graphql_orm :: graphql :: orm :: Entity > :: metadata () . read_policy , :: graphql_orm :: graphql :: orm :: EntityAccessKind :: Read , :: graphql_orm :: graphql :: orm :: EntityAccessSurface :: GraphqlSubscription) . await ? { return :: std :: result :: Result :: Err (:: graphql_orm :: graphql :: errors :: OrmPublicError :: forbidden () . into_graphql_error ()) ; } Record :: __gom_authorize_changed_operation (ctx) ? ; :: graphql_orm :: graphql :: assurance :: enforce_resolver_assurance (ctx , :: graphql_orm :: graphql :: orm :: GraphqlOperationKind :: Subscription) } . await ;
                        if result.is_err() {
                            ::graphql_orm::graphql::errors::buffered_errors::clear(ctx);
                        }
                        result
                    }
                },
            ),
        )
    }
}
