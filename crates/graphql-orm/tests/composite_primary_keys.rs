use graphql_orm::prelude::*;

#[cfg(feature = "sqlite")]
mod sqlite_fixture {
    use super::*;
    use graphql_orm::graphql::orm::{DatabaseEntity, Entity, SqlValue};

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
        PartialEq,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "jim_labour",
        plural = "JimLabourEntries",
        default_sort = "JimObjectType ASC, RefNo ASC, LineNum ASC"
    )]
    pub struct JimLabourEntry {
        #[primary_key]
        #[graphql(name = "JimObjectType")]
        #[graphql_orm(db_column = "JimObjectType", write = false)]
        #[filterable(type = "number")]
        #[sortable]
        pub jim_object_type: i32,

        #[primary_key]
        #[graphql(name = "RefNo")]
        #[graphql_orm(db_column = "RefNo", write = false)]
        #[filterable(type = "number")]
        #[sortable]
        pub ref_no: i32,

        #[primary_key]
        #[graphql(name = "LineNum")]
        #[graphql_orm(db_column = "LineNum", write = false)]
        #[filterable(type = "number")]
        #[sortable]
        pub line_num: i16,

        #[graphql(name = "LabourDate")]
        #[graphql_orm(db_column = "LabourDate", write = false)]
        #[filterable(type = "date")]
        pub labour_date: Option<String>,
    }

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
        PartialEq,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "single_key_records",
        plural = "SingleKeyRecords",
        default_sort = "id ASC"
    )]
    pub struct SingleKeyRecord {
        #[primary_key]
        pub id: String,

        #[sortable]
        pub name: String,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [JimLabourEntry, SingleKeyRecord],
    }

    async fn setup_pool() -> sqlx::SqlitePool {
        sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("sqlite pool")
    }

    #[test]
    fn composite_primary_key_metadata_exposes_all_keys() {
        assert_eq!(JimLabourEntry::PRIMARY_KEY, "JimObjectType");
        assert_eq!(
            JimLabourEntry::PRIMARY_KEYS,
            &["JimObjectType", "RefNo", "LineNum"]
        );

        let metadata = <JimLabourEntry as Entity>::metadata();
        assert_eq!(metadata.primary_key, "JimObjectType");
        assert_eq!(
            metadata.primary_keys.as_ref(),
            ["JimObjectType", "RefNo", "LineNum"]
        );

        let primary_fields = metadata
            .fields
            .iter()
            .filter(|field| field.is_primary_key)
            .map(|field| field.rust_name)
            .collect::<Vec<_>>();
        assert_eq!(
            primary_fields,
            vec!["jim_object_type", "ref_no", "line_num"]
        );
    }

    #[test]
    fn single_primary_key_metadata_stays_compatible() {
        assert_eq!(SingleKeyRecord::PRIMARY_KEY, "id");
        assert_eq!(SingleKeyRecord::PRIMARY_KEYS, &["id"]);

        let metadata = <SingleKeyRecord as Entity>::metadata();
        assert_eq!(metadata.primary_key, "id");
        assert_eq!(metadata.primary_keys.as_ref(), ["id"]);
    }

    #[test]
    fn sqlite_composite_lookup_uses_ordered_qmark_conditions_and_values() {
        let key = JimLabourEntryKey {
            jim_object_type: 1,
            ref_no: 12_345,
            line_num: 2,
        };

        assert_eq!(
            JimLabourEntry::__gom_key_where_clause(),
            "\"JimObjectType\" = ? AND \"RefNo\" = ? AND \"LineNum\" = ?"
        );
        assert_eq!(
            JimLabourEntry::__gom_key_values(&key),
            vec![SqlValue::Int(1), SqlValue::Int(12_345), SqlValue::Int(2)]
        );
    }

    #[tokio::test]
    async fn composite_lookup_sdl_uses_one_argument_per_key() {
        let pool = setup_pool().await;
        let schema = schema_builder(graphql_orm::db::Database::new(pool)).finish();
        let sdl = schema.sdl();

        assert!(sdl.contains(
            "jimLabourEntry(jimObjectType: Int!, refNo: Int!, lineNum: Int!): JimLabourEntry"
        ));
        assert!(sdl.contains("singleKeyRecord(id: String!): SingleKeyRecord"));
        assert!(!sdl.contains("createJimLabourEntry("));
        assert!(!sdl.contains("updateJimLabourEntry("));
        assert!(!sdl.contains("deleteJimLabourEntry("));
        assert!(sdl.contains(
            "createSingleKeyRecord(input: CreateSingleKeyRecordInput!): SingleKeyRecordResult!"
        ));
    }
}

#[cfg(feature = "postgres")]
mod postgres_fixture {
    use super::*;
    use graphql_orm::graphql::orm::SqlValue;

    #[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq)]
    #[graphql_entity(
        backend = "postgres",
        table = "jim_labour",
        plural = "JimLabourEntries",
        default_sort = "JimObjectType ASC, RefNo ASC, LineNum ASC"
    )]
    pub struct JimLabourEntry {
        #[primary_key]
        #[graphql(name = "JimObjectType")]
        #[graphql_orm(db_column = "JimObjectType", write = false)]
        #[sortable]
        pub jim_object_type: i32,

        #[primary_key]
        #[graphql(name = "RefNo")]
        #[graphql_orm(db_column = "RefNo", write = false)]
        #[sortable]
        pub ref_no: i32,

        #[primary_key]
        #[graphql(name = "LineNum")]
        #[graphql_orm(db_column = "LineNum", write = false)]
        #[sortable]
        pub line_num: i16,
    }

    #[test]
    fn postgres_composite_lookup_uses_ordered_numbered_conditions() {
        let key = JimLabourEntryKey {
            jim_object_type: 1,
            ref_no: 12_345,
            line_num: 2,
        };

        assert_eq!(
            JimLabourEntry::__gom_key_where_clause(),
            "\"JimObjectType\" = $1 AND \"RefNo\" = $2 AND \"LineNum\" = $3"
        );
        assert_eq!(
            JimLabourEntry::__gom_key_values(&key),
            vec![SqlValue::Int(1), SqlValue::Int(12_345), SqlValue::Int(2)]
        );
    }
}

#[cfg(feature = "mssql")]
mod mssql_fixture {
    use super::*;
    use graphql_orm::graphql::orm::{DatabaseBackend, DatabaseEntity, SqlDialect, SqlValue};

    #[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq)]
    #[graphql_entity(
        backend = "mssql",
        table = "dbo.JimLabour",
        plural = "JimLabourEntries",
        default_sort = "[JimObjectType] ASC, [RefNo] ASC, [LineNum] ASC"
    )]
    pub struct JimLabourEntry {
        #[primary_key]
        #[graphql(name = "JimObjectType")]
        #[graphql_orm(db_column = "JimObjectType", write = false)]
        #[sortable]
        pub jim_object_type: i32,

        #[primary_key]
        #[graphql(name = "RefNo")]
        #[graphql_orm(db_column = "RefNo", write = false)]
        #[sortable]
        pub ref_no: i32,

        #[primary_key]
        #[graphql(name = "LineNum")]
        #[graphql_orm(db_column = "LineNum", write = false)]
        #[sortable]
        pub line_num: i16,

        #[graphql(name = "LabourDate")]
        #[graphql_orm(db_column = "LabourDate", write = false)]
        pub labour_date: Option<String>,
    }

    schema_roots! {
        backend: "mssql",
        query_custom_ops: [],
        entities: [JimLabourEntry],
    }

    #[test]
    fn mssql_composite_lookup_uses_ordered_tiberius_conditions() {
        let key = JimLabourEntryKey {
            jim_object_type: 1,
            ref_no: 12_345,
            line_num: 2,
        };

        assert_eq!(
            JimLabourEntry::TABLE_NAME,
            DatabaseBackend::Mssql.quote_identifier_path("dbo.JimLabour")
        );
        assert_eq!(
            JimLabourEntry::PRIMARY_KEYS,
            &["[JimObjectType]", "[RefNo]", "[LineNum]"]
        );
        assert_eq!(
            JimLabourEntry::__gom_key_where_clause(),
            "[JimObjectType] = @P1 AND [RefNo] = @P2 AND [LineNum] = @P3"
        );
        assert_eq!(
            JimLabourEntry::__gom_key_values(&key),
            vec![SqlValue::Int(1), SqlValue::Int(12_345), SqlValue::Int(2)]
        );
    }

    #[test]
    fn mssql_composite_schema_is_read_only() {
        let schema = graphql_orm::async_graphql::Schema::build(
            QueryRoot::default(),
            graphql_orm::async_graphql::EmptyMutation,
            graphql_orm::async_graphql::EmptySubscription,
        )
        .finish();
        let sdl = schema.sdl();

        assert!(sdl.contains(
            "jimLabourEntry(jimObjectType: Int!, refNo: Int!, lineNum: Int!): JimLabourEntry"
        ));
        assert!(!sdl.contains("type Mutation"));
        assert!(!sdl.contains("type Subscription"));
    }
}
