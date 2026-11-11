use crate::connection::transformer::Transformer;
use crate::connection::worker::Worker;
use crate::msg_handler::ReceiveOptions;
use crate::sync_helpers::*;
use maybe_async::*;
use smb_msg::{ResponseContent, Status};
use smb_transport::{IoVec, SmbTransport, SmbTransportWrite, TransportError};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

use crate::{
    Error,
    msg_handler::{IncomingMessage, OutgoingMessage, SendMessageResult},
};

use super::backend_trait::MultiWorkerBackend;

/// SMB2 base parallel transport worker (multi-threaded or async).
///
/// This struct is responsible for handling the transport to the server,
/// sending messages from SMB2 messages, and redirecting correct messages when received,
/// if using async, to the correct pending task.
/// One-per transport connection, hence takes ownership of the [SmbTransport] on [ParallelWorker::start].
pub struct ParallelWorker<BackendImplT>
where
    BackendImplT: MultiWorkerBackend + std::fmt::Debug,
    BackendImplT::AwaitingNotifier: std::fmt::Debug,
{
    /// The state of the worker, regarding messages to be received.
    /// See [`WorkerAwaitState`] for more details.
    pub(crate) state: Mutex<WorkerAwaitState<BackendImplT>>,

    /// The backend implementation of the worker -
    /// multi-threaded or async, depending on the crate configuration.
    backend_impl: Mutex<Option<Arc<BackendImplT>>>,

    transformer: Transformer,

    /// A channel that is being used to pass on messages that are being received from the server with
    /// no associated message ID (message id -1 - oplock break/server to client notification).
    notify_messages_channel: OnceCell<mpsc::Sender<IncomingMessage>>,

    /// The channel that is being used to pass on messages that are being sent to the server,
    /// from user threads to the worker send thread.
    pub(crate) sender: mpsc::Sender<BackendImplT::SendMessage>,

    /// A flag that indicates whether the worker is stopped.
    stopped: AtomicBool,
    /// The current timeout configured for the worker.
    timeout: AtomicU64,
}

/// Local patch (graphql-orm-storage): the largest number of request message
/// IDs tracked as awaiting a response. Legitimate in-flight requests are
/// bounded by the server's credit grant; common servers grant at most 8192.
/// When exceeded, the oldest (lowest) ID is forgotten, and its late response is
/// discarded like any other unsolicited message.
pub(crate) const MAX_OUTSTANDING_REQUESTS: usize = 8192;

/// Local patch (graphql-orm-storage): the largest number of responses held
/// before a task receives them. A response is normally received immediately
/// after its request is sent; when exceeded, the oldest held response is
/// dropped, so a server cannot grow this buffer without bound.
pub(crate) const MAX_PENDING_RESPONSES: usize = 256;

/// Holds state for the worker, regarding messages to be received:
/// - outstanding: message IDs of requests sent and not yet answered.
/// - awaiting: tasks that are waiting for a specific message ID.
/// - pending: messages that are waiting to be receive()-d.
#[derive(Debug)]
pub struct WorkerAwaitState<T>
where
    T: MultiWorkerBackend,
    T::AwaitingNotifier: std::fmt::Debug,
{
    /// Local patch (graphql-orm-storage): message IDs this client sent and
    /// still expects a response for. A response for any other ID is dropped.
    pub outstanding: BTreeSet<u64>,
    /// Stores the awaiting tasks that are waiting for a specific message ID.
    pub awaiting: HashMap<u64, T::AwaitingNotifier>,
    /// Stores the pending messages, waiting to be receive()-d. Bounded by
    /// [`MAX_PENDING_RESPONSES`]; message IDs increase, so the first entry is
    /// the oldest.
    pub pending: BTreeMap<u64, crate::Result<IncomingMessage>>,
}

impl<T> WorkerAwaitState<T>
where
    T: MultiWorkerBackend,
    T::AwaitingNotifier: std::fmt::Debug,
{
    fn new() -> Self {
        Self {
            outstanding: BTreeSet::new(),
            awaiting: HashMap::new(),
            pending: BTreeMap::new(),
        }
    }

    /// Local patch (graphql-orm-storage): records a sent request's message ID.
    pub(crate) fn expect_response(&mut self, msg_id: u64) {
        self.outstanding.insert(msg_id);
        while self.outstanding.len() > MAX_OUTSTANDING_REQUESTS {
            self.outstanding.pop_first();
        }
    }

    /// Local patch (graphql-orm-storage): accepts a response only for a
    /// message ID this client sent. An interim (STATUS_PENDING) response keeps
    /// the ID outstanding for its final response; any other response
    /// completes it.
    fn accept_response(&mut self, msg_id: u64, msg: &crate::Result<IncomingMessage>) -> bool {
        if !self.outstanding.contains(&msg_id) {
            return false;
        }
        let interim = matches!(
            msg,
            Ok(m) if m.message.header.flags.async_command()
                && m.message.header.status == Status::Pending as u32
        );
        if !interim {
            self.outstanding.remove(&msg_id);
        }
        true
    }

    /// Local patch (graphql-orm-storage): holds a response until it is
    /// received, dropping the oldest held response beyond the bound.
    fn hold_response(&mut self, msg_id: u64, msg: crate::Result<IncomingMessage>) {
        self.pending.insert(msg_id, msg);
        while self.pending.len() > MAX_PENDING_RESPONSES {
            if let Some((dropped, _)) = self.pending.pop_first() {
                log::warn!("Dropping unreceived response for message ID {dropped}.");
            }
        }
    }
}

#[maybe_async(AFIT)]
impl<T> ParallelWorker<T>
where
    T: MultiWorkerBackend + std::fmt::Debug,
    T::AwaitingNotifier: std::fmt::Debug,
{
    /// Returns whether the worker is stopped.
    pub fn stopped(&self) -> bool {
        self.stopped.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Local patch (graphql-orm-storage): starts the worker with `expected`
    /// message IDs already outstanding, before the receive loop reads. Tests
    /// use this to script responses to requests they did not send.
    pub(crate) async fn start_expecting(
        transport: Box<dyn SmbTransport>,
        timeout: Duration,
        expected: &[u64],
    ) -> crate::Result<Arc<Self>> {
        // Build the worker
        let (tx, rx) = T::make_send_channel_pair();
        let mut state = WorkerAwaitState::new();
        for msg_id in expected {
            state.expect_response(*msg_id);
        }
        let worker = Arc::new(ParallelWorker::<T> {
            state: Mutex::new(state),
            backend_impl: Default::default(),
            transformer: Transformer::default(),
            notify_messages_channel: Default::default(),
            sender: tx,
            stopped: AtomicBool::new(false),
            timeout: AtomicU64::new(u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)),
        });

        worker
            .backend_impl
            .lock()
            .await?
            .replace(T::start(transport, worker.clone(), rx).await?);

        Ok(worker)
    }

    /// Local patch (graphql-orm-storage): marks the worker stopped after its
    /// receive loop ended, and fails every awaiting task, so later receives
    /// fail immediately instead of waiting for a response that cannot arrive.
    pub(crate) async fn receive_loop_ended(&self) {
        if let Ok(mut state) = self.state.lock().await {
            self.stopped.store(true, Ordering::SeqCst);
            for (_, tx) in state.awaiting.drain() {
                let _notify_result = T::send_notify(tx, Err(Error::ConnectionStopped));
            }
        }
    }

    /// This is a function that should be used by multi worker implementations (async/mtd),
    /// after gettting a messages from the server, this function processes it and
    /// notifies the awaiting tasks.
    pub(crate) async fn incoming_data_callback(
        self: &Arc<Self>,
        message: Result<Vec<u8>, TransportError>,
    ) -> crate::Result<()> {
        log::trace!("Received message from server.");
        let message = message?;

        // Tranform the message and verify it.
        let msg = self.transformer.transform_incoming(message).await;
        let (msg, msg_id) = match msg {
            // Good flow, message is OK.
            Ok(msg) => {
                let msg_id = msg.message.header.message_id;
                (Ok(msg), msg_id)
            }
            // If we have a message ID to notify the error, use it.
            Err(crate::Error::TranformFailed(e)) => match e.msg_id {
                Some(msg_id) => (Err(crate::Error::TranformFailed(e)), msg_id),
                None => return Err(Error::TranformFailed(e)),
            },
            Err(e) => {
                log::error!("Failed to transform message: {e:?}");
                return Err(e);
            }
        };

        // Message ID (-1) is used and valid for notifications -
        // OPLOCK_BREAK or SERVER_TO_CLIENT_NOTIFICATION only.
        if msg_id == u64::MAX {
            // Nothing's waiting, so if there was an error, return it --
            // no need to notify anyone else.
            let msg = msg?;

            // Server-to-client commands check.
            // allow only oplock break and server to client notification.
            if !matches!(
                msg.message.content,
                ResponseContent::OplockBreakNotify(_)
                    | ResponseContent::ServerToClientNotification(_)
            ) {
                return Err(Error::MessageProcessingError(
                    "Received notification message, but not an OPLOCK_BREAK or SERVER_TO_CLIENT_NOTIFICATION.".to_string(),
                ));
            }

            if let Some(s2c_channel) = self.notify_messages_channel.get() {
                log::trace!("Sending notification message to notify channel.");
                s2c_channel.send(msg).await.map_err(|_| {
                    Error::MessageProcessingError(
                        "Failed to send notification message to notify channel.".to_string(),
                    )
                })?;
            } else {
                log::warn!("Received notification message, but no notify channel is set.");
            }
            return Ok(());
        }

        // Update the state: If awaited, wake up the task. Else, store it.
        let mut state = self.state.lock().await?;
        // Local patch (graphql-orm-storage): drop a response for a message ID
        // this client did not send, or that was already answered.
        if !state.accept_response(msg_id, &msg) {
            log::warn!("Discarding unsolicited response for message ID {msg_id}.");
            return Ok(());
        }
        let message_waiter = state.awaiting.remove(&msg_id);
        match message_waiter {
            Some(tx) => {
                log::trace!("Waking up awaiting task for key {msg_id}.");
                // Local patch (graphql-orm-storage): a task that stopped
                // waiting (for example, timed out) is not a receive error.
                if T::send_notify(tx, msg).is_err() {
                    log::debug!(
                        "Task awaiting message ID {msg_id} is gone; dropping its response."
                    );
                }
            }
            None => {
                log::trace!("Storing message until awaited: {msg_id}.",);
                state.hold_response(msg_id, msg);
            }
        }
        Ok(())
    }

    /// This function is used to set the notify channel for the worker.
    pub fn start_notify_channel(
        self: &Arc<Self>,
        notify_channel: mpsc::Sender<IncomingMessage>,
    ) -> crate::Result<()> {
        self.notify_messages_channel
            .set(notify_channel)
            .map_err(|_| Error::InvalidState("Notify channel is already set.".to_string()))?;
        Ok(())
    }

    /// This is a function that should be used by multi worker implementations (async/mtd),
    /// to send a message to the server.
    pub async fn outgoing_data_callback(
        self: &Arc<Self>,
        message: Option<IoVec>,
        wtransport: &mut dyn SmbTransportWrite,
    ) -> crate::Result<()> {
        let message = match message {
            Some(m) => m,
            None => {
                if self.stopped() {
                    return Err(Error::ConnectionStopped);
                } else {
                    return Err(Error::MessageProcessingError(
                        "Empty message cannot be sent to the server.".to_string(),
                    ));
                }
            }
        };
        wtransport.send(&message).await?;

        Ok(())
    }
}

#[maybe_async(AFIT)]
impl<T> Worker for ParallelWorker<T>
where
    T: MultiWorkerBackend + std::fmt::Debug,
    T::AwaitingNotifier: std::fmt::Debug,
{
    async fn start(
        transport: Box<dyn SmbTransport>,
        timeout: Duration,
    ) -> crate::Result<Arc<Self>> {
        Self::start_expecting(transport, timeout, &[]).await
    }

    async fn stop(&self) -> crate::Result<()> {
        self.stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
        {
            self.backend_impl
                .lock()
                .await?
                .take()
                .ok_or(Error::InvalidState(
                    "No backend present for worker.".to_string(),
                ))?
        }
        .stop()
        .await
    }

    async fn send(&self, msg: OutgoingMessage) -> crate::Result<SendMessageResult> {
        log::trace!("ParallelWorker::send({msg:?}) called");
        let return_raw_data = msg.return_raw_data;

        let id = msg.message.header.message_id;
        let message = { self.transformer.transform_outgoing(msg).await? };

        log::trace!("Message with ID {id} is passed to the worker for sending",);

        let raw_message_copy = if return_raw_data {
            Some(message.clone())
        } else {
            None
        };

        // Local patch (graphql-orm-storage): register the ID before the request
        // can reach the server, so its response is never mistaken as unsolicited.
        self.state.lock().await?.expect_response(id);

        let message = T::wrap_msg_to_send(message);

        self.sender.send(message).await.map_err(|_| {
            Error::MessageProcessingError("Failed to send message to worker!".to_string())
        })?;

        Ok(SendMessageResult::new(id, raw_message_copy))
    }

    async fn receive_next(&self, options: &ReceiveOptions<'_>) -> crate::Result<IncomingMessage> {
        let wait_for_receive = {
            let mut state = self.state.lock().await?;
            if self.stopped() {
                log::trace!("Connection is closed, avoid receiving.");
                return Err(Error::ConnectionStopped);
            }
            if state.pending.contains_key(&options.msg_id) {
                log::trace!(
                    "Message with ID {} is already received, remove from pending.",
                    &options.msg_id
                );
                let data = state.pending.remove(&options.msg_id).ok_or_else(|| {
                    Error::InvalidState("Message ID not found in pending messages.".to_string())
                })?;
                return data;
            }

            log::trace!(
                "Message with ID {} is not received yet, insert channel and await.",
                options.msg_id
            );

            let (tx, rx) = T::make_notifier_awaiter_pair();
            state.awaiting.insert(options.msg_id, tx);
            rx
        };

        let timeout = options
            .timeout
            .unwrap_or_else(|| Duration::from_millis(self.timeout.load(Ordering::Relaxed)));
        let result = T::wait_on_waiter(wait_for_receive, timeout).await?;

        log::trace!("Received message {result:?}");

        Ok(result)
    }

    fn transformer(&self) -> &Transformer {
        &self.transformer
    }
}

impl<T> std::fmt::Debug for ParallelWorker<T>
where
    T: MultiWorkerBackend + std::fmt::Debug,
    T::AwaitingNotifier: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParallelWorker")
            .field("state", &self.state)
            .field("backend", &self.backend_impl)
            .field("sender", &self.sender)
            .field("stopped", &self.stopped)
            .finish()
    }
}

/// Local patch (graphql-orm-storage): regression tests bounding the resources
/// a server can make the worker hold, and the receive loop's error handling.
#[cfg(all(test, feature = "async"))]
mod tests {
    use super::super::ParallelWorker;
    use super::*;
    use binrw::BinWrite;
    use futures_core::future::BoxFuture;
    use futures_util::FutureExt;
    use smb_msg::{Command, EchoResponse, Header, HeaderFlags};
    use smb_transport::SmbTransportRead;
    use std::io::Cursor;
    use std::net::SocketAddr;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Notify;

    /// Serves scripted server messages, signals when they are exhausted, then
    /// behaves like an idle peer.
    struct ScriptedTransport {
        incoming: Vec<u8>,
        position: usize,
        exhausted: Arc<Notify>,
    }

    /// Fails every read immediately with a connection reset, counting reads.
    struct ResetTransport {
        reads: Arc<AtomicUsize>,
    }

    struct DiscardWrite;

    fn split_with_discard(
        read: Box<dyn SmbTransportRead>,
    ) -> smb_transport::error::Result<(Box<dyn SmbTransportRead>, Box<dyn SmbTransportWrite>)> {
        Ok((read, Box::new(DiscardWrite)))
    }

    macro_rules! test_transport {
        ($name:ty) => {
            impl SmbTransport for $name {
                fn connect<'a>(
                    &'a mut self,
                    _server_name: &'a str,
                    _address: SocketAddr,
                ) -> BoxFuture<'a, smb_transport::error::Result<()>> {
                    async { Ok(()) }.boxed()
                }

                fn default_port(&self) -> u16 {
                    445
                }

                fn split(
                    self: Box<Self>,
                ) -> smb_transport::error::Result<(
                    Box<dyn SmbTransportRead>,
                    Box<dyn SmbTransportWrite>,
                )> {
                    split_with_discard(self)
                }

                fn remote_address(&self) -> smb_transport::error::Result<SocketAddr> {
                    Ok(SocketAddr::from(([127, 0, 0, 1], 445)))
                }
            }

            impl SmbTransportWrite for $name {
                fn send_raw<'a>(
                    &'a mut self,
                    _buf: &'a [u8],
                ) -> BoxFuture<'a, smb_transport::error::Result<()>> {
                    async { Ok(()) }.boxed()
                }
            }
        };
    }
    test_transport!(ScriptedTransport);
    test_transport!(ResetTransport);

    impl SmbTransportRead for ScriptedTransport {
        fn receive_exact<'a>(
            &'a mut self,
            out_buf: &'a mut [u8],
        ) -> BoxFuture<'a, smb_transport::error::Result<()>> {
            async move {
                let end = self.position + out_buf.len();
                if end > self.incoming.len() {
                    self.exhausted.notify_one();
                    std::future::pending::<()>().await;
                    return Err(TransportError::NotConnected);
                }
                out_buf.copy_from_slice(&self.incoming[self.position..end]);
                self.position = end;
                Ok(())
            }
            .boxed()
        }
    }

    impl SmbTransportRead for ResetTransport {
        fn receive_exact<'a>(
            &'a mut self,
            _out_buf: &'a mut [u8],
        ) -> BoxFuture<'a, smb_transport::error::Result<()>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            async {
                // Yield like a ready socket would, so a retrying loop is
                // observable on the single-threaded test runtime.
                tokio::task::yield_now().await;
                Err(TransportError::IoError(std::io::Error::from(
                    std::io::ErrorKind::ConnectionReset,
                )))
            }
            .boxed()
        }
    }

    impl SmbTransportWrite for DiscardWrite {
        fn send_raw<'a>(
            &'a mut self,
            _buf: &'a [u8],
        ) -> BoxFuture<'a, smb_transport::error::Result<()>> {
            async { Ok(()) }.boxed()
        }
    }

    /// An unsigned, sessionless ECHO response for `message_id`, as any party
    /// that can reach the client's socket could send before authentication.
    fn response(message_id: u64) -> Vec<u8> {
        let header = Header {
            credit_charge: 1,
            status: Status::Success as u32,
            command: Command::Echo,
            credit_request: 1,
            flags: HeaderFlags::new().with_server_to_redir(true),
            next_command: 0,
            message_id,
            tree_id: Some(0),
            async_id: None,
            session_id: 0,
            signature: 0,
        };
        let mut message = Cursor::new(Vec::new());
        header.write_le(&mut message).unwrap();
        EchoResponse::default().write_le(&mut message).unwrap();
        let message = message.into_inner();
        let mut framed = Vec::with_capacity(message.len() + 4);
        framed.extend_from_slice(&(message.len() as u32).to_be_bytes());
        framed.extend_from_slice(&message);
        framed
    }

    /// Starts a worker that reads `incoming` with `expected` IDs outstanding,
    /// and returns once every scripted message has been processed.
    async fn worker_after_script(incoming: Vec<u8>, expected: &[u64]) -> Arc<ParallelWorker> {
        let exhausted = Arc::new(Notify::new());
        let transport = ScriptedTransport {
            incoming,
            position: 0,
            exhausted: exhausted.clone(),
        };
        let worker =
            ParallelWorker::start_expecting(Box::new(transport), Duration::from_secs(5), expected)
                .await
                .unwrap();
        tokio::time::timeout(Duration::from_secs(10), exhausted.notified())
            .await
            .expect("the scripted messages must be read");
        worker
    }

    #[tokio::test]
    async fn unsolicited_response_flood_is_not_held() {
        let incoming: Vec<u8> = (100..1_100).flat_map(response).collect();
        let worker = worker_after_script(incoming, &[]).await;
        let state = worker.state.lock().await.unwrap();
        assert!(
            state.pending.is_empty(),
            "held {} responses to requests that were never sent",
            state.pending.len()
        );
    }

    #[tokio::test]
    async fn held_responses_to_sent_requests_are_bounded() {
        let count = MAX_PENDING_RESPONSES as u64 + 50;
        let expected: Vec<u64> = (1..=count).collect();
        let incoming: Vec<u8> = expected.iter().copied().flat_map(response).collect();
        let worker = worker_after_script(incoming, &expected).await;
        let state = worker.state.lock().await.unwrap();
        assert_eq!(state.pending.len(), MAX_PENDING_RESPONSES);
        // The oldest responses are the ones dropped.
        assert_eq!(state.pending.keys().next().copied(), Some(51));
    }

    #[tokio::test]
    async fn a_request_is_answered_once() {
        let incoming: Vec<u8> = [7, 7, 7].into_iter().flat_map(response).collect();
        let worker = worker_after_script(incoming, &[7]).await;
        {
            let state = worker.state.lock().await.unwrap();
            assert_eq!(state.pending.len(), 1);
            assert!(state.outstanding.is_empty());
        }
        let options = ReceiveOptions::new().with_msg_id_filter(7);
        worker.receive_next(&options).await.unwrap();
        let options = options.with_timeout(Duration::from_millis(50));
        assert!(matches!(
            worker.receive_next(&options).await,
            Err(Error::OperationTimeout(..))
        ));
    }

    #[::core::prelude::v1::test]
    fn outstanding_request_ids_are_bounded() {
        let mut state = WorkerAwaitState::<super::super::async_backend::AsyncBackend>::new();
        for msg_id in 0..(MAX_OUTSTANDING_REQUESTS as u64 + 10) {
            state.expect_response(msg_id);
        }
        assert_eq!(state.outstanding.len(), MAX_OUTSTANDING_REQUESTS);
        assert_eq!(state.outstanding.first().copied(), Some(10));
    }

    #[tokio::test]
    async fn receive_loop_stops_after_a_transport_failure() {
        let reads = Arc::new(AtomicUsize::new(0));
        let transport = ResetTransport {
            reads: reads.clone(),
        };
        let worker =
            ParallelWorker::start_expecting(Box::new(transport), Duration::from_secs(5), &[1])
                .await
                .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let observed = reads.load(Ordering::SeqCst);
        assert_eq!(
            observed, 1,
            "the receive loop retried a failed transport {observed} times"
        );
        // Later receives fail immediately rather than waiting for a response.
        let options = ReceiveOptions::new().with_msg_id_filter(1);
        let result = tokio::time::timeout(Duration::from_secs(1), worker.receive_next(&options))
            .await
            .expect("a receive on a failed connection must not wait");
        assert!(
            matches!(result, Err(Error::ConnectionStopped)),
            "{result:?}"
        );
    }
}
