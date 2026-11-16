use graphql_orm::graphql::orm::{
    PaginationConfig, RuntimeQueryErrorCode, RuntimeRelationErrorCode,
};
#[test]
fn main_struct_literal_and_exhaustive_matches_compile() {
    let _ = PaginationConfig {
        default_limit: Some(50),
        max_limit: Some(100),
        max_scanned_rows: Some(10000),
        max_bulk_write_rows: Some(10000),
    };
    let code = RuntimeQueryErrorCode::InvalidRequest;
    match code {
        RuntimeQueryErrorCode::InvalidHandle => (),
        RuntimeQueryErrorCode::InvalidRequest => (),
        RuntimeQueryErrorCode::InvalidFilter => (),
        RuntimeQueryErrorCode::UnsupportedOperator => (),
        RuntimeQueryErrorCode::UnsupportedOrder => (),
        RuntimeQueryErrorCode::UnsupportedBackend => (),
        RuntimeQueryErrorCode::ResourceLimit => (),
        RuntimeQueryErrorCode::CursorInvalid => (),
        RuntimeQueryErrorCode::CursorSchemaMismatch => (),
        RuntimeQueryErrorCode::Decode => (),
        RuntimeQueryErrorCode::BackendExecution => (),
        RuntimeQueryErrorCode::InvalidSearch => (),
    }
    let code = RuntimeRelationErrorCode::InvalidRequest;
    match code {
        RuntimeRelationErrorCode::InvalidRelation => (),
        RuntimeRelationErrorCode::InvalidRequest => (),
        RuntimeRelationErrorCode::InvalidParent => (),
        RuntimeRelationErrorCode::SchemaMismatch => (),
        RuntimeRelationErrorCode::ResourceLimit => (),
        RuntimeRelationErrorCode::UnsupportedKey => (),
        RuntimeRelationErrorCode::UnsupportedBackend => (),
        RuntimeRelationErrorCode::CursorInvalid => (),
        RuntimeRelationErrorCode::CursorMismatch => (),
        RuntimeRelationErrorCode::CardinalityViolation => (),
        RuntimeRelationErrorCode::Decode => (),
        RuntimeRelationErrorCode::BackendExecution => (),
    }
}
