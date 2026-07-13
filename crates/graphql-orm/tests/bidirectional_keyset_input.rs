use graphql_orm::graphql::errors::OrmErrorCode;
use graphql_orm::graphql::orm::{
    DatabaseBackend, KeysetDirection, KeysetNulls, KeysetOrderColumn, render_keyset_before,
};
use graphql_orm::graphql::pagination::{KeysetConnectionInput, KeysetValue, KeysetWindowDirection};

#[test]
fn validates_forward_tail_and_backward_history_windows() {
    let forward = KeysetConnectionInput {
        after: Some("cursor-a".to_owned()),
        first: Some(50),
        ..KeysetConnectionInput::default()
    }
    .validate(50, 200)
    .expect("forward input should validate");
    assert_eq!(forward.direction, KeysetWindowDirection::Forward);
    assert_eq!(forward.cursor.as_deref(), Some("cursor-a"));
    assert_eq!(forward.limit, 50);

    let backward = KeysetConnectionInput {
        before: Some("cursor-b".to_owned()),
        last: Some(500),
        ..KeysetConnectionInput::default()
    }
    .validate(50, 200)
    .expect("backward input should validate and clamp");
    assert_eq!(backward.direction, KeysetWindowDirection::Backward);
    assert_eq!(backward.cursor.as_deref(), Some("cursor-b"));
    assert_eq!(backward.limit, 200);
}

#[test]
fn rejects_mixed_direction_and_non_positive_pages() {
    for input in [
        KeysetConnectionInput {
            first: Some(10),
            last: Some(10),
            ..KeysetConnectionInput::default()
        },
        KeysetConnectionInput {
            after: Some("a".to_owned()),
            last: Some(10),
            ..KeysetConnectionInput::default()
        },
        KeysetConnectionInput {
            before: Some("b".to_owned()),
            first: Some(10),
            ..KeysetConnectionInput::default()
        },
        KeysetConnectionInput {
            first: Some(0),
            ..KeysetConnectionInput::default()
        },
        KeysetConnectionInput {
            before: Some("b".to_owned()),
            ..KeysetConnectionInput::default()
        },
    ] {
        let error = input
            .validate(50, 200)
            .expect_err("invalid combination must fail");
        assert_eq!(error.code, OrmErrorCode::InvalidInput);
    }
}

#[test]
fn before_predicate_reverses_order_and_preserves_null_semantics() {
    let columns = [
        KeysetOrderColumn {
            column: "created_at",
            direction: KeysetDirection::Desc,
            nulls: KeysetNulls::Last,
        },
        KeysetOrderColumn::asc("id"),
    ];
    let (sql, values) = render_keyset_before(
        DatabaseBackend::Postgres,
        &columns,
        &[KeysetValue::Int(10), KeysetValue::String("id-5".to_owned())],
        1,
    )
    .expect("before predicate should render");

    assert!(sql.contains("created_at > $1"));
    assert!(sql.contains("id < $3"));
    assert_eq!(values.len(), 3);
}
