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
