use graphql_orm::graphql::orm::{FencedLeaseState, LeaseError};
use uuid::Uuid;

#[test]
fn reclaim_increments_fence_and_rejects_stale_worker() {
    let mut state = FencedLeaseState::new("run-1", 0);
    let worker_a_attempt = Uuid::from_u128(1);
    let worker_b_attempt = Uuid::from_u128(2);

    let worker_a = state
        .claim("worker-a", worker_a_attempt, 1_000, 100, 0)
        .expect("first claim should succeed");
    assert_eq!(worker_a.fencing_token.get(), 1);

    let worker_b = state
        .claim("worker-b", worker_b_attempt, 1_101, 100, 1)
        .expect("expired lease should be reclaimable");
    assert_eq!(worker_b.fencing_token.get(), 2);

    assert_eq!(
        state.validate(&worker_a, 1_102, 2),
        Err(LeaseError::StaleFence)
    );
    assert_eq!(state.validate(&worker_b, 1_102, 2), Ok(()));
}

#[test]
fn every_transition_requires_current_cas_version() {
    let mut state = FencedLeaseState::new("run-1", 7);
    let proof = state
        .claim("worker", Uuid::from_u128(3), 1_000, 100, 7)
        .expect("claim should succeed");

    assert_eq!(
        state.heartbeat(&proof, 1_010, 100, 7),
        Err(LeaseError::VersionConflict {
            expected: 7,
            actual: 8,
        })
    );
    state
        .heartbeat(&proof, 1_010, 100, 8)
        .expect("current version should heartbeat");
    state
        .release(&proof, 1_020, 9)
        .expect("current fence and version should release");
    assert!(!state.is_active(1_020));
}

#[test]
fn active_lease_cannot_be_stolen() {
    let mut state = FencedLeaseState::new("run-1", 0);
    state
        .claim("worker-a", Uuid::from_u128(1), 1_000, 100, 0)
        .expect("first claim should succeed");

    assert_eq!(
        state.claim("worker-b", Uuid::from_u128(2), 1_050, 100, 1),
        Err(LeaseError::AlreadyLeased)
    );
}

#[test]
fn child_write_advances_version_and_stale_attempt_still_cannot_append() {
    let mut state = FencedLeaseState::new("run-1", 0);
    let worker_a = state
        .claim("worker-a", Uuid::from_u128(1), 1_000, 100, 0)
        .expect("first claim should succeed");
    let worker_b = state
        .claim("worker-b", Uuid::from_u128(2), 1_101, 100, 1)
        .expect("expired lease should be reclaimable");

    assert_eq!(
        state.commit_fenced_write(&worker_a, 1_102, 2),
        Err(LeaseError::StaleFence)
    );
    assert_eq!(
        state
            .commit_fenced_write(&worker_b, 1_102, 2)
            .expect("current worker should append"),
        3
    );
}

#[test]
fn overflow_errors_leave_lease_state_unchanged() {
    let mut claim_state = FencedLeaseState::new("run-1", i64::MAX);
    let original_claim_state = claim_state.clone();
    assert_eq!(
        claim_state.claim("worker", Uuid::from_u128(1), 1_000, 100, i64::MAX),
        Err(LeaseError::VersionExhausted)
    );
    assert_eq!(claim_state, original_claim_state);

    let mut heartbeat_state = FencedLeaseState::new("run-2", i64::MAX - 1);
    let proof = heartbeat_state
        .claim("worker", Uuid::from_u128(2), 1_000, 100, i64::MAX - 1)
        .expect("claim should consume the last available version");
    let original_heartbeat_state = heartbeat_state.clone();
    assert_eq!(
        heartbeat_state.heartbeat(&proof, 1_010, 100, i64::MAX),
        Err(LeaseError::VersionExhausted)
    );
    assert_eq!(heartbeat_state, original_heartbeat_state);

    assert_eq!(
        heartbeat_state.release(&proof, 1_010, i64::MAX),
        Err(LeaseError::VersionExhausted)
    );
    assert_eq!(heartbeat_state, original_heartbeat_state);
}
