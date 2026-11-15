use std::sync::Arc;

use crate::{connection::preauth_hash::PreauthHashState, dialects::DialectImpl};
use binrw::prelude::*;
use smb_dtyp::Guid;
use smb_msg::*;

use super::ConnectionConfig;

/// Contains important information from the negotiation process,
/// to be used during connection operations.
#[derive(Debug)]
pub struct NegotiatedProperties {
    /// From the server's negotiation response.
    pub server_guid: Guid,

    /// From the server's negotiation response.
    pub caps: GlobalCapabilities,

    /// From the server's negotiation response.
    pub max_transact_size: u32,
    /// From the server's negotiation response.
    pub max_read_size: u32,
    /// From the server's negotiation response.
    pub max_write_size: u32,

    /// From the server's negotiation response.
    pub auth_buffer: Vec<u8>,

    /// Signing algorithm used for the connection, and specified by the server
    /// using negotiation context. This is irrelevant for dialects below 3.1.1,
    /// and if not specified, this property is not set, but the connection may still be
    /// signed using the default algorithm, as specified in the spec.
    pub signing_algo: Option<SigningAlgorithmId>,
    /// Encryption cipher used for the connection, and specified by the server
    /// using negotiation context. This is irrelevant for dialects below 3.1.1,
    /// and if not specified, this property is not set, but the connection may still be
    /// encrypted using the default cipher, as specified in the spec.
    pub encryption_cipher: Option<EncryptionCipher>,
    /// Compression capabilities used for the connection, and specified by the server
    /// using negotiation context.
    pub compression: Option<CompressionCapabilities>,

    /// The selected dialect revision for the connection.
    /// Use [ConnectionInfo::dialect] to get the implementation of the selected dialect.
    pub dialect_rev: Dialect,
}

/// This struct is initalized once a connection is established and negotiated.
/// It contains all the information about the connection.
#[derive(Debug)]
pub struct ConnectionInfo {
    /// The server name used for the connection.
    pub server_name: String,
    /// The server address used for the connection.
    pub server_address: std::net::SocketAddr,

    /// Contains negotiated properties of the connection.
    pub negotiation: NegotiatedProperties,
    /// Contains the implementation of the selected dialect.
    pub dialect: Arc<DialectImpl>,
    /// Contains the configuration of the connection, as specified by the user when the connection was established.
    pub config: ConnectionConfig,
    /// Preauthentication hash state, if applicable.
    pub preauth_hash: PreauthHashState,
    /// The client GUID used for the connection.
    pub client_guid: Guid,
    /// Local patch (graphql-orm-storage): cleanup deferred by destructors,
    /// run on the connection's runtime.
    #[cfg(feature = "async")]
    pub(crate) cleanup: CleanupQueue,
}

/// Local patch (graphql-orm-storage): the future a cleanup step runs, such as
/// the CLOSE of a handle dropped while still open.
#[cfg(feature = "async")]
pub(crate) type CleanupTask = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

/// Local patch (graphql-orm-storage): a deferred cleanup step. It captures
/// only weak references and builds its future when it runs, returning `None`
/// when what it would clean up is already gone, so a queued step never keeps
/// a connection, session, tree or socket alive.
#[cfg(feature = "async")]
pub(crate) type CleanupStep = Box<dyn FnOnce() -> Option<CleanupTask> + Send>;

/// Local patch (graphql-orm-storage): pending cleanup steps a connection
/// holds at most. Further steps are refused and counted; the server releases
/// what they would have cleaned up when the session ends.
#[cfg(feature = "async")]
pub(crate) const CLEANUP_QUEUE_CAPACITY: usize = 256;

/// Local patch (graphql-orm-storage): what happened to a connection's
/// deferred cleanup steps.
#[cfg(feature = "async")]
#[derive(Debug, Default)]
pub(crate) struct CleanupStats {
    /// Refused because the queue was full.
    pub(crate) overflowed: std::sync::atomic::AtomicU64,
    /// Refused because the queue had shut down.
    pub(crate) refused: std::sync::atomic::AtomicU64,
    /// Ran to completion.
    pub(crate) completed: std::sync::atomic::AtomicU64,
    /// Skipped because what they would clean up was already gone.
    pub(crate) expired: std::sync::atomic::AtomicU64,
    /// Abandoned at their deadline.
    pub(crate) timed_out: std::sync::atomic::AtomicU64,
    /// Dropped, pending or running, when the queue shut down.
    pub(crate) abandoned: std::sync::atomic::AtomicU64,
    /// Whether the drain task has ended.
    pub(crate) finished: std::sync::atomic::AtomicBool,
}

/// Local patch (graphql-orm-storage): cleanup that destructors defer to the
/// connection's runtime.
///
/// A destructor may run on any thread, with no current Tokio runtime, for
/// example after `Runtime::block_on` has returned. It offers its step with a
/// non-blocking `try_send`, which needs no runtime. One task, spawned on the
/// runtime that negotiated the connection (the one that runs its worker),
/// runs the steps one at a time, each within a deadline.
///
/// The queue holds at most [`CLEANUP_QUEUE_CAPACITY`] steps; a step offered
/// to a full or shut-down queue is refused, counted and logged at debug level.
/// The drain task holds no sender and steps hold only weak references, so the
/// task ends when the last owner of the connection's state drops, or as soon
/// as the connection is closed or dropped ([`Self::shutdown`]), dropping any
/// pending steps. Whatever a refused, abandoned or timed-out step would have
/// closed, the server releases when the session ends.
#[cfg(feature = "async")]
#[derive(Clone)]
pub(crate) struct CleanupQueue {
    sender: tokio::sync::mpsc::Sender<CleanupStep>,
    shutdown: tokio_util::sync::CancellationToken,
    stats: Arc<CleanupStats>,
}

#[cfg(feature = "async")]
impl CleanupQueue {
    /// A queue drained on the current runtime, each step within `deadline`
    /// (the connection's default timeout when zero, which would mean
    /// forever), or one that refuses every step when there is no runtime.
    pub(crate) fn start(deadline: std::time::Duration) -> Self {
        Self::with_capacity(CLEANUP_QUEUE_CAPACITY, deadline)
    }

    fn with_capacity(capacity: usize, deadline: std::time::Duration) -> Self {
        let deadline = if deadline.is_zero() {
            ConnectionConfig::DEFAULT_TIMEOUT
        } else {
            deadline
        };
        let (sender, receiver) = tokio::sync::mpsc::channel::<CleanupStep>(capacity);
        let shutdown = tokio_util::sync::CancellationToken::new();
        let stats = Arc::new(CleanupStats::default());
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(Self::drain(
                    receiver,
                    shutdown.clone(),
                    stats.clone(),
                    deadline,
                ));
            }
            Err(_) => {
                drop(receiver);
                stats
                    .finished
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        Self {
            sender,
            shutdown,
            stats,
        }
    }

    async fn drain(
        mut receiver: tokio::sync::mpsc::Receiver<CleanupStep>,
        shutdown: tokio_util::sync::CancellationToken,
        stats: Arc<CleanupStats>,
        deadline: std::time::Duration,
    ) {
        use std::sync::atomic::Ordering;
        loop {
            let step = tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                step = receiver.recv() => match step {
                    Some(step) => step,
                    // Every owner of the connection's state is gone.
                    None => break,
                },
            };
            let Some(task) = step() else {
                stats.expired.fetch_add(1, Ordering::SeqCst);
                continue;
            };
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    stats.abandoned.fetch_add(1, Ordering::SeqCst);
                    break;
                }
                outcome = tokio::time::timeout(deadline, task) => match outcome {
                    Ok(()) => {
                        stats.completed.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(_) => {
                        stats.timed_out.fetch_add(1, Ordering::SeqCst);
                        log::debug!(
                            "Deferred cleanup step abandoned after {deadline:?}; the server releases it when the session ends."
                        );
                    }
                },
            }
        }
        receiver.close();
        let mut pending = 0;
        while receiver.try_recv().is_ok() {
            pending += 1;
        }
        if pending > 0 {
            stats.abandoned.fetch_add(pending, Ordering::SeqCst);
            log::debug!(
                "Connection cleanup ended with {pending} pending steps; the server releases them when the session ends."
            );
        }
        stats.finished.store(true, Ordering::SeqCst);
    }

    /// Offers `step` without blocking and without a runtime. Returns whether
    /// it was queued; a refused step is counted and dropped on this thread.
    pub(crate) fn defer(&self, step: CleanupStep) -> bool {
        use std::sync::atomic::Ordering;
        match self.sender.try_send(step) {
            Ok(()) => true,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                self.stats.overflowed.fetch_add(1, Ordering::SeqCst);
                log::debug!(
                    "Connection cleanup queue is full; the server releases the step's resource when the session ends."
                );
                false
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                self.stats.refused.fetch_add(1, Ordering::SeqCst);
                log::debug!(
                    "Connection cleanup has shut down; the server releases the step's resource when the session ends."
                );
                false
            }
        }
    }

    /// Ends the drain task and drops pending steps. Needs no runtime.
    pub(crate) fn shutdown(&self) {
        self.shutdown.cancel();
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> &CleanupStats {
        &self.stats
    }
}

#[cfg(feature = "async")]
impl std::fmt::Debug for CleanupQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CleanupQueue")
            .field("shut_down", &self.shutdown.is_cancelled())
            .field("stats", &self.stats)
            .finish()
    }
}

#[cfg(all(test, feature = "async"))]
mod cleanup_queue_tests {
    use super::{CleanupQueue, CleanupStep};
    use std::sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Duration;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn flag_step(flag: &Arc<AtomicBool>) -> CleanupStep {
        let flag = flag.clone();
        Box::new(move || Some(Box::pin(async move { flag.store(true, Ordering::SeqCst) })))
    }

    /// A step that, like a CLOSE waiting for credits that are never granted,
    /// holds its upgraded owner and never finishes.
    fn stuck_step(owner: Weak<()>) -> CleanupStep {
        Box::new(move || {
            let owner = owner.upgrade()?;
            Some(Box::pin(async move {
                let _owner = owner;
                std::future::pending::<()>().await
            }))
        })
    }

    async fn settle() {
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
    }

    #[test]
    fn a_step_deferred_outside_any_runtime_runs_on_the_connections_runtime() {
        let runtime = runtime();
        let queue = runtime.block_on(async { CleanupQueue::start(Duration::from_secs(5)) });
        let ran = Arc::new(AtomicBool::new(false));
        assert!(tokio::runtime::Handle::try_current().is_err());
        assert!(queue.defer(flag_step(&ran)));
        runtime.block_on(settle());
        assert!(ran.load(Ordering::SeqCst), "the deferred step ran");
        assert_eq!(queue.stats().completed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn steps_beyond_the_capacity_are_refused_and_counted_without_blocking() {
        let runtime = runtime();
        let queue =
            runtime.block_on(async { CleanupQueue::with_capacity(4, Duration::from_secs(5)) });
        // The drain task cannot run while the runtime is idle, so the queue fills.
        let ran = Arc::new(AtomicBool::new(false));
        let accepted = (0..10).filter(|_| queue.defer(flag_step(&ran))).count();
        assert_eq!(accepted, 4);
        assert_eq!(queue.stats().overflowed.load(Ordering::SeqCst), 6);
        runtime.block_on(settle());
        assert_eq!(queue.stats().completed.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn a_stuck_step_times_out_and_queued_steps_never_retain_their_owner() {
        let runtime = runtime();
        let queue = runtime.block_on(async { CleanupQueue::start(Duration::from_millis(100)) });
        let running = Arc::new(());
        let queued = Arc::new(());
        let (running_weak, queued_weak) = (Arc::downgrade(&running), Arc::downgrade(&queued));
        assert!(queue.defer(stuck_step(running_weak.clone())));
        assert!(queue.defer(stuck_step(queued_weak.clone())));
        // The first step holds its owner while it runs; the second waits
        // behind it holding only a weak reference.
        runtime.block_on(settle());
        drop(queued);
        assert!(
            queued_weak.upgrade().is_none(),
            "a queued step retained its owner"
        );
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(150)).await });
        assert_eq!(queue.stats().timed_out.load(Ordering::SeqCst), 1);
        // The stuck step was abandoned, so dropping the last owner frees it.
        drop(running);
        assert!(
            running_weak.upgrade().is_none(),
            "a timed-out step retained its owner"
        );
        runtime.block_on(settle());
        assert_eq!(queue.stats().expired.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn shutdown_ends_the_drain_task_and_drops_pending_steps() {
        let runtime = runtime();
        let queue = runtime.block_on(async { CleanupQueue::start(Duration::from_secs(5)) });
        let owner = Arc::new(());
        assert!(queue.defer(stuck_step(Arc::downgrade(&owner))));
        let ran = Arc::new(AtomicBool::new(false));
        assert!(queue.defer(flag_step(&ran)));
        runtime.block_on(settle());
        queue.shutdown();
        runtime.block_on(settle());
        let stats = queue.stats();
        assert!(
            stats.finished.load(Ordering::SeqCst),
            "the drain task ended"
        );
        assert_eq!(stats.abandoned.load(Ordering::SeqCst), 2);
        assert!(!ran.load(Ordering::SeqCst));
        assert_eq!(
            Arc::strong_count(&owner),
            1,
            "the abandoned step released its owner"
        );
        assert!(!queue.defer(flag_step(&ran)));
        assert_eq!(stats.refused.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_drain_task_ends_when_the_last_owner_drops() {
        let runtime = runtime();
        let queue = runtime.block_on(async { CleanupQueue::start(Duration::from_secs(5)) });
        let stats = queue.stats.clone();
        drop(queue);
        runtime.block_on(settle());
        assert!(stats.finished.load(Ordering::SeqCst));
    }

    #[test]
    fn a_step_deferred_after_the_runtime_shut_down_is_refused_without_panicking() {
        let runtime = runtime();
        let queue = runtime.block_on(async { CleanupQueue::start(Duration::from_secs(5)) });
        drop(runtime);
        let ran = Arc::new(AtomicBool::new(false));
        assert!(!queue.defer(flag_step(&ran)));
        assert!(!ran.load(Ordering::SeqCst));
        // Without a runtime at all, the queue refuses every step.
        assert!(!CleanupQueue::start(Duration::from_secs(5)).defer(flag_step(&ran)));
    }
}
