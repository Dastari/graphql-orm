//! Execute public repository APIs using exact host-supplied keys.
use super::host_primary_keys::*;
use graphql_orm::prelude::*;

#[cfg(feature = "sqlite")]
pub type Backend = SqliteBackend;
#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
pub type Backend = PostgresBackend;
#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
pub type Backend = MssqlBackend;

pub async fn exercise(database: &Database<Backend>) -> Result<(), Box<dyn std::error::Error>> {
    macro_rules! keys {
        ($entity:ident, $create:ident, $update:ident, $values:expr) => {{
            let values = $values;
            let bare = $entity::insert(
                database.pool(),
                $create {
                    id: values[0].to_owned(),
                    label: "bare".into(),
                },
            )
            .await?;
            assert_eq!(bare.id, values[0]);
            assert_eq!(bare.label, "bare");
            let direct = $entity::insert(
                database,
                $create {
                    id: values[1].to_owned(),
                    label: "direct".into(),
                },
            )
            .await?;
            assert_eq!(direct.id, values[1]);
            let key = values[2].to_owned();
            let transactional = database
                .transaction(TransactionMode::StateMachine, |tx| {
                    Box::pin(async move {
                        Ok(tx
                            .insert::<$entity>($create {
                                id: key,
                                label: "transaction".into(),
                            })
                            .await?)
                    })
                })
                .await?;
            assert_eq!(transactional.id, values[2]);
            assert_eq!(transactional.label, "transaction");
            let fetched = $entity::find_by_id(database, &values[0]).await?.unwrap();
            assert_eq!(fetched.id, values[0]);
            let updated = $entity::update_by_id(
                database,
                &values[1],
                $update {
                    label: Some("updated".into()),
                },
            )
            .await?
            .unwrap();
            assert_eq!(updated.id, values[1]);
            assert_eq!(updated.label, "updated");
        }};
    }
    keys!(
        HostI16,
        CreateHostI16Input,
        UpdateHostI16Input,
        [i16::MIN, 0, i16::MAX]
    );
    keys!(
        HostI32,
        CreateHostI32Input,
        UpdateHostI32Input,
        [i32::MIN, 0, i32::MAX]
    );
    keys!(
        HostI64,
        CreateHostI64Input,
        UpdateHostI64Input,
        [i64::MIN, 0, i64::MAX]
    );
    keys!(
        HostString,
        CreateHostStringInput,
        UpdateHostStringInput,
        [String::new(), "host-π".to_owned(), "literal%_\\".to_owned()]
    );
    keys!(
        HostUuid,
        CreateHostUuidInput,
        UpdateHostUuidInput,
        [
            graphql_orm::uuid::Uuid::nil(),
            graphql_orm::uuid::Uuid::from_u128(42),
            graphql_orm::uuid::Uuid::from_u128(u128::MAX)
        ]
    );
    keys!(
        HostBytes,
        CreateHostBytesInput,
        UpdateHostBytesInput,
        [vec![0], vec![255, 0, 17], vec![1, 2, 3]]
    );
    #[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
    {
        keys!(
            HostI8,
            CreateHostI8Input,
            UpdateHostI8Input,
            [i8::MIN, 0, i8::MAX]
        );
        keys!(
            HostIsize,
            CreateHostIsizeInput,
            UpdateHostIsizeInput,
            [-9isize, 0, 17]
        );
        keys!(
            HostU8,
            CreateHostU8Input,
            UpdateHostU8Input,
            [0u8, 127, u8::MAX]
        );
        keys!(
            HostU16,
            CreateHostU16Input,
            UpdateHostU16Input,
            [0u16, 32768, u16::MAX]
        );
        keys!(
            HostU32,
            CreateHostU32Input,
            UpdateHostU32Input,
            [0u32, i32::MAX as u32 + 1, u32::MAX]
        );
        // SqlValue's existing integer domain is signed i64; this fix does not widen it.
        keys!(
            HostU64,
            CreateHostU64Input,
            UpdateHostU64Input,
            [0u64, u32::MAX as u64 + 1, i64::MAX as u64]
        );
        keys!(
            HostUsize,
            CreateHostUsizeInput,
            UpdateHostUsizeInput,
            [0usize, 1, 2]
        );
    }
    let first = HostComposite::insert(
        database,
        CreateHostCompositeInput {
            namespace: "same-owner".into(),
            id: 1,
            label: "first".into(),
        },
    )
    .await?;
    assert_eq!(first.id, 1);
    let second = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                Ok(tx
                    .insert::<HostComposite>(CreateHostCompositeInput {
                        namespace: "same-owner".into(),
                        id: 2,
                        label: "second".into(),
                    })
                    .await?)
            })
        })
        .await?;
    assert_eq!(second.id, 2);
    assert_eq!(second.namespace, "same-owner");
    let first = HostComposite::find_by_key(
        database,
        &HostCompositeKey {
            namespace: "same-owner".into(),
            id: 1,
        },
    )
    .await?
    .unwrap();
    assert_eq!(first.label, "first");
    let rolled_back = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let staged = tx
                    .insert::<HostI32>(CreateHostI32Input {
                        id: 17,
                        label: "rollback".into(),
                    })
                    .await?;
                assert_eq!(staged.id, 17);
                tx.insert::<HostString>(CreateHostStringInput {
                    id: "atomic-journal".into(),
                    label: "rollback".into(),
                })
                .await?;
                Err::<(), _>(graphql_orm::graphql::errors::OrmPublicError::new(
                    graphql_orm::graphql::errors::OrmErrorCode::Conflict,
                ))
            })
        })
        .await;
    assert!(rolled_back.is_err());
    assert!(HostI32::find_by_id(database, &17).await?.is_none());
    assert!(
        HostString::find_by_id(database, &"atomic-journal".to_owned())
            .await?
            .is_none()
    );
    Ok(())
}
