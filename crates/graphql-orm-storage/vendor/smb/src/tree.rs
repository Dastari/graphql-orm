use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use maybe_async::*;
use smb_msg::{FileId, FsctlRequest, IoctlRequest, IoctlRequestFlags};

use crate::FileCreateArgs;
use crate::connection::connection_info::ConnectionInfo;
use smb_fscc::{FileAccessMask, FileAttributes};
use smb_msg::{
    CreateOptions, RequestContent, ShareFlags, ShareType,
    create::CreateDisposition,
    tree_connect::{TreeConnectRequest, TreeDisconnectRequest},
};

use crate::{
    Error, Resource,
    msg_handler::{HandlerReference, MessageHandler},
    session::SessionMessageHandler,
};
mod dfs_tree;
mod ipc_tree;
use crate::msg_handler::OutgoingMessage;
pub use dfs_tree::*;
pub use ipc_tree::*;

type Upstream = HandlerReference<SessionMessageHandler>;

#[derive(Debug, Clone)]
pub struct TreeConnectInfo {
    share_type: ShareType,
    share_flags: ShareFlags,
}

/// Represents an SMB share.
///
/// A Tree is the SMB protocol's representation of a connected share on the server.
pub struct Tree {
    handler: HandlerReference<TreeMessageHandler>,
    conn_info: Arc<ConnectionInfo>,
}

#[maybe_async(AFIT)]
impl Tree {
    #[maybe_async]
    pub(crate) async fn connect(
        name: &str,
        upstream: &Upstream,
        conn_info: &Arc<ConnectionInfo>,
    ) -> crate::Result<Tree> {
        // send and receive tree request & response.
        let response = upstream
            .send_recv(TreeConnectRequest::new(name).into())
            .await?;

        let content = response.message.content.to_treeconnect()?;

        // Make sure the share flags from the server are valid to the dialect.
        if ((!u32::from_le_bytes(conn_info.dialect.get_tree_connect_caps_mask().into_bytes()))
            & u32::from_le_bytes(content.capabilities.into_bytes()))
            != 0
        {
            return Err(Error::InvalidMessage(format!(
                "Invalid share flags received from server for tree '{}': {:?}",
                name, content.share_flags
            )));
        }

        // Same for share flags
        if ((!u32::from_le_bytes(conn_info.dialect.get_share_flags_mask().into_bytes()))
            & u32::from_le_bytes(content.share_flags.into_bytes()))
            != 0
        {
            return Err(Error::InvalidMessage(format!(
                "Invalid capabilities received from server for tree '{}': {:?}",
                name, content.capabilities
            )));
        }

        // If encryption is required, make sure it is available.
        if content.share_flags.encrypt_data() && conn_info.config.encryption_mode.is_disabled() {
            return Err(Error::InvalidMessage(
                "Server requires encryption, but client does not support it".to_string(),
            ));
        }

        let tree_id = response
            .message
            .header
            .tree_id
            .ok_or(Error::InvalidMessage(
                "Tree ID is not set in the response".to_string(),
            ))?;

        log::info!("Connected to tree {name} (#{tree_id})");

        let tree_connect_info = TreeConnectInfo {
            share_type: content.share_type,
            share_flags: content.share_flags,
        };

        let t = Tree {
            handler: TreeMessageHandler::new(
                upstream,
                tree_id,
                name.to_string(),
                tree_connect_info,
            ),
            conn_info: conn_info.clone(),
        };

        Ok(t)
    }

    /// Creates a resource (file, directory, pipe, or printer) on the remote server by it's name.
    /// See [Tree::create_file] and [Tree::create_directory] for an easier API.
    /// # Arguments
    /// * `file_name` - The name of the resource to create. This should NOT contain the share name, or begin with a backslash.
    /// * `args` - The arguments for the create operation. This includes the desired access, file attributes, and create options.
    ///     See [`FileCreateArgs`] for more information.
    /// # Returns
    /// * A [Resource] object representing the created resource. This can be a file, directory, pipe, or printer.
    /// # Notes
    /// This function automatically handles the following:
    /// * *DFS operations*: If the share has been opened as a DFS referral share, the create operation will modify the file name to include the DFS path.
    ///     That is, assuming it is NOT prefixed with "\\". This is rquired for a proper DFS referral file open. ("DFS normalization", MS-SMB2 2.2.13 + 3.3.5.9)
    pub async fn create(&self, file_name: &str, args: &FileCreateArgs) -> crate::Result<Resource> {
        let info = self.handler.info()?;
        Resource::create(
            file_name,
            &self.handler,
            args,
            &self.conn_info,
            info.share_type,
            info.share_flags.dfs(),
        )
        .await
    }

    /// A wrapper around [Tree::create] that creates a file on the remote server.
    /// See [Tree::create] for more information.
    pub async fn create_file(
        &self,
        file_name: &str,
        disposition: CreateDisposition,
        desired_access: FileAccessMask,
    ) -> crate::Result<Resource> {
        self.create(
            file_name,
            &FileCreateArgs {
                disposition,
                options: CreateOptions::new(),
                desired_access,
                attributes: FileAttributes::new(),
                share_access: None,
            },
        )
        .await
    }

    /// A wrapper around [Tree::create] that creates a directory on the remote server.
    /// See [Tree::create] for more information.
    pub async fn create_directory(
        &self,
        dir_name: &str,
        disposition: CreateDisposition,
        desired_access: FileAccessMask,
    ) -> crate::Result<Resource> {
        self.create(
            dir_name,
            &FileCreateArgs {
                disposition,
                options: CreateOptions::new().with_directory_file(true),
                desired_access,
                attributes: FileAttributes::new().with_directory(true),
                share_access: None,
            },
        )
        .await
    }

    /// A wrapper around [create][crate::tree::Tree::create] that opens an existing file or directory on the remote server.
    /// See [create][crate::tree::Tree::create] for more information.
    pub async fn open_existing(
        &self,
        file_name: &str,
        access: FileAccessMask,
    ) -> crate::Result<Resource> {
        self.create(file_name, &FileCreateArgs::make_open_existing(access))
            .await
    }

    pub fn is_dfs_root(&self) -> crate::Result<bool> {
        let info = self.handler.info()?;
        Ok(info.share_flags.dfs_root() && info.share_flags.dfs())
    }

    pub fn as_dfs_tree(&self) -> crate::Result<DfsRootTreeRef<'_>> {
        if !self.is_dfs_root()? {
            return Err(Error::InvalidState("Tree is not a DFS tree".to_string()));
        }
        Ok(DfsRootTreeRef::new(self))
    }

    pub fn as_ipc_tree(&self) -> crate::Result<IpcTreeRef<'_>> {
        let info = self.handler.info()?;
        if info.share_type != ShareType::Pipe {
            return Err(Error::InvalidState(format!(
                "Tree is not IPC tree ({:?})",
                info.share_type
            )));
        }

        IpcTreeRef::new(self)
    }

    /// Disconnects from the tree (share) on the server.
    ///
    /// After calling this method, none of the resources held open by the tree are accessible.
    pub async fn disconnect(&self) -> crate::Result<()> {
        self.handler.disconnect().await?;
        Ok(())
    }

    // TODO: Make it common with ResourceHandle::fsctl_with_options
    #[maybe_async]
    pub(crate) async fn fsctl_with_options<T: FsctlRequest>(
        &self,
        request: T,
        max_output_response: u32,
    ) -> crate::Result<T::Response> {
        const NO_INPUT_IN_RESPONSE: u32 = 0;
        let response = self
            .handler
            .send_recv(RequestContent::Ioctl(IoctlRequest {
                ctl_code: T::FSCTL_CODE as u32,
                file_id: FileId::FULL,
                max_input_response: NO_INPUT_IN_RESPONSE,
                max_output_response,
                flags: IoctlRequestFlags::new().with_is_fsctl(true),
                buffer: request.into(),
            }))
            .await?
            .message
            .content
            .to_ioctl()?
            .parse_fsctl::<T::Response>()?;
        Ok(response)
    }
}

pub(crate) struct TreeMessageHandler {
    tree_id: AtomicU32,

    upstream: Upstream,

    tree_name: String,
    info: TreeConnectInfo,
}

impl TreeMessageHandler {
    const INVALID_TREE_ID: u32 = u32::MAX;

    pub fn new(
        upstream: &Upstream,
        tree_id: u32,
        tree_name: String,
        info: TreeConnectInfo,
    ) -> HandlerReference<TreeMessageHandler> {
        HandlerReference::new(TreeMessageHandler {
            tree_id: AtomicU32::new(tree_id),
            upstream: upstream.clone(),
            info,
            tree_name,
        })
    }

    #[maybe_async]
    async fn _disconnect(upstream: Upstream, tree_id: u32, encrypt: bool) -> crate::Result<()> {
        // send and receive tree disconnect request & response.
        let request_content: RequestContent = TreeDisconnectRequest::default().into();
        let mut message = OutgoingMessage::new(request_content).with_encrypt(encrypt);
        message.message.header.tree_id = Some(tree_id);

        let _response = upstream.sendo_recv(message).await?;

        Ok(())
    }

    #[maybe_async]
    async fn disconnect(&self) -> crate::Result<()> {
        let tree_id = self.tree_id.swap(Self::INVALID_TREE_ID, Ordering::SeqCst);
        if tree_id == Self::INVALID_TREE_ID {
            // Already disconnected
            return Ok(());
        }
        let encrypt = self.info.share_flags.encrypt_data();
        Self::_disconnect(self.upstream.clone(), tree_id, encrypt).await
    }

    pub fn info(&self) -> crate::Result<&TreeConnectInfo> {
        if self.tree_id.load(Ordering::Relaxed) == Self::INVALID_TREE_ID {
            return Err(Error::InvalidState("Tree is closed".to_string()));
        }

        Ok(&self.info)
    }
}

impl MessageHandler for TreeMessageHandler {
    #[maybe_async]
    async fn sendo(
        &self,
        mut msg: crate::msg_handler::OutgoingMessage,
    ) -> crate::Result<crate::msg_handler::SendMessageResult> {
        if !msg.message.header.flags.async_command() {
            msg.message.header.tree_id = self.tree_id.load(Ordering::SeqCst).into();
            if self.info.share_flags.encrypt_data() {
                msg.encrypt = true;
            }
        }

        self.upstream.sendo(msg).await
    }

    #[maybe_async]
    async fn recvo(
        &self,
        options: crate::msg_handler::ReceiveOptions<'_>,
    ) -> crate::Result<crate::msg_handler::IncomingMessage> {
        let msg = self.upstream.recvo(options).await?;

        if !msg.message.header.flags.async_command()
            && msg.message.header.tree_id.unwrap() != self.tree_id.load(Ordering::SeqCst)
        {
            return Err(Error::InvalidMessage(
                "Received message for different tree, or tree disconnecting.".to_string(),
            ));
        }

        // Make sure encryption is enforced if the share requires it.
        if !msg.form.encrypted && self.info()?.share_flags.encrypt_data() {
            return Err(Error::InvalidMessage(
                "Received unencrypted message on encrypted share".to_string(),
            ));
        }

        Ok(msg)
    }
}

#[cfg(not(feature = "async"))]
impl Drop for TreeMessageHandler {
    fn drop(&mut self) {
        self.disconnect()
            .map_err(|e| {
                log::error!("Failed to disconnect from tree {}: {e}", self.tree_name);
                e
            })
            .ok();
    }
}

#[cfg(feature = "async")]
impl Drop for TreeMessageHandler {
    fn drop(&mut self) {
        let tree_id = self.tree_id.load(Ordering::SeqCst);
        if tree_id == Self::INVALID_TREE_ID {
            // Already dropped
            return;
        }

        // Local patch (graphql-orm-storage): the TREE_DISCONNECT goes to the
        // connection's bounded cleanup queue, which needs no current runtime
        // and runs it on the connection's runtime within a deadline. The
        // queued step holds only a weak reference to the session, so it never
        // keeps the session, connection or socket alive. If the session is
        // gone by then, its logoff disconnects the tree.
        let Some(cleanup) = self.upstream.cleanup_queue() else {
            log::debug!(
                "Tree {} dropped before negotiation completed; nothing to disconnect.",
                self.tree_name
            );
            return;
        };
        let session = self.upstream.weak();
        let tree_name = self.tree_name.clone();
        let encrypt = self.info.share_flags.encrypt_data();
        let deferred = cleanup.defer(Box::new(move || {
            let upstream = HandlerReference {
                handler: session.upgrade()?,
            };
            Some(Box::pin(async move {
                Self::_disconnect(upstream, tree_id, encrypt)
                    .await
                    .map_err(|e| {
                        log::error!("Failed to disconnect from tree {}: {e}", tree_name);
                    })
                    .ok();
            }))
        }));
        if !deferred {
            log::debug!(
                "Disconnect of tree {} not deferred; it ends with its session.",
                self.tree_name
            );
        }
    }
}

/// Local patch (graphql-orm-storage): cleanup that destructors start never
/// keeps the session, connection or socket alive, and never waits forever.
#[cfg(all(test, feature = "async"))]
mod destructor_cleanup_tests {
    use super::*;
    use crate::connection::ConnectionMessageHandler;
    use crate::connection::config::ConnectionConfig;
    use crate::connection::connection_info::{
        CleanupQueue, CleanupStats, ConnectionInfo, NegotiatedProperties,
    };
    use crate::connection::worker::Worker;
    use crate::dialects::DialectImpl;
    use crate::resource::ResourceHandle;
    use crate::session::{ChannelMessageHandler, SessionAndChannel, SessionInfo};
    use crate::sync_helpers::RwLock;
    use futures_core::future::BoxFuture;
    use futures_util::FutureExt;
    use smb_dtyp::Guid;
    use smb_msg::{Dialect, GlobalCapabilities, SessionFlags};
    use smb_transport::{SmbTransport, SmbTransportRead, SmbTransportWrite, TransportError};
    use std::net::SocketAddr;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    const SESSION_ID: u64 = 0x0000_4000_0000_0021;
    const SESSION_KEY: [u8; 16] = [0x3c; 16];
    const TREE_ID: u32 = 5;
    /// Each deferred cleanup step's deadline.
    const DEADLINE: Duration = Duration::from_millis(200);

    /// A server that accepts every write and never sends anything, so it
    /// never grants a credit.
    struct IdleServer;

    impl SmbTransport for IdleServer {
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
            Ok((self, Box::new(IdleServer)))
        }

        fn remote_address(&self) -> smb_transport::error::Result<SocketAddr> {
            Ok(SocketAddr::from(([127, 0, 0, 1], 445)))
        }
    }

    impl SmbTransportRead for IdleServer {
        fn receive_exact<'a>(
            &'a mut self,
            _out_buf: &'a mut [u8],
        ) -> BoxFuture<'a, smb_transport::error::Result<()>> {
            async {
                std::future::pending::<()>().await;
                Err(TransportError::NotConnected)
            }
            .boxed()
        }
    }

    impl SmbTransportWrite for IdleServer {
        fn send_raw<'a>(
            &'a mut self,
            _buf: &'a [u8],
        ) -> BoxFuture<'a, smb_transport::error::Result<()>> {
            async { Ok(()) }.boxed()
        }
    }

    /// A server that never sends anything and never accepts a write; its
    /// write half reports when it is dropped.
    struct StuckWriteServer {
        write_dropped: Arc<std::sync::atomic::AtomicBool>,
    }

    struct StuckWrite {
        dropped: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Drop for StuckWrite {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    impl SmbTransport for StuckWriteServer {
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
            let write = StuckWrite {
                dropped: self.write_dropped.clone(),
            };
            Ok((Box::new(IdleServer), Box::new(write)))
        }

        fn remote_address(&self) -> smb_transport::error::Result<SocketAddr> {
            Ok(SocketAddr::from(([127, 0, 0, 1], 445)))
        }
    }

    impl SmbTransportRead for StuckWriteServer {
        fn receive_exact<'a>(
            &'a mut self,
            _out_buf: &'a mut [u8],
        ) -> BoxFuture<'a, smb_transport::error::Result<()>> {
            async {
                std::future::pending::<()>().await;
                Err(TransportError::NotConnected)
            }
            .boxed()
        }
    }

    impl SmbTransportWrite for StuckWriteServer {
        fn send_raw<'a>(
            &'a mut self,
            _buf: &'a [u8],
        ) -> BoxFuture<'a, smb_transport::error::Result<()>> {
            async {
                std::future::pending::<()>().await;
                Ok(())
            }
            .boxed()
        }
    }

    impl SmbTransportWrite for StuckWrite {
        fn send_raw<'a>(
            &'a mut self,
            _buf: &'a [u8],
        ) -> BoxFuture<'a, smb_transport::error::Result<()>> {
            async {
                std::future::pending::<()>().await;
                Ok(())
            }
            .boxed()
        }
    }

    fn connection_info() -> ConnectionInfo {
        let dialect = Dialect::Smb0302;
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
            config: ConnectionConfig::default(),
            preauth_hash: crate::connection::preauth_hash::PreauthHashState::Unsupported,
            client_guid: Guid::generate(),
            cleanup: CleanupQueue::start(DEADLINE),
        }
    }

    fn alive_tasks() -> usize {
        tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks()
    }

    struct Chain {
        connection: HandlerReference<ConnectionMessageHandler>,
        info: Arc<ConnectionInfo>,
        stats: Arc<CleanupStats>,
        session: HandlerReference<SessionMessageHandler>,
        tree: HandlerReference<TreeMessageHandler>,
    }

    /// Real connection, channel, session and tree handlers over a server
    /// that never grants a credit. The connection's only credit is already
    /// spent, so every request waits for a credit forever.
    async fn chain() -> Chain {
        let (connection, info) = ConnectionMessageHandler::with_test_transport_negotiated(
            Box::new(IdleServer),
            connection_info(),
        )
        .await
        .unwrap();
        let stats = info.cleanup.stats_handle();
        let mut session = SessionInfo::new(SESSION_ID);
        session.setup(&SESSION_KEY, &None, &info).unwrap();
        session.ready(SessionFlags::new(), &info).unwrap();
        let state = Arc::new(RwLock::new(SessionAndChannel::new(
            SESSION_ID,
            Arc::new(RwLock::new(session)),
        )));
        connection
            .worker()
            .unwrap()
            .session_started(&state)
            .await
            .unwrap();
        let channel = ChannelMessageHandler::new(SESSION_ID, 1, &connection, &state);
        let session = HandlerReference::new(SessionMessageHandler::new(channel));
        let tree = TreeMessageHandler::new(
            &session,
            TREE_ID,
            "share".to_string(),
            TreeConnectInfo {
                share_type: ShareType::Disk,
                share_flags: ShareFlags::new(),
            },
        );
        assert_eq!(
            connection.available_credits(),
            0,
            "precondition: credits are exhausted, and the server grants none"
        );
        Chain {
            connection,
            info,
            stats,
            session,
            tree,
        }
    }

    async fn settle() {
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
    }

    /// Waits up to `limit` for `done`.
    async fn eventually(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
        tokio::time::timeout(limit, async {
            while !done() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    }

    /// #717: a handle CLOSE that times out waiting for a credit drops the last
    /// reference to its tree. The tree's TREE_DISCONNECT, the session's
    /// LOGOFF and the connection's worker stop that follow must not keep the
    /// session or connection alive, or leave a task waiting for a credit.
    #[tokio::test]
    async fn a_timed_out_close_that_drops_the_last_tree_frees_the_session_and_connection() {
        let Chain {
            connection,
            info,
            stats,
            session,
            tree,
        } = chain().await;
        let (session_weak, connection_weak) = (session.weak(), connection.weak());

        // A handle dropped while open defers its CLOSE; it starts at once and
        // waits for a credit while holding its tree.
        assert!(info.cleanup.defer(ResourceHandle::deferred_close(
            tree.weak(),
            FileId {
                persistent: 1,
                volatile: 2,
            },
        )));
        settle().await;
        // Every other owner lets go: the waiting CLOSE holds the last tree
        // reference, which holds the session, channel and connection.
        drop((tree, session, connection, info));
        assert!(
            session_weak.upgrade().is_some(),
            "precondition: the running CLOSE holds the session"
        );

        // At its deadline the CLOSE is abandoned and drops the last tree
        // reference. Within one more deadline the session and connection are
        // freed and no task is left.
        let freed = eventually(DEADLINE * 3, || {
            session_weak.upgrade().is_none()
                && connection_weak.upgrade().is_none()
                && stats.finished.load(Ordering::SeqCst)
                && alive_tasks() == 0
        })
        .await;
        assert!(
            freed,
            "after the CLOSE timed out: session alive {}, connection alive {}, \
             cleanup finished {}, {} tasks alive",
            session_weak.upgrade().is_some(),
            connection_weak.upgrade().is_some(),
            stats.finished.load(Ordering::SeqCst),
            alive_tasks()
        );
        assert_eq!(stats.timed_out.load(Ordering::SeqCst), 1, "{stats:?}");
        // The TREE_DISCONNECT and LOGOFF were still queued when the last
        // connection owner dropped, which shut the queue down.
        assert_eq!(stats.abandoned.load(Ordering::SeqCst), 2, "{stats:?}");
        // The worker stop finished within its deadline.
        assert_eq!(stats.completed.load(Ordering::SeqCst), 1, "{stats:?}");
    }

    /// #717: while the connection stays open, the TREE_DISCONNECT and LOGOFF
    /// that destructors defer each wait for a credit that never comes. Each
    /// is abandoned at its deadline, and neither keeps the session alive
    /// afterwards or holds the connection.
    #[tokio::test]
    async fn deferred_disconnect_and_logoff_each_time_out_and_release_what_they_hold() {
        let Chain {
            connection,
            info,
            stats,
            session,
            tree,
        } = chain().await;
        drop(info);
        let session_weak = session.weak();

        // The tree's destructor defers its TREE_DISCONNECT, which holds the
        // session while it waits for a credit.
        drop(tree);
        settle().await;
        assert!(
            Arc::strong_count(&session.handler) > 1,
            "precondition: the waiting TREE_DISCONNECT holds the session"
        );
        assert!(
            eventually(DEADLINE * 3, || Arc::strong_count(&session.handler) == 1).await,
            "the TREE_DISCONNECT still holds the session"
        );
        assert_eq!(stats.timed_out.load(Ordering::SeqCst), 1, "{stats:?}");

        // The session's destructor defers its LOGOFF, which holds the
        // connection only while it waits.
        drop(session);
        assert!(session_weak.upgrade().is_none(), "the session is freed");
        settle().await;
        assert!(
            Arc::strong_count(&connection.handler) > 1,
            "precondition: the waiting LOGOFF holds the connection"
        );
        assert!(
            eventually(DEADLINE * 3, || Arc::strong_count(&connection.handler) == 1).await,
            "the LOGOFF still holds the connection"
        );
        assert_eq!(stats.timed_out.load(Ordering::SeqCst), 2, "{stats:?}");

        // Dropping the connection ends the drain task and the worker.
        drop(connection);
        assert!(
            eventually(DEADLINE * 3, || stats.finished.load(Ordering::SeqCst)
                && alive_tasks() == 0)
            .await,
            "{} tasks alive",
            alive_tasks()
        );
        assert_eq!(stats.abandoned.load(Ordering::SeqCst), 0, "{stats:?}");
    }

    /// #717: the worker stop that the connection's destructor starts has a
    /// deadline. A send loop inside a socket write that never completes does
    /// not see the stop; at the deadline it is aborted, which drops the
    /// socket, and no task is left.
    #[tokio::test]
    async fn a_worker_stop_blocked_in_a_socket_write_is_abandoned_and_releases_the_socket() {
        let write_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (connection, info) = ConnectionMessageHandler::with_test_transport_negotiated(
            Box::new(StuckWriteServer {
                write_dropped: write_dropped.clone(),
            }),
            connection_info(),
        )
        .await
        .unwrap();
        let stats = info.cleanup.stats_handle();
        drop(info);
        // The worker hands the request to its send loop, which then waits
        // in the socket write forever.
        connection
            .worker()
            .unwrap()
            .send(OutgoingMessage::new(
                TreeDisconnectRequest::default().into(),
            ))
            .await
            .unwrap();
        settle().await;

        drop(connection);
        assert!(
            eventually(DEADLINE * 3, || write_dropped.load(Ordering::SeqCst)
                && alive_tasks() == 0)
            .await,
            "socket released {}, {} tasks alive",
            write_dropped.load(Ordering::SeqCst),
            alive_tasks()
        );
        assert_eq!(stats.timed_out.load(Ordering::SeqCst), 1, "{stats:?}");
    }
}
