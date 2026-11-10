use std::{sync::Arc, time::Duration};

use crate::{
    Error, connection::connection_info::ConnectionInfo, msg_handler::ReceiveOptions,
    session::SessionAndChannel, sync_helpers::*,
};
use smb_transport::SmbTransport;

use maybe_async::*;
use smb_msg::Status;

use crate::{
    connection::transformer::Transformer,
    msg_handler::{IncomingMessage, OutgoingMessage, SendMessageResult},
};

/// SMB2 connection worker.
///
/// Each Implementation of this trait is responsible for handling the connection to the server,
/// sending messages, and redirecting correct messages when received,
/// if using async, to the correct pending task.
#[maybe_async(AFIT)]
#[allow(async_fn_in_trait)]
pub trait Worker: Sized + std::fmt::Debug {
    /// Instantiates a new connection worker.
    async fn start(transport: Box<dyn SmbTransport>, timeout: Duration)
    -> crate::Result<Arc<Self>>;
    /// Stops the worker, shutting down the connection.
    async fn stop(&self) -> crate::Result<()>;

    async fn send(&self, msg: OutgoingMessage) -> crate::Result<SendMessageResult>;

    /// (Internal)
    ///
    /// This function is implemented to receive a single message from the server,
    /// with the specified filters.
    /// Use [`Worker::receive`] instead, if you're a user of this trait, and not an implementor.
    /// # Arguments
    /// * `msg_id` - The message ID to receive. This function will not return until the message id specified
    /// is received.
    /// # Returns
    /// * The message received from the server, matching the filters.
    async fn receive_next(&self, options: &ReceiveOptions<'_>) -> crate::Result<IncomingMessage>;

    #[cfg(feature = "async")]
    async fn receive_next_cancellable(
        &self,
        options: &ReceiveOptions<'_>,
    ) -> crate::Result<IncomingMessage> {
        if options.async_cancel.is_none() {
            return self.receive_next(options).await;
        }
        let recv_fut = self.receive_next(options);
        tokio::select! {
            biased;
            _ = options.async_cancel.as_ref().unwrap().cancelled() => {
                Err(Error::Cancelled("receive_next"))
            }
            res = recv_fut => {
                res
            }
        }
    }

    #[cfg(not(feature = "async"))]
    async fn receive_next_cancellable(
        &self,
        options: &ReceiveOptions<'_>,
    ) -> crate::Result<IncomingMessage> {
        // There's no actual async cancellation, so we do our best effort.
        // If the request is already running, cancellation must be performed by sending a
        // cancel message to the server.
        if options
            .async_cancel
            .as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::SeqCst))
        {
            return Err(Error::Cancelled("receive_next"));
        }

        self.receive_next(options).await
    }

    /// Receive a message from the server.
    /// This is a user function that will wait for the message to be received.
    async fn receive(&self, options: &ReceiveOptions<'_>) -> crate::Result<IncomingMessage> {
        if options.msg_id == u64::MAX {
            return Err(Error::InvalidArgument(
                "Message ID -1 is not valid for receive()".to_string(),
            ));
        }

        // Interim (STATUS_PENDING) responses are exempt from signing (MS-SMB2
        // 3.2.5.1.3) and are consumed here, before any session verification.
        // An interim is therefore untrusted: it may only extend the wait. It
        // never completes the request, and it never causes the final response
        // to be rejected (GHSA-439h-2cx8-xwgx).
        let mut waiting_async = false;
        loop {
            let msg = if waiting_async {
                self.receive_next_cancellable(options).await?
            } else {
                self.receive_next(options).await?
            };

            // Not async -- return the result. A synchronous final response also
            // completes a request after an (unverifiable) interim.
            if !msg.message.header.flags.async_command() {
                return Ok(msg);
            }

            // Handle async.
            if !options.allow_async {
                return Err(Error::InvalidArgument(
                    "Async command is not allowed in this context.".to_string(),
                ));
            }

            // If not pending, that's the result, right away! The session handler
            // verifies it before its status is evaluated.
            if msg.message.header.status != Status::Pending as u32 {
                return Ok(msg);
            }

            // An encryption-required session never accepts an unencrypted
            // message, so discard such an interim and keep waiting.
            if !msg.form.encrypted
                && self
                    .transformer()
                    .requires_encryption(msg.message.header.session_id)
                    .await
            {
                log::warn!(
                    "Discarding unencrypted interim response for message ID {} on an encryption-required session.",
                    msg.message.header.message_id
                );
                continue;
            }

            log::debug!(
                "Received async pending message with ID {} and status {}.",
                msg.message.header.message_id,
                msg.message.header.status
            );

            let async_id = match msg.message.header.async_id {
                Some(async_id) => async_id,
                None => {
                    panic!("Async ID is None, but async command is set. This should not happen.")
                }
            };

            // The most recent interim names the async ID used for cancellation.
            // A later interim or final response may carry another async ID: an
            // earlier interim may have been forged, so it is not binding.
            if async_id != 0 {
                if let Some(async_msg_ids) = &options.async_msg_ids {
                    async_msg_ids.set(options.msg_id, async_id);
                }
            }
            waiting_async = true;
        }
    }

    /// Get the transformer for this worker.
    fn transformer(&self) -> &Transformer;

    #[maybe_async]
    async fn negotaite_complete(&self, neg: &ConnectionInfo) {
        self.transformer().negotiated(neg).await.unwrap();
    }

    #[maybe_async]
    async fn session_started(&self, info: &Arc<RwLock<SessionAndChannel>>) -> crate::Result<()> {
        self.transformer().session_started(info).await
    }

    #[maybe_async]
    async fn session_ended(&self, info: &Arc<RwLock<SessionAndChannel>>) -> crate::Result<()> {
        self.transformer().session_ended(info).await
    }
}
