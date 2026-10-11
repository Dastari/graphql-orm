//! Monotonic transaction state for cancellation-safe runtime work.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
pub(crate) struct RuntimeTransactionState {
    unfinished: AtomicUsize,
    poisoned: AtomicBool,
}
impl RuntimeTransactionState {
    pub(crate) fn enter(self: &Arc<Self>) -> RuntimeOperationGuard {
        self.unfinished.fetch_add(1, Ordering::SeqCst);
        RuntimeOperationGuard {
            state: self.clone(),
            complete: false,
        }
    }
    pub(crate) fn cannot_commit(&self) -> bool {
        self.poisoned.load(Ordering::SeqCst) || self.unfinished.load(Ordering::SeqCst) != 0
    }
}
pub(crate) struct RuntimeOperationGuard {
    state: Arc<RuntimeTransactionState>,
    complete: bool,
}
impl RuntimeOperationGuard {
    pub(crate) fn complete(mut self) {
        self.complete = true;
    }
}
impl Drop for RuntimeOperationGuard {
    fn drop(&mut self) {
        if !self.complete {
            self.state.poisoned.store(true, Ordering::SeqCst);
        }
        self.state.unfinished.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dropped_and_forgotten_operations_never_allow_commit() {
        let state = Arc::new(RuntimeTransactionState::default());
        assert!(!state.cannot_commit());
        let guard = state.enter();
        assert!(state.cannot_commit());
        guard.complete();
        assert!(!state.cannot_commit());
        drop(state.enter());
        state.enter().complete();
        assert!(state.cannot_commit());
        let state = Arc::new(RuntimeTransactionState::default());
        std::mem::forget(state.enter());
        assert!(state.cannot_commit());
    }
}
