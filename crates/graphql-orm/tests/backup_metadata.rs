use graphql_orm::graphql::orm::{
    ColumnBackupPolicy, Entity, backup_descriptors_from_entities, schema_snapshot_from_entities,
};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "backup_users",
    plural = "BackupUsers",
    default_sort = "email ASC"
)]
struct BackupUser {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub email: String,

    #[graphql_orm(db_column = "display_name")]
    pub name: String,

    #[backup(redact)]
    pub password_hash: String,

    #[backup(exclude)]
    pub session_secret: Option<String>,

    pub created_at: i64,
}

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "backup_posts",
    plural = "BackupPosts",
    default_sort = "title ASC"
)]
struct BackupPost {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "uuid")]
    pub user_id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub title: String,

    #[graphql(skip)]
    #[relation(target = "BackupUser", from = "user_id", to = "id")]
    pub user: Option<BackupUser>,
}

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(table = "backup_disabled", plural = "BackupDisabled", backup = false)]
struct BackupDisabled {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
}

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "backup_manual_order",
    plural = "BackupManualOrder",
    backup_restore_order = 200
)]
struct BackupManualOrder {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
}

#[test]
fn generated_entity_metadata_produces_backup_descriptors() {
    let descriptors = backup_descriptors_from_entities(&[
        <BackupPost as Entity>::metadata(),
        <BackupUser as Entity>::metadata(),
        <BackupDisabled as Entity>::metadata(),
        <BackupManualOrder as Entity>::metadata(),
    ]);

    assert_eq!(descriptors.len(), 3);

    let user = descriptors
        .iter()
        .find(|descriptor| descriptor.table_name == "backup_users")
        .expect("backup user descriptor");
    assert_eq!(user.entity_name, "BackupUser");
    assert_eq!(user.primary_key_column, "id");

    let password = user
        .columns
        .iter()
        .find(|column| column.rust_field_name == "password_hash")
        .expect("password column");
    assert_eq!(password.column_name, "password_hash");
    assert_eq!(password.backup_policy, ColumnBackupPolicy::Redact);

    let email = user
        .columns
        .iter()
        .find(|column| column.rust_field_name == "email")
        .expect("email column");
    assert_eq!(email.backup_policy, ColumnBackupPolicy::Include);

    let display_name = user
        .columns
        .iter()
        .find(|column| column.rust_field_name == "name")
        .expect("renamed display name column");
    assert_eq!(display_name.column_name, "display_name");

    let session_secret = user
        .columns
        .iter()
        .find(|column| column.rust_field_name == "session_secret")
        .expect("session secret column");
    assert_eq!(session_secret.backup_policy, ColumnBackupPolicy::Exclude);

    let post = descriptors
        .iter()
        .find(|descriptor| descriptor.table_name == "backup_posts")
        .expect("backup post descriptor");
    assert!(post.restore_order > user.restore_order);
    assert_eq!(post.dependencies.len(), 1);
    assert_eq!(post.dependencies[0].entity_name, "BackupUser");
    assert_eq!(post.dependencies[0].table_name, "backup_users");
    assert_eq!(post.dependencies[0].source_column, "user_id");
    assert_eq!(post.dependencies[0].target_column, "id");

    let manual = descriptors
        .iter()
        .find(|descriptor| descriptor.table_name == "backup_manual_order")
        .expect("manual order descriptor");
    assert_eq!(manual.restore_order, 200);
}

#[test]
fn schema_snapshot_hash_is_stable_for_entity_ordering() {
    let first = schema_snapshot_from_entities(
        graphql_orm::graphql::orm::DatabaseBackend::Sqlite,
        "20260514",
        &[
            <BackupPost as Entity>::metadata(),
            <BackupUser as Entity>::metadata(),
            <BackupManualOrder as Entity>::metadata(),
        ],
    );
    let second = schema_snapshot_from_entities(
        graphql_orm::graphql::orm::DatabaseBackend::Sqlite,
        "20260514",
        &[
            <BackupUser as Entity>::metadata(),
            <BackupManualOrder as Entity>::metadata(),
            <BackupPost as Entity>::metadata(),
        ],
    );

    assert_eq!(first.schema_hash, second.schema_hash);
}
