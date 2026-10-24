use std::time::Duration;

use graphql_orm_storage::{
    BlobStore, CapacityQuota, CapacityRequest, CapacityState, CapacityValidationError,
    LocalStorageBackend, StorageError,
};
use tempfile::TempDir;

#[tokio::test]
async fn local_capacity_observes_the_root_filesystem_for_the_caller() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    let observation = backend
        .observe_capacity(&CapacityRequest::default())
        .await
        .expect("observe capacity");

    assert_eq!(observation.binding, backend.capacity_binding());
    assert!(observation.valid_until > observation.observed_at);
    let state = observation
        .validate_for(&backend.capacity_binding(), observation.observed_at)
        .expect("fresh observation");

    if cfg!(unix) {
        let CapacityState::Known(known) = state else {
            panic!("expected known local capacity, got {state:?}");
        };
        let total = known.total_bytes.expect("total bytes");
        let physical = known.physical_available_bytes.expect("physical bytes");
        assert!(total > 0);
        assert!(physical <= total);
        assert!(known.caller_available_bytes <= physical);
        if let CapacityQuota::Limited(limits) = &known.quota {
            assert!(!limits.is_empty());
            for limit in limits {
                assert!(known.caller_available_bytes <= limit.remaining_bytes());
            }
        }
        assert!(
            observation
                .placement
                .as_ref()
                .is_some_and(|placement| placement.0.starts_with("local:"))
        );
    } else {
        assert!(matches!(state, CapacityState::Unknown(_)));
    }
}

#[tokio::test]
async fn local_capacity_binding_tracks_the_configured_root() {
    let first = TempDir::new().expect("first dir");
    let second = TempDir::new().expect("second dir");
    let first_backend = LocalStorageBackend::new(first.path());
    let second_backend = LocalStorageBackend::new(second.path());
    assert_eq!(
        first_backend.capacity_binding(),
        LocalStorageBackend::new(first.path()).capacity_binding()
    );
    assert_ne!(
        first_backend.capacity_binding(),
        second_backend.capacity_binding()
    );

    let observation = first_backend
        .observe_capacity(&CapacityRequest::default())
        .await
        .expect("observe capacity");
    assert_eq!(
        observation.validate_for(&second_backend.capacity_binding(), observation.observed_at),
        Err(CapacityValidationError::BindingMismatch)
    );

    // Two tempdirs on the same filesystem share a placement but not a binding.
    let other = second_backend
        .observe_capacity(&CapacityRequest::default())
        .await
        .expect("observe capacity");
    assert!(!observation.same_placement(&other));
    let again = first_backend
        .observe_capacity(&CapacityRequest::default())
        .await
        .expect("observe capacity");
    if cfg!(unix) {
        assert!(observation.same_placement(&again));
    }
}

#[tokio::test]
async fn local_capacity_observations_expire() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    let observation = backend
        .observe_capacity(&CapacityRequest {
            max_age: Duration::from_secs(1),
        })
        .await
        .expect("observe capacity");
    let later = observation.observed_at + time::Duration::seconds(1);
    assert!(matches!(
        observation.validate_for(&backend.capacity_binding(), later),
        Err(CapacityValidationError::Expired { .. })
    ));
}

#[tokio::test]
async fn local_capacity_fails_for_missing_or_non_directory_roots() {
    let temp = TempDir::new().expect("temp dir");
    let missing = LocalStorageBackend::new(temp.path().join("missing"));
    let error = missing
        .observe_capacity(&CapacityRequest::default())
        .await
        .expect_err("missing root must fail");
    assert!(matches!(error, StorageError::Io { .. }));

    let file = temp.path().join("file");
    std::fs::write(&file, b"not a directory").expect("write file");
    let error = LocalStorageBackend::new(&file)
        .observe_capacity(&CapacityRequest::default())
        .await
        .expect_err("file root must fail");
    assert!(matches!(error, StorageError::Io { .. }));
}
