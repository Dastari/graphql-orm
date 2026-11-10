use super::*;

pub(crate) type ChannelUpstream = HandlerReference<ConnectionMessageHandler>;

pub struct Channel {
    channel_id: u32,

    pub(crate) handler: HandlerReference<ChannelMessageHandler>,
    pub(crate) conn_info: Arc<ConnectionInfo>,
}

impl Channel {
    #[maybe_async]
    pub(crate) async fn new(
        upstream: &ChannelUpstream,
        conn_info: &Arc<ConnectionInfo>,
        setup_result: &Arc<RwLock<SessionAndChannel>>,
    ) -> crate::Result<Self> {
        let (session_id, channel_id) = {
            let setup_result = setup_result.read().await?;
            let session = setup_result.session.read().await?;
            let channel = setup_result
                .channel
                .as_ref()
                .ok_or_else(|| Error::InvalidState("Channel not set in setup result".into()))?;
            (session.id(), channel.id())
        };
        let handler = ChannelMessageHandler::new(session_id, channel_id, upstream, setup_result);
        Ok(Self {
            channel_id,
            handler,
            conn_info: conn_info.clone(),
        })
    }

    /// Returns the Session ID of this session.
    ///
    /// This ID is the same as the SMB's session id,
    /// so it is unique-per-connection, and may be seen on the wire as well.
    #[inline]
    pub fn session_id(&self) -> u64 {
        self.handler.session_id()
    }

    #[inline]
    pub fn channel_id(&self) -> u32 {
        self.channel_id
    }
}

/// Message handler a specific channel.
///
/// This only makes sense, since sessions are not actually able to send data
/// as "themselves", but rather, through a channel.
pub struct ChannelMessageHandler {
    session_id: u64,
    channel_id: u32,
    upstream: ChannelUpstream,

    session_state: Arc<RwLock<SessionAndChannel>>,
}

#[maybe_async(AFIT)]
impl ChannelMessageHandler {
    fn new(
        session_id: u64,
        channel_id: u32,
        upstream: &ChannelUpstream,
        setup_result: &Arc<RwLock<SessionAndChannel>>,
    ) -> HandlerReference<ChannelMessageHandler> {
        HandlerReference::new(ChannelMessageHandler {
            session_id,
            channel_id,
            upstream: upstream.clone(),
            session_state: setup_result.clone(),
        })
    }

    pub(crate) async fn make_for_setup(
        setup_result: &Arc<RwLock<SessionAndChannel>>,
        upstream: &ChannelUpstream,
    ) -> crate::Result<Self> {
        let session_id = setup_result.read().await?.session.read().await?.id();
        Ok(Self {
            session_id,
            channel_id: u32::MAX,
            upstream: upstream.clone(),
            session_state: setup_result.clone(),
        })
    }

    /// (Internal)
    ///
    /// Verifies an [`IncomingMessage`] for the current session.
    /// This is trustworthy only since we trust the [`Transformer`][crate::connection::transformer::Transformer] implementation
    /// to provide the correct IDs and verify signatures and encryption.
    ///
    /// This must run before the status of a response is evaluated: a status
    /// taken from an unverified message must never be acted on
    /// (GHSA-439h-2cx8-xwgx).
    ///
    /// # Arguments
    /// * `incoming` - The incoming message to verify.
    /// * `expected_status` - For a response to a request, the statuses the caller
    ///   accepts. `None` (notifications) permits no unsigned exemption.
    /// # Returns
    /// An empty [`crate::Result`] if the message is valid, or an error if the message is invalid.
    #[maybe_async]
    async fn _verify_incoming(
        &self,
        incoming: &IncomingMessage,
        expected_status: Option<&[Status]>,
    ) -> crate::Result<()> {
        // allow unsigned messages only if the session is anonymous or guest.
        // this is enforced against configuration when setting up the session.
        let (unsigned_allowed, encryption_required, session_ready) = {
            let session = self.session_state.read().await?;
            let session = session.session.read().await?;
            let encryption_required = session.is_ready() && session.should_encrypt()?;
            (
                session.allow_unsigned()?,
                encryption_required,
                session.is_ready(),
            )
        };

        // Make sure that it's our session.
        if incoming.message.header.session_id == 0 {
            return Err(Error::InvalidMessage(
                "No session ID in message that got to session!".to_string(),
            ));
        }
        if incoming.message.header.session_id != self.session_id {
            return Err(Error::InvalidMessage(
                "Message not for this session!".to_string(),
            ));
        }
        // Make sure encryption is used when required.
        if !incoming.form.encrypted && encryption_required {
            return Err(Error::UnverifiedMessage(
                "Message not encrypted, but encryption is required for the session!".to_string(),
            ));
        }
        // and signed, unless allowed not to.
        if !incoming.form.signed_or_encrypted()
            && !unsigned_allowed
            && !expected_status.is_some_and(|expected| {
                Self::unsigned_response_permitted(incoming, session_ready, expected)
            })
        {
            return Err(Error::UnverifiedMessage(
                "Message not signed or encrypted, but signing is required for the session!"
                    .to_string(),
            ));
        }

        Ok(())
    }

    /// (Internal)
    ///
    /// The MS-SMB2 3.2.5.1.3 exemptions for an unsigned response while signing
    /// is required. Each one is permitted only for a status the caller does not
    /// accept, so the response can only ever fail the request; an unsigned
    /// status is never accepted as a result.
    /// * `STATUS_PENDING`: an interim response, which servers do not sign.
    /// * A `SESSION_SETUP` response before session setup completes, which can
    ///   only abort the setup.
    ///
    /// Responses with message ID `0xFFFFFFFFFFFFFFFF` are notifications, which
    /// are verified strictly in [`MessageHandler::notify`].
    fn unsigned_response_permitted(
        incoming: &IncomingMessage,
        session_ready: bool,
        expected_status: &[Status],
    ) -> bool {
        let header = &incoming.message.header;
        if expected_status.iter().any(|s| header.status == *s as u32) {
            return false;
        }
        header.status == Status::Pending as u32
            || (!session_ready && header.command == smb_msg::Command::SessionSetup)
    }

    /// **Insecure! Insecure! Insecure!**
    ///
    /// Same as [`ChannelMessageHandler::recvo`], but possible skips security validation.
    /// # Arguments
    /// * `options` - The options for receiving the message.
    /// * `skip_security_validation` - Whether to skip security validation of the incoming message.
    ///   This shall only be used when authentication is still being set up.
    /// # Returns
    /// An [`IncomingMessage`] if the message is valid, or an error if the message is invalid.
    #[maybe_async]
    pub(crate) async fn recvo_internal(
        &self,
        options: ReceiveOptions<'_>,
        skip_security_validation: bool,
    ) -> crate::Result<IncomingMessage> {
        let expected_status = options.status;
        let incoming = self.upstream.recvo_before_status(options).await?;

        if !skip_security_validation {
            self._verify_incoming(&incoming, Some(expected_status))
                .await?;
        } else {
            // Note: this is performed here for extra security,
            // while we could have just checked the session state, let's require
            // the caller to explicitly state that it is okay to skip security validation.
            let session = self.session_state.read().await?;
            let session = session.session.read().await?;
            assert!(
                session.is_initial(),
                "Incorrect internal state: security checks are never skipped, unless the session is still being set up!"
            );
        }

        ConnectionMessageHandler::check_expected_status(incoming, expected_status)
    }

    /// (Internal)
    ///
    /// Assures the sessions may not be used anymore.
    async fn _invalidate(&self) -> crate::Result<()> {
        self.upstream
            .worker()
            .ok_or_else(|| Error::InvalidState("Worker not available!".to_string()))?
            .session_ended(&self.session_state)
            .await
    }

    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    pub fn channel_id(&self) -> u32 {
        self.channel_id
    }

    pub fn session_state(&self) -> &Arc<RwLock<SessionAndChannel>> {
        &self.session_state
    }
}

#[maybe_async(AFIT)]
impl MessageHandler for ChannelMessageHandler {
    async fn sendo(&self, mut msg: OutgoingMessage) -> crate::Result<SendMessageResult> {
        {
            let session = self.session_state.read().await?;
            let session = session.session.read().await?;
            if session.is_invalid() {
                return Err(Error::InvalidState("Session is invalid".to_string()));
            }

            // It is possible for a lower level to request encryption.
            if msg.encrypt {
                // Session must be ready to encrypt messages.
                if !session.is_ready() {
                    return Err(Error::InvalidState(
                        "Session is not ready, cannot encrypt message".to_string(),
                    ));
                }
            }
            // Otherwise, we should check the session's configuration.
            else if session.is_ready() || session.is_setting_up() {
                // Encrypt if configured for the session,
                if session.is_ready() && session.should_encrypt()? {
                    msg.encrypt = true;
                }
                // Sign
                else if !session.allow_unsigned()? {
                    msg.message.header.flags.set_signed(true);
                }
            }
        }
        msg.message.header.session_id = self.session_id;
        self.upstream.sendo(msg).await
    }

    async fn recvo(&self, options: ReceiveOptions<'_>) -> crate::Result<IncomingMessage> {
        // Verify the signature or encryption before evaluating the status.
        let expected_status = options.status;
        let incoming = self.upstream.recvo_before_status(options).await?;

        self._verify_incoming(&incoming, Some(expected_status))
            .await?;

        ConnectionMessageHandler::check_expected_status(incoming, expected_status)
    }

    async fn notify(&self, msg: IncomingMessage) -> crate::Result<()> {
        self._verify_incoming(&msg, None).await?;

        match &msg.message.content {
            ResponseContent::ServerToClientNotification(s2c_notification) => {
                match s2c_notification.notification {
                    // TODO: Move this to primary session
                    Notification::NotifySessionClosed(_) => self._invalidate().await,
                }
            }
            _ => {
                log::warn!(
                    "Received unexpected message in session handler: {:?}",
                    msg.message.content
                );
                Ok(())
            }
        }
    }
}

/// Regression tests for GHSA-439h-2cx8-xwgx: an unauthenticated on-path party
/// must not be able to complete a pending request with an unsigned or
/// unencrypted error status once signing or encryption is required.
#[cfg(all(test, feature = "async"))]
mod tests {
    use super::*;
    use crate::connection::config::{ConnectionConfig, EncryptionMode};
    use crate::connection::connection_info::{ConnectionInfo, NegotiatedProperties};
    use crate::connection::worker::Worker;
    use crate::dialects::DialectImpl;
    use crate::msg_handler::AsyncMessageIds;
    use binrw::BinWrite;
    use futures_core::future::BoxFuture;
    use futures_util::FutureExt;
    use smb_dtyp::Guid;
    use smb_msg::{
        Command, Dialect, ErrorResponse, GlobalCapabilities, Header, HeaderFlags, SessionFlags,
    };
    use smb_transport::{SmbTransport, SmbTransportRead, SmbTransportWrite, TransportError};
    use std::io::Cursor;
    use std::net::SocketAddr;

    const SESSION_ID: u64 = 0x0000_4000_0000_0011;
    const MESSAGE_ID: u64 = 7;
    const SESSION_KEY: [u8; 16] = [0x5a; 16];

    /// Serves scripted server messages, then never yields again.
    struct ScriptedTransport {
        incoming: Vec<u8>,
        position: usize,
    }

    struct DiscardWrite;

    impl SmbTransport for ScriptedTransport {
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
        ) -> smb_transport::error::Result<(Box<dyn SmbTransportRead>, Box<dyn SmbTransportWrite>)>
        {
            Ok((self, Box::new(DiscardWrite)))
        }

        fn remote_address(&self) -> smb_transport::error::Result<SocketAddr> {
            Ok(SocketAddr::from(([127, 0, 0, 1], 445)))
        }
    }

    impl SmbTransportRead for ScriptedTransport {
        fn receive_exact<'a>(
            &'a mut self,
            out_buf: &'a mut [u8],
        ) -> BoxFuture<'a, smb_transport::error::Result<()>> {
            async move {
                let end = self.position + out_buf.len();
                if end > self.incoming.len() {
                    // Script exhausted: behave like an idle peer.
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

    impl SmbTransportWrite for DiscardWrite {
        fn send_raw<'a>(
            &'a mut self,
            _buf: &'a [u8],
        ) -> BoxFuture<'a, smb_transport::error::Result<()>> {
            async { Ok(()) }.boxed()
        }
    }

    impl SmbTransportWrite for ScriptedTransport {
        fn send_raw<'a>(
            &'a mut self,
            _buf: &'a [u8],
        ) -> BoxFuture<'a, smb_transport::error::Result<()>> {
            async { Ok(()) }.boxed()
        }
    }

    /// An unsigned, unencrypted plain response as an on-path party would forge it.
    fn forged_response(command: Command, status: Status) -> Vec<u8> {
        forged_response_with(command, status, None)
    }

    /// As [`forged_response`], optionally as an async response with `async_id`.
    fn forged_response_with(command: Command, status: Status, async_id: Option<u64>) -> Vec<u8> {
        let header = Header {
            credit_charge: 1,
            status: status as u32,
            command,
            credit_request: 1,
            flags: HeaderFlags::new()
                .with_server_to_redir(true)
                .with_async_command(async_id.is_some()),
            next_command: 0,
            message_id: MESSAGE_ID,
            tree_id: async_id.is_none().then_some(1),
            async_id,
            session_id: SESSION_ID,
            signature: 0,
        };
        let mut message = Cursor::new(Vec::new());
        header.write_le(&mut message).unwrap();
        ErrorResponse { error_data: vec![] }
            .write_le(&mut message)
            .unwrap();
        // ErrorData is one byte when ByteCount is zero.
        message.get_mut().push(0);
        let message = message.into_inner();

        let mut framed = Vec::with_capacity(message.len() + 4);
        framed.extend_from_slice(&(message.len() as u32).to_be_bytes());
        framed.extend_from_slice(&message);
        framed
    }

    fn connection_info(dialect: Dialect, encryption_mode: EncryptionMode) -> ConnectionInfo {
        ConnectionInfo {
            server_name: "server".to_string(),
            server_address: SocketAddr::from(([127, 0, 0, 1], 445)),
            negotiation: NegotiatedProperties {
                server_guid: Guid::generate(),
                caps: GlobalCapabilities::new(),
                max_transact_size: 65536,
                max_read_size: 65536,
                max_write_size: 65536,
                auth_buffer: vec![],
                signing_algo: None,
                encryption_cipher: None,
                compression: None,
                dialect_rev: dialect,
            },
            dialect: DialectImpl::new(dialect),
            config: ConnectionConfig {
                encryption_mode,
                allow_unsigned_guest_access: false,
                ..ConnectionConfig::default()
            },
            preauth_hash: crate::connection::preauth_hash::PreauthHashState::Unsupported,
            client_guid: Guid::generate(),
        }
    }

    enum Stage {
        SettingUp,
        Ready,
    }

    async fn channel_for(
        incoming: Vec<u8>,
        dialect: Dialect,
        encryption_mode: EncryptionMode,
        stage: Stage,
    ) -> HandlerReference<ChannelMessageHandler> {
        handlers_for(incoming, dialect, encryption_mode, stage)
            .await
            .0
    }

    /// A channel handler and its connection handler, with the session
    /// registered in the connection's transformer.
    async fn handlers_for(
        incoming: Vec<u8>,
        dialect: Dialect,
        encryption_mode: EncryptionMode,
        stage: Stage,
    ) -> (
        HandlerReference<ChannelMessageHandler>,
        HandlerReference<ConnectionMessageHandler>,
    ) {
        let info = connection_info(dialect, encryption_mode);
        let mut session = SessionInfo::new(SESSION_ID);
        session.setup(&SESSION_KEY, &None, &info).unwrap();
        if matches!(stage, Stage::Ready) {
            session.ready(SessionFlags::new(), &info).unwrap();
        }
        let state = Arc::new(RwLock::new(SessionAndChannel::new(
            SESSION_ID,
            Arc::new(RwLock::new(session)),
        )));
        let upstream = ConnectionMessageHandler::with_test_transport(Box::new(ScriptedTransport {
            incoming,
            position: 0,
        }))
        .await
        .unwrap();
        let worker = upstream.worker().unwrap();
        worker.negotaite_complete(&info).await;
        worker.session_started(&state).await.unwrap();
        (
            ChannelMessageHandler::new(SESSION_ID, 1, &upstream, &state),
            upstream,
        )
    }

    fn expect_success(msg_id: u64) -> ReceiveOptions<'static> {
        ReceiveOptions::new()
            .with_status(&[Status::Success])
            .with_msg_id_filter(msg_id)
    }

    fn assert_rejected_before_status(result: crate::Result<IncomingMessage>) {
        match result {
            Err(Error::UnverifiedMessage(_)) => {}
            Err(Error::ReceivedErrorMessage(status, _))
            | Err(Error::UnexpectedMessageStatus(status)) => {
                panic!("forged unsigned status {status:#x} was acted on before verification")
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[tokio::test]
    async fn unsigned_error_status_is_rejected_when_signing_is_required() {
        let channel = channel_for(
            forged_response(Command::Create, Status::ObjectNameNotFound),
            Dialect::Smb021,
            EncryptionMode::Allowed,
            Stage::Ready,
        )
        .await;
        assert_rejected_before_status(channel.recvo(expect_success(MESSAGE_ID)).await);
    }

    #[tokio::test]
    async fn unencrypted_error_status_is_rejected_when_encryption_is_required() {
        let channel = channel_for(
            forged_response(Command::Create, Status::ObjectNameNotFound),
            Dialect::Smb0302,
            EncryptionMode::Required,
            Stage::Ready,
        )
        .await;
        assert_rejected_before_status(channel.recvo(expect_success(MESSAGE_ID)).await);
    }

    #[tokio::test]
    async fn unsigned_success_is_rejected_when_signing_is_required() {
        let channel = channel_for(
            forged_response(Command::Create, Status::Success),
            Dialect::Smb021,
            EncryptionMode::Allowed,
            Stage::Ready,
        )
        .await;
        assert_rejected_before_status(channel.recvo(expect_success(MESSAGE_ID)).await);
    }

    #[tokio::test]
    async fn unsigned_interim_pending_is_never_accepted_as_a_result() {
        let channel = channel_for(
            forged_response(Command::Create, Status::Pending),
            Dialect::Smb021,
            EncryptionMode::Allowed,
            Stage::Ready,
        )
        .await;
        let result = channel.recvo(expect_success(MESSAGE_ID)).await;
        assert!(
            matches!(
                result,
                Err(Error::ReceivedErrorMessage(status, _) | Error::UnexpectedMessageStatus(status))
                    if status == Status::Pending as u32
            ),
            "unexpected result: {result:?}"
        );
    }

    #[tokio::test]
    async fn unsigned_session_setup_failure_is_reported_before_setup_completes() {
        let channel = channel_for(
            forged_response(Command::SessionSetup, Status::LogonFailure),
            Dialect::Smb021,
            EncryptionMode::Allowed,
            Stage::SettingUp,
        )
        .await;
        let result = channel
            .recvo_internal(expect_success(MESSAGE_ID), false)
            .await;
        assert!(
            matches!(
                result,
                Err(Error::ReceivedErrorMessage(status, _) | Error::UnexpectedMessageStatus(status))
                    if status == Status::LogonFailure as u32
            ),
            "unexpected result: {result:?}"
        );
    }

    #[tokio::test]
    async fn unsigned_session_setup_success_is_rejected_when_signing_is_required() {
        let channel = channel_for(
            forged_response(Command::SessionSetup, Status::Success),
            Dialect::Smb021,
            EncryptionMode::Allowed,
            Stage::SettingUp,
        )
        .await;
        assert_rejected_before_status(
            channel
                .recvo_internal(expect_success(MESSAGE_ID), false)
                .await,
        );
    }

    #[tokio::test]
    async fn unsigned_no_more_files_cannot_truncate_a_directory_listing() {
        // `Directory` ends its listing on STATUS_NO_MORE_FILES; a forged one
        // must be rejected, not reported as the end of the directory.
        let channel = channel_for(
            forged_response(Command::QueryDirectory, Status::NoMoreFiles),
            Dialect::Smb021,
            EncryptionMode::Allowed,
            Stage::Ready,
        )
        .await;
        assert_rejected_before_status(channel.recvo(expect_success(MESSAGE_ID)).await);
    }

    const FORGED_ASYNC_ID: u64 = 0x0bad;
    const SERVER_ASYNC_ID: u64 = 0x0600d;

    fn async_options(ids: &Arc<AsyncMessageIds>) -> ReceiveOptions<'static> {
        expect_success(MESSAGE_ID)
            .with_allow_async(true)
            .with_async_msg_ids(ids.clone())
    }

    async fn worker_receive(
        incoming: Vec<u8>,
        encryption_mode: EncryptionMode,
        ids: &Arc<AsyncMessageIds>,
    ) -> crate::Result<IncomingMessage> {
        let dialect = if encryption_mode.is_required() {
            Dialect::Smb0302
        } else {
            Dialect::Smb021
        };
        let (_channel, upstream) =
            handlers_for(incoming, dialect, encryption_mode, Stage::Ready).await;
        upstream
            .worker()
            .unwrap()
            .receive(&async_options(ids))
            .await
    }

    fn interim_then_final(interim_async_id: u64, final_async_id: Option<u64>) -> Vec<u8> {
        let mut incoming =
            forged_response_with(Command::Create, Status::Pending, Some(interim_async_id));
        incoming.extend(forged_response_with(
            Command::Create,
            Status::Success,
            final_async_id,
        ));
        incoming
    }

    fn assert_final(result: crate::Result<IncomingMessage>) {
        let msg = result.expect("the final response must be delivered");
        assert_eq!(msg.message.header.status, Status::Success as u32);
    }

    #[tokio::test]
    async fn forged_interim_does_not_reject_a_final_with_another_async_id() {
        let ids = Arc::new(AsyncMessageIds::default());
        let result = worker_receive(
            interim_then_final(FORGED_ASYNC_ID, Some(SERVER_ASYNC_ID)),
            EncryptionMode::Allowed,
            &ids,
        )
        .await;
        assert_final(result);
    }

    #[tokio::test]
    async fn forged_interim_does_not_reject_a_synchronous_final() {
        let ids = Arc::new(AsyncMessageIds::default());
        let result = worker_receive(
            interim_then_final(FORGED_ASYNC_ID, None),
            EncryptionMode::Allowed,
            &ids,
        )
        .await;
        assert_final(result);
    }

    #[tokio::test]
    async fn interim_with_zero_async_id_is_not_returned_as_a_result() {
        let ids = Arc::new(AsyncMessageIds::default());
        let result =
            worker_receive(interim_then_final(0, None), EncryptionMode::Allowed, &ids).await;
        assert_final(result);
    }

    #[tokio::test]
    async fn unsigned_interim_is_honoured_when_only_signing_is_required() {
        // MS-SMB2 3.2.5.1.3 exempts STATUS_PENDING from signing.
        let ids = Arc::new(AsyncMessageIds::default());
        let result = worker_receive(
            interim_then_final(SERVER_ASYNC_ID, Some(SERVER_ASYNC_ID)),
            EncryptionMode::Allowed,
            &ids,
        )
        .await;
        assert_final(result);
        assert_eq!(
            ids.async_id.load(std::sync::atomic::Ordering::SeqCst),
            SERVER_ASYNC_ID
        );
    }

    #[tokio::test]
    async fn unencrypted_interim_is_discarded_when_encryption_is_required() {
        let ids = Arc::new(AsyncMessageIds::default());
        let result = worker_receive(
            interim_then_final(FORGED_ASYNC_ID, None),
            EncryptionMode::Required,
            &ids,
        )
        .await;
        // The worker delivers the final; the session handler then verifies it.
        assert_final(result);
        assert_eq!(
            ids.async_id.load(std::sync::atomic::Ordering::SeqCst),
            u64::MAX,
            "an unencrypted interim must not be recorded as the request's async ID"
        );
    }
}
