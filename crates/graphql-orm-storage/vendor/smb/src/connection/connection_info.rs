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

/// Local patch (graphql-orm-storage): a deferred cleanup step, such as the
/// CLOSE of a handle dropped while still open.
#[cfg(feature = "async")]
pub(crate) type CleanupTask = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

/// Local patch (graphql-orm-storage): cleanup that destructors defer to the
/// connection's runtime.
///
/// A destructor may run on any thread, with no current Tokio runtime, for
/// example after `Runtime::block_on` has returned. It hands its async
/// cleanup to this queue, which needs no runtime to accept it. A task spawned
/// on the runtime that negotiated the connection, the one that runs the
/// connection's worker, drains the queue and runs each step there. If that
/// runtime has shut down, the step is refused and dropped; the connection's
/// socket closed with it, and the server releases what the session held.
#[cfg(feature = "async")]
#[derive(Clone)]
pub(crate) struct CleanupQueue(tokio::sync::mpsc::UnboundedSender<CleanupTask>);

#[cfg(feature = "async")]
impl CleanupQueue {
    /// A queue drained on the current runtime, or one that refuses every
    /// step when there is none.
    pub(crate) fn start() -> Self {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<CleanupTask>();
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    // Ends once every sender, and so the connection's state,
                    // is gone and the queue is empty.
                    while let Some(task) = receiver.recv().await {
                        tokio::spawn(task);
                    }
                });
            }
            Err(_) => drop(receiver),
        }
        Self(sender)
    }

    /// Hands `task` to the connection's runtime. Returns whether it was
    /// accepted; a refused task is dropped on this thread.
    pub(crate) fn defer(&self, task: CleanupTask) -> bool {
        self.0.send(task).is_ok()
    }
}

#[cfg(feature = "async")]
impl std::fmt::Debug for CleanupQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CleanupQueue")
            .field("open", &!self.0.is_closed())
            .finish()
    }
}

#[cfg(all(test, feature = "async"))]
mod cleanup_queue_tests {
    use super::CleanupQueue;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    fn flag_task(flag: &Arc<AtomicBool>) -> super::CleanupTask {
        let flag = flag.clone();
        Box::pin(async move { flag.store(true, Ordering::SeqCst) })
    }

    #[test]
    fn a_step_deferred_outside_any_runtime_runs_on_the_connections_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let queue = runtime.block_on(async { CleanupQueue::start() });
        let ran = Arc::new(AtomicBool::new(false));
        // No runtime is current here.
        assert!(tokio::runtime::Handle::try_current().is_err());
        assert!(queue.defer(flag_task(&ran)));
        runtime.block_on(async {
            for _ in 0..100 {
                if ran.load(Ordering::SeqCst) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        });
        assert!(ran.load(Ordering::SeqCst), "the deferred step ran");
    }

    #[test]
    fn a_step_deferred_after_the_runtime_shut_down_is_refused_without_panicking() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let queue = runtime.block_on(async { CleanupQueue::start() });
        drop(runtime);
        let ran = Arc::new(AtomicBool::new(false));
        assert!(!queue.defer(flag_task(&ran)));
        assert!(!ran.load(Ordering::SeqCst));
        // Without a runtime at all, the queue refuses every step.
        assert!(!CleanupQueue::start().defer(flag_task(&ran)));
    }
}
