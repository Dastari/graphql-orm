use std::{
    cell::RefCell,
    collections::BTreeMap,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    rc::{Rc, Weak},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Instant, SystemTime},
};

use hive_router::ntex::{
    SharedCfg, chain, rt,
    service::{Service, fn_factory_with_config, fn_service, fn_shutdown},
    web::{self, HttpRequest, HttpResponse, ws},
};
use serde_json::{Value, json};

use crate::{
    AuthenticationProvider, RouterError, RouterErrorKind, SubscriptionConfig,
    metrics::RouterMetrics,
};

pub(crate) const INTERNAL_SUBSCRIPTION_HEADER: &str = "x-graphql-orm-router-internal";
pub(crate) const INTERNAL_SUBSCRIPTION_VARIABLES_EXTENSION: &str = "graphqlOrmRouterVariables";
const GRAPHQL_TRANSPORT_WS: &str = "graphql-transport-ws";
const MAX_AUTHORIZATION_VARIABLE_BYTES: usize = 4 * 1024;
const MAX_AUTHORIZATION_VARIABLE_VALUE_BYTES: usize = 1024;

#[derive(Clone)]
pub(crate) struct InternalSubscriptionEndpoint {
    pub(crate) path: String,
    secret: String,
    pub(crate) continuity: crate::subscription_continuity::Continuity,
}

impl InternalSubscriptionEndpoint {
    pub(crate) fn generate() -> Result<Self, RouterError> {
        // The path and the secret come from separate random draws, so the
        // path, which reaches routing and logs, says nothing about the secret.
        let mut path = [0_u8; 16];
        let mut secret = [0_u8; 32];
        getrandom::fill(&mut path)
            .and_then(|()| getrandom::fill(&mut secret))
            .map_err(|_| {
                RouterError::new(
                    RouterErrorKind::Runtime,
                    "failed to initialize the private subscription transport",
                )
            })?;
        let hex = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        Ok(Self {
            path: format!("/_graphql-orm-router/{}/websocket", hex(&path)),
            secret: hex(&secret),
            continuity: Default::default(),
        })
    }

    pub(crate) fn authorizes(&self, path: &str, supplied: Option<&str>) -> bool {
        use subtle::ConstantTimeEq as _;
        path != self.path
            || supplied.is_some_and(|supplied| {
                bool::from(supplied.as_bytes().ct_eq(self.secret.as_bytes()))
            })
    }
}

impl std::fmt::Debug for InternalSubscriptionEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InternalSubscriptionEndpoint")
            .field("path", &"[private]")
            .field("secret", &"[redacted]")
            .finish()
    }
}

pub(crate) struct SubscriptionGateway {
    config: SubscriptionConfig,
    authentication: Arc<dyn AuthenticationProvider>,
    internal: InternalSubscriptionEndpoint,
    connect_address: SocketAddr,
    internal_url: String,
    /// Connections whose `connection_init` credential has validated.
    active_connections: AtomicUsize,
    /// Upgraded connections that have not authenticated yet. This budget is
    /// separate, so unauthenticated sockets never hold authenticated capacity.
    pending_connections: AtomicUsize,
    active_operations: AtomicUsize,
    connection_attempts: ConnectionAttemptLimiter,
    metrics: Arc<RouterMetrics>,
}

impl SubscriptionGateway {
    pub(crate) fn new(
        config: SubscriptionConfig,
        authentication: Arc<dyn AuthenticationProvider>,
        internal: InternalSubscriptionEndpoint,
        bound_address: SocketAddr,
        metrics: Arc<RouterMetrics>,
    ) -> Arc<Self> {
        let connect_address = loopback_connect_address(bound_address);
        let internal_url = format!("ws://localhost:{}{}", connect_address.port(), internal.path);
        let connection_attempts =
            ConnectionAttemptLimiter::new(config.max_connection_attempts_per_second);
        Arc::new(Self {
            config,
            authentication,
            internal,
            connect_address,
            internal_url,
            active_connections: AtomicUsize::new(0),
            pending_connections: AtomicUsize::new(0),
            active_operations: AtomicUsize::new(0),
            connection_attempts,
            metrics,
        })
    }

    fn try_reserve_connection_attempt(&self) -> bool {
        self.connection_attempts.try_acquire()
    }

    fn authenticated_capacity_reached(&self) -> bool {
        self.active_connections.load(Ordering::Acquire) >= self.config.max_connections
    }

    /// Charges the authenticated budget. Call only after the credential
    /// validates.
    fn try_reserve_connection(self: &Arc<Self>) -> Option<ConnectionPermit> {
        self.try_reserve(Admission::Authenticated)
    }

    fn try_reserve_pending_connection(self: &Arc<Self>) -> Option<ConnectionPermit> {
        self.try_reserve(Admission::Pending)
    }

    fn try_reserve(self: &Arc<Self>, admission: Admission) -> Option<ConnectionPermit> {
        let (counter, maximum) = self.admission_budget(admission);
        let mut current = counter.load(Ordering::Acquire);
        loop {
            if current >= maximum {
                return None;
            }
            match counter.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.metrics.websocket_connected();
                    return Some(ConnectionPermit {
                        gateway: self.clone(),
                        admission,
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn admission_budget(&self, admission: Admission) -> (&AtomicUsize, usize) {
        match admission {
            Admission::Pending => (
                &self.pending_connections,
                self.config.max_pending_connections,
            ),
            Admission::Authenticated => (&self.active_connections, self.config.max_connections),
        }
    }

    fn add_operation(&self) {
        self.active_operations.fetch_add(1, Ordering::AcqRel);
        self.metrics.subscription_started();
    }

    fn remove_operations(&self, count: usize) {
        if count != 0 {
            self.active_operations.fetch_sub(count, Ordering::AcqRel);
            self.metrics.subscriptions_ended(count);
        }
    }
}

struct ConnectionAttemptLimiter {
    maximum_per_second: usize,
    state: Mutex<ConnectionAttemptState>,
}

struct ConnectionAttemptState {
    available: f64,
    last_refill: Instant,
}

impl ConnectionAttemptLimiter {
    fn new(maximum_per_second: usize) -> Self {
        Self::new_at(maximum_per_second, Instant::now())
    }

    fn new_at(maximum_per_second: usize, now: Instant) -> Self {
        Self {
            maximum_per_second,
            state: Mutex::new(ConnectionAttemptState {
                available: maximum_per_second as f64,
                last_refill: now,
            }),
        }
    }

    fn try_acquire(&self) -> bool {
        self.try_acquire_at(Instant::now())
    }

    fn try_acquire_at(&self, now: Instant) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let elapsed = now.saturating_duration_since(state.last_refill);
        state.last_refill = now;
        let capacity = self.maximum_per_second as f64;
        state.available = (state.available + elapsed.as_secs_f64() * capacity).min(capacity);
        if state.available < 1.0 {
            return false;
        }
        state.available -= 1.0;
        true
    }
}

fn loopback_connect_address(bound: SocketAddr) -> SocketAddr {
    match bound.ip() {
        IpAddr::V4(address) if address.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bound.port())
        }
        IpAddr::V6(address) if address.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), bound.port())
        }
        _ => bound,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Admission {
    Pending,
    Authenticated,
}

struct ConnectionPermit {
    gateway: Arc<SubscriptionGateway>,
    admission: Admission,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.gateway
            .admission_budget(self.admission)
            .0
            .fetch_sub(1, Ordering::AcqRel);
        self.gateway.metrics.websocket_disconnected();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionPhase {
    WaitingForInit,
    Connecting,
    Ready,
    Closed,
}

struct ConnectionState {
    gateway: Arc<SubscriptionGateway>,
    phase: ConnectionPhase,
    internal_sink: Option<ws::WsSink>,
    operations: BTreeMap<String, Operation>,
    next_operation: u64,
    local_loss: hive_router::tokio::sync::watch::Sender<u64>,
    permit: Option<ConnectionPermit>,
}

struct Operation {
    internal_id: String,
    epoch: u64,
    subscription: bool,
    lost: bool,
}

struct ConnectionTermination {
    first: bool,
    internal_sink: Option<ws::WsSink>,
}

impl ConnectionState {
    fn operation_lost(&self, operation: &Operation) -> bool {
        operation.subscription
            && (operation.lost || operation.epoch != self.gateway.internal.continuity.epoch())
    }

    fn lose_connection_subscriptions(&mut self) {
        for operation in self
            .operations
            .values_mut()
            .filter(|operation| operation.subscription)
        {
            operation.lost = true;
        }
        self.local_loss
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    fn remove_operation(&mut self, id: &str) {
        if self.operations.remove(id).is_some() {
            self.gateway.remove_operations(1);
        }
    }

    fn terminate(&mut self) -> ConnectionTermination {
        if self.phase == ConnectionPhase::Closed {
            return ConnectionTermination {
                first: false,
                internal_sink: None,
            };
        }
        self.phase = ConnectionPhase::Closed;
        self.gateway.remove_operations(self.operations.len());
        self.operations.clear();
        self.permit.take();
        ConnectionTermination {
            first: true,
            internal_sink: self.internal_sink.take(),
        }
    }

    fn terminate_and_close_internal(&mut self) {
        let _ = self.terminate_and_close_internal_once();
    }

    fn terminate_and_close_internal_once(&mut self) -> bool {
        let termination = self.terminate();
        if let Some(internal) = termination.internal_sink {
            rt::spawn(async move {
                let _ = internal
                    .send(close_message(1000, "Client disconnected"))
                    .await;
            });
        }
        termination.first
    }

    fn terminate_with_public_close(&mut self, code: u16, description: &str) -> Option<ws::Message> {
        self.terminate_and_close_internal_once()
            .then(|| close_message(code, description))
    }
}

impl Drop for ConnectionState {
    fn drop(&mut self) {
        self.gateway.remove_operations(self.operations.len());
        self.operations.clear();
        self.permit.take();
    }
}

type SharedConnectionState = Rc<RefCell<ConnectionState>>;

pub(crate) async fn websocket_index(
    req: HttpRequest,
    gateway: Arc<SubscriptionGateway>,
) -> Result<HttpResponse, web::Error> {
    if !gateway.try_reserve_connection_attempt() {
        gateway.metrics.websocket_rejected();
        return Ok(HttpResponse::TooManyRequests()
            .set_header("retry-after", "1")
            .finish());
    }
    // Refuse early when authenticated capacity is already full; checking it
    // uses no budget. The upgrade itself holds only a pre-authentication slot.
    if gateway.authenticated_capacity_reached() {
        gateway.metrics.websocket_rejected();
        return Ok(HttpResponse::ServiceUnavailable().finish());
    }
    let Some(permit) = gateway.try_reserve_pending_connection() else {
        gateway.metrics.websocket_rejected();
        return Ok(HttpResponse::ServiceUnavailable().finish());
    };
    let accepted = ws::subprotocols(&req)
        .any(|protocol| protocol == GRAPHQL_TRANSPORT_WS)
        .then_some(GRAPHQL_TRANSPORT_WS);
    let permit = Rc::new(RefCell::new(Some(permit)));
    ws::start(
        req,
        accepted,
        fn_factory_with_config(move |sink: ws::WsSink| {
            let permit = permit.clone();
            let gateway = gateway.clone();
            async move {
                let permit = permit
                    .borrow_mut()
                    .take()
                    .expect("WebSocket service factory is called once");
                create_connection_service(sink, gateway, permit, accepted.is_some()).await
            }
        }),
    )
    .await
}

async fn create_connection_service(
    sink: ws::WsSink,
    gateway: Arc<SubscriptionGateway>,
    permit: ConnectionPermit,
    accepted_subprotocol: bool,
) -> Result<impl Service<ws::Frame, Response = Option<ws::Message>, Error = io::Error>, web::Error>
{
    let state = Rc::new(RefCell::new(ConnectionState {
        gateway: gateway.clone(),
        phase: ConnectionPhase::WaitingForInit,
        internal_sink: None,
        operations: BTreeMap::new(),
        next_operation: 0,
        local_loss: hive_router::tokio::sync::watch::channel(0).0,
        permit: Some(permit),
    }));

    spawn_continuity_watch(
        Rc::downgrade(&state),
        sink.clone(),
        gateway.internal.continuity.subscribe(),
    );

    if accepted_subprotocol {
        spawn_connection_init_timeout(
            Rc::downgrade(&state),
            sink.clone(),
            gateway.config.connection_init_timeout,
        );
    } else {
        state.borrow_mut().terminate_and_close_internal();
        let _ = sink
            .send(close_message(4406, "Subprotocol not acceptable"))
            .await;
    }

    let service_state = state.clone();
    let service_sink = sink.clone();
    let service = fn_service(move |frame| {
        let state = service_state.clone();
        let sink = service_sink.clone();
        async move { handle_frame(frame, sink, state).await }
    });
    let shutdown = fn_shutdown(async move || {
        let termination = state.borrow_mut().terminate();
        if let Some(internal) = termination.internal_sink {
            let _ = internal
                .send(close_message(1000, "Client disconnected"))
                .await;
        }
    });
    Ok(chain(service).and_then(shutdown))
}

fn spawn_connection_init_timeout(
    state: Weak<RefCell<ConnectionState>>,
    sink: ws::WsSink,
    timeout: std::time::Duration,
) {
    rt::spawn(async move {
        hive_router::tokio::time::sleep(timeout).await;
        let Some(state) = state.upgrade() else {
            return;
        };
        let termination = {
            let mut state = state.borrow_mut();
            if state.phase != ConnectionPhase::WaitingForInit {
                return;
            }
            state.terminate()
        };
        if let Some(internal) = termination.internal_sink {
            let _ = internal
                .send(close_message(1000, "Connection initialization timed out"))
                .await;
        }
        if termination.first {
            let _ = sink
                .send(close_message(4408, "Connection initialisation timeout"))
                .await;
        }
    });
}

fn spawn_expiry(
    state: Weak<RefCell<ConnectionState>>,
    sink: ws::WsSink,
    expires_at: SystemTime,
    now: SystemTime,
) {
    let delay = expires_at.duration_since(now).unwrap_or_default();
    rt::spawn(async move {
        hive_router::tokio::time::sleep(delay).await;
        let Some(state) = state.upgrade() else {
            return;
        };
        let termination = state.borrow_mut().terminate();
        if let Some(internal) = termination.internal_sink {
            let _ = internal
                .send(close_message(1000, "Credential expired"))
                .await;
        }
        if termination.first {
            let _ = sink
                .send(close_message(4401, "Bearer credential expired"))
                .await;
        }
    });
}

async fn handle_frame(
    frame: ws::Frame,
    sink: ws::WsSink,
    state: SharedConnectionState,
) -> Result<Option<ws::Message>, io::Error> {
    match frame {
        ws::Frame::Text(bytes) => {
            let max_bytes = state.borrow().gateway.config.max_client_message_bytes;
            if bytes.len() > max_bytes {
                return Ok(state
                    .borrow_mut()
                    .terminate_with_public_close(4400, "Client message is too large"));
            }
            let Ok(text) = std::str::from_utf8(&bytes) else {
                return Ok(state
                    .borrow_mut()
                    .terminate_with_public_close(4400, "Invalid client message"));
            };
            Ok(handle_text(text, sink, state).await)
        }
        ws::Frame::Ping(bytes) => Ok(Some(ws::Message::Pong(bytes))),
        ws::Frame::Pong(_) => Ok(None),
        ws::Frame::Close(reason) => {
            let termination = state.borrow_mut().terminate();
            if let Some(internal) = termination.internal_sink {
                let _ = internal.send(ws::Message::Close(reason.clone())).await;
            }
            Ok(termination.first.then(|| ws::Message::Close(reason)))
        }
        ws::Frame::Binary(_) | ws::Frame::Continuation(_) => Ok(state
            .borrow_mut()
            .terminate_with_public_close(4400, "Text messages are required")),
    }
}

async fn handle_text(
    text: &str,
    sink: ws::WsSink,
    state: SharedConnectionState,
) -> Option<ws::Message> {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return state
            .borrow_mut()
            .terminate_with_public_close(4400, "Invalid client message");
    };
    let Some(message_type) = message.get("type").and_then(Value::as_str) else {
        return state
            .borrow_mut()
            .terminate_with_public_close(4400, "Invalid client message");
    };

    match message_type {
        "connection_init" => initialize_connection(&message, sink, state).await,
        "ping" => Some(ws::Message::Text(
            json!({"type": "pong"}).to_string().into(),
        )),
        "pong" => None,
        "subscribe" => forward_subscribe(message, text, state).await,
        "complete" => forward_complete(message, text, state).await,
        _ => state
            .borrow_mut()
            .terminate_with_public_close(4400, "Invalid client message"),
    }
}

async fn initialize_connection(
    message: &Value,
    external_sink: ws::WsSink,
    state: SharedConnectionState,
) -> Option<ws::Message> {
    if state.borrow().phase != ConnectionPhase::WaitingForInit {
        return state
            .borrow_mut()
            .terminate_with_public_close(4429, "Too many initialisation requests");
    }
    let Some(authorization) = connection_init_authorization(message) else {
        return state
            .borrow_mut()
            .terminate_with_public_close(4401, "Invalid bearer credential");
    };
    let Some(token) = crate::server::strict_bearer_token(authorization) else {
        return state
            .borrow_mut()
            .terminate_with_public_close(4401, "Invalid bearer credential");
    };
    let gateway = state.borrow().gateway.clone();
    let principal = match gateway.authentication.authenticate_bearer(token) {
        Ok(principal) => principal,
        Err(_) => {
            return state
                .borrow_mut()
                .terminate_with_public_close(4401, "Invalid bearer credential");
        }
    };
    let Some(expires_at) = principal.expires_at() else {
        return state
            .borrow_mut()
            .terminate_with_public_close(4401, "Bearer credential has no usable expiry");
    };
    let now = match gateway.authentication.current_time() {
        Ok(now) => now,
        Err(_) => {
            return state
                .borrow_mut()
                .terminate_with_public_close(1011, "Authentication service unavailable");
        }
    };
    if expires_at <= now {
        return state
            .borrow_mut()
            .terminate_with_public_close(4401, "Bearer credential expired");
    }
    // Only now is the connection charged against the authenticated budget.
    // Replacing the permit releases its pre-authentication slot.
    let Some(permit) = gateway.try_reserve_connection() else {
        gateway.metrics.websocket_rejected();
        return state
            .borrow_mut()
            .terminate_with_public_close(1013, "Too many connections");
    };
    {
        let mut state = state.borrow_mut();
        state.permit = Some(permit);
        state.phase = ConnectionPhase::Connecting;
    }
    spawn_expiry(
        Rc::downgrade(&state),
        external_sink.clone(),
        expires_at,
        now,
    );

    if connect_internal(&gateway, token, external_sink, state.clone())
        .await
        .is_err()
    {
        return state
            .borrow_mut()
            .terminate_with_public_close(1011, "Subscription transport unavailable");
    }
    None
}

fn connection_init_authorization(message: &Value) -> Option<&str> {
    let payload = message.get("payload")?.as_object()?;
    let headers = payload
        .get("headers")
        .and_then(Value::as_object)
        .unwrap_or(payload);
    let mut values = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.as_str());
    let value = values.next()??;
    values.next().is_none().then_some(value)
}

// Select using the executable AST, never textual prefixes: operationName may
// select a query from a document that also contains a subscription or fragments.
fn selected_subscription(message: &Value) -> Option<bool> {
    let payload = message.get("payload")?;
    let document = cynic_parser::parse_executable_document(payload.get("query")?.as_str()?).ok()?;
    let name = match payload.get("operationName") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_str()?),
    };
    let mut operations = document
        .operations()
        .filter(|operation| name.is_none_or(|name| operation.name() == Some(name)));
    let operation = operations.next()?;
    if operations.next().is_some() {
        return None;
    }
    Some(operation.operation_type() == cynic_parser::common::OperationType::Subscription)
}

fn enqueue_subscription_frame(
    tx: &hive_router::tokio::sync::mpsc::Sender<
        Result<ws::Frame, hive_router::ntex::ws::error::WsError<()>>,
    >,
    frame: ws::Frame,
    state: &SharedConnectionState,
) -> Option<ws::Frame> {
    let id = match &frame {
        ws::Frame::Text(bytes) => serde_json::from_slice::<Value>(bytes)
            .ok()
            .and_then(|message| message.get("id").and_then(Value::as_str).map(str::to_owned)),
        _ => None,
    };
    let mut state = state.borrow_mut();
    let Some(operation) = id.as_ref().and_then(|id| {
        state
            .operations
            .values_mut()
            .find(|operation| operation.internal_id == *id && operation.subscription)
    }) else {
        return Some(frame);
    };
    if operation.lost {
        return None;
    }
    if matches!(
        tx.try_send(Ok(frame)),
        Err(hive_router::tokio::sync::mpsc::error::TrySendError::Full(_))
    ) {
        operation.lost = true;
        state
            .local_loss
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }
    None
}

async fn connect_internal(
    gateway: &Arc<SubscriptionGateway>,
    token: &str,
    external_sink: ws::WsSink,
    state: SharedConnectionState,
) -> Result<(), ()> {
    let mut builder = hive_router::ntex::ws::WsClient::builder(&gateway.internal_url);
    builder
        .address(gateway.connect_address)
        .protocols([GRAPHQL_TRANSPORT_WS])
        .max_frame_size(gateway.config.max_client_message_bytes)
        .timeout(gateway.config.connection_init_timeout)
        .set_header(
            INTERNAL_SUBSCRIPTION_HEADER,
            gateway.internal.secret.as_str(),
        )
        .set_header("authorization", format!("Bearer {token}"));
    let client = builder.build(SharedCfg::default()).await.map_err(|_| ())?;
    let connection = client.connect().await.map_err(|_| ())?;
    let internal_sink = connection.sink();
    // ntex's receiver() uses an unbounded queue. Keep the bridge bounded and
    // attribute a full queue to the subscription named by the dropped frame.
    let (tx, receiver) = hive_router::tokio::sync::mpsc::channel(32);
    let bridge_state = state.clone();
    let bridge_external = external_sink.clone();
    rt::spawn(async move {
        let _ = connection
            .seal()
            .start(fn_service(move |frame| {
                let state = bridge_state.clone();
                let sink = bridge_external.clone();
                let queued = enqueue_subscription_frame(&tx, frame, &state);
                async move {
                    // One-shot results and connection controls never enter the
                    // lossy subscription queue, even when it is saturated.
                    match queued {
                        Some(ws::Frame::Text(bytes)) => {
                            if let Some(text) = public_server_message(&bytes, &state)
                                && sink.send(ws::Message::Text(text.into())).await.is_err()
                            {
                                state.borrow_mut().terminate_and_close_internal();
                            }
                        }
                        Some(ws::Frame::Ping(bytes)) => {
                            return Ok::<_, ()>(Some(ws::Message::Pong(bytes)));
                        }
                        Some(ws::Frame::Close(reason)) => {
                            state.borrow_mut().terminate_and_close_internal();
                            let _ = sink.send(ws::Message::Close(reason)).await;
                        }
                        Some(ws::Frame::Binary(_) | ws::Frame::Continuation(_)) => {
                            state.borrow_mut().terminate_and_close_internal();
                            let _ = sink
                                .send(close_message(1011, "Subscription transport unavailable"))
                                .await;
                        }
                        _ => {}
                    }
                    Ok::<_, ()>(None)
                }
            }))
            .await;
    });
    state.borrow_mut().internal_sink = Some(internal_sink.clone());
    let init = json!({
        "type": "connection_init",
        "payload": {
            "authorization": format!("Bearer {token}"),
            INTERNAL_SUBSCRIPTION_HEADER: gateway.internal.secret,
        }
    });
    internal_sink
        .send(ws::Message::Text(init.to_string().into()))
        .await
        .map_err(|_| ())?;
    rt::spawn(forward_internal_messages(
        receiver,
        internal_sink,
        external_sink,
        state,
    ));
    Ok(())
}

async fn forward_internal_messages(
    mut receiver: hive_router::tokio::sync::mpsc::Receiver<
        Result<ws::Frame, hive_router::ntex::ws::error::WsError<()>>,
    >,
    internal_sink: ws::WsSink,
    external_sink: ws::WsSink,
    state: SharedConnectionState,
) {
    enum ForwardEnd {
        InternalFailure,
        ExternalFailure,
        InternalClose(Option<ws::CloseReason>),
    }

    let mut end = ForwardEnd::InternalFailure;
    while let Some(frame) = receiver.recv().await {
        let Ok(frame) = frame else {
            break;
        };
        match frame {
            ws::Frame::Text(bytes) => {
                if external_sink.io().is_wr_backpressure() {
                    state.borrow_mut().lose_connection_subscriptions();
                }
                let Some(text) = public_server_message(&bytes, &state) else {
                    continue;
                };
                if external_sink
                    .send(ws::Message::Text(text.into()))
                    .await
                    .is_err()
                {
                    end = ForwardEnd::ExternalFailure;
                    break;
                }
            }
            ws::Frame::Ping(bytes) => {
                if internal_sink.send(ws::Message::Pong(bytes)).await.is_err() {
                    break;
                }
            }
            ws::Frame::Pong(_) => {}
            ws::Frame::Close(reason) => {
                end = ForwardEnd::InternalClose(reason);
                break;
            }
            ws::Frame::Binary(_) | ws::Frame::Continuation(_) => break,
        }
    }
    let termination = state.borrow_mut().terminate();
    if !termination.first {
        return;
    }
    match end {
        ForwardEnd::InternalClose(reason) => {
            let _ = external_sink.send(ws::Message::Close(reason)).await;
        }
        ForwardEnd::InternalFailure => {
            let _ = external_sink
                .send(close_message(1011, "Subscription transport unavailable"))
                .await;
        }
        ForwardEnd::ExternalFailure => {
            if let Some(internal) = termination.internal_sink {
                let _ = internal
                    .send(close_message(1000, "Client disconnected"))
                    .await;
            }
        }
    }
}

// Hive can emit complete after an error. Translate each bridge generation back to
// its public ID only while active: error is terminal, even if that ID is reused.
fn public_server_message(bytes: &[u8], state: &SharedConnectionState) -> Option<String> {
    let mut message = serde_json::from_slice::<Value>(bytes).ok()?;
    let mut state = state.borrow_mut();
    match message.get("type").and_then(Value::as_str) {
        Some("connection_ack") => {
            if state.phase == ConnectionPhase::Connecting {
                state.phase = ConnectionPhase::Ready;
            }
        }
        Some(kind @ ("next" | "error" | "complete")) => {
            let internal_id = message.get("id")?.as_str()?;
            let id = state
                .operations
                .iter()
                .find(|(_, operation)| operation.internal_id == internal_id)
                .map(|(id, _)| id.clone())?;
            if state.operation_lost(&state.operations[&id]) {
                if let Some(internal) = state.internal_sink.clone() {
                    let complete = json!({"id": internal_id, "type": "complete"});
                    rt::spawn(async move {
                        let _ = internal
                            .send(ws::Message::Text(complete.to_string().into()))
                            .await;
                    });
                }
                state.remove_operation(&id);
                return Some(continuity_error(&id).to_string());
            }
            if kind != "next" {
                state.remove_operation(&id);
            }
            message["id"] = Value::String(id);
        }
        _ => {}
    }
    Some(message.to_string())
}

async fn forward_subscribe(
    mut message: Value,
    _raw: &str,
    state: SharedConnectionState,
) -> Option<ws::Message> {
    if state.borrow().phase != ConnectionPhase::Ready {
        return state
            .borrow_mut()
            .terminate_with_public_close(4401, "Connection is not acknowledged");
    }
    let Some(id) = message.get("id").and_then(Value::as_str).map(str::to_owned) else {
        return state
            .borrow_mut()
            .terminate_with_public_close(4400, "Subscription ID is required");
    };
    if id.is_empty() {
        return state
            .borrow_mut()
            .terminate_with_public_close(4400, "Subscription ID is required");
    }
    {
        let mut state = state.borrow_mut();
        // Duplicate IDs are connection errors, regardless of the new payload.
        // No operation-scoped terminal error may leave the old generation live.
        if state.operations.contains_key(&id) {
            return state.terminate_with_public_close(4409, "Subscriber already exists");
        }
        let Some(subscription) = selected_subscription(&message) else {
            return Some(ws::Message::Text(
                json!({"id": id, "type": "error", "payload": [{
                    "message": "An explicit, unambiguous GraphQL operation is required",
                    "extensions": {"code": "GRAPHQL_VALIDATION_FAILED"}
                }]})
                .to_string()
                .into(),
            ));
        };
        if state.operations.len() >= state.gateway.config.max_operations_per_connection {
            return Some(operation_limit_error(&id));
        }
        let epoch = if subscription {
            let Some(epoch) = state.gateway.internal.continuity.subscription_epoch() else {
                return Some(ws::Message::Text(continuity_error(&id).to_string().into()));
            };
            epoch
        } else {
            0
        };
        let Some(next) = state.next_operation.checked_add(1) else {
            return state.terminate_with_public_close(1011, "Operation sequence exhausted");
        };
        state.next_operation = next;
        let internal_id = next.to_string();
        message["id"] = Value::String(internal_id.clone());
        state.operations.insert(
            id.clone(),
            Operation {
                internal_id,
                epoch,
                subscription,
                lost: false,
            },
        );
        state.gateway.add_operation();
    }
    let internal_max_bytes = state.borrow().gateway.config.max_client_message_bytes;
    inject_operation_variables(&mut message, internal_max_bytes);
    let internal = state.borrow().internal_sink.clone();
    if let Some(internal) = internal
        && internal
            .send(ws::Message::Text(message.to_string().into()))
            .await
            .is_ok()
    {
        return None;
    }
    state.borrow_mut().remove_operation(&id);
    state
        .borrow_mut()
        .terminate_with_public_close(1011, "Subscription transport unavailable")
}

fn inject_operation_variables(message: &mut Value, internal_max_bytes: usize) {
    let Some(payload) = message.get_mut("payload").and_then(Value::as_object_mut) else {
        return;
    };
    let mut candidates = payload
        .get("variables")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|variables| variables.iter())
        .filter(|(_, value)| {
            value.is_null() || value.is_boolean() || value.is_number() || value.is_string()
        })
        .filter_map(|(name, value)| {
            let encoded = value.to_string();
            (encoded.len() <= MAX_AUTHORIZATION_VARIABLE_VALUE_BYTES)
                .then(|| (name.to_owned(), value.clone(), name.len() + encoded.len()))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.0.cmp(&right.0));

    let mut projected = serde_json::Map::new();
    let mut projected_bytes = 0_usize;
    for (name, value, encoded_bytes) in candidates {
        if projected_bytes.saturating_add(encoded_bytes) > MAX_AUTHORIZATION_VARIABLE_BYTES {
            break;
        }
        projected_bytes += encoded_bytes;
        projected.insert(name, value);
    }

    let extensions = payload.entry("extensions").or_insert_with(|| json!({}));
    let Some(extensions) = extensions.as_object_mut() else {
        return;
    };
    // The public gateway is the only writer trusted by the private Hive
    // endpoint. Replace client-supplied reserved metadata with a small scalar
    // projection. Large data variables remain only in the standard variables
    // object, while missing authorization values fail closed at analysis.
    extensions.insert(
        INTERNAL_SUBSCRIPTION_VARIABLES_EXTENSION.to_owned(),
        Value::Object(projected),
    );

    while message.to_string().len() > internal_max_bytes {
        let Some(projected) = message
            .get_mut("payload")
            .and_then(Value::as_object_mut)
            .and_then(|payload| payload.get_mut("extensions"))
            .and_then(Value::as_object_mut)
            .and_then(|extensions| extensions.get_mut(INTERNAL_SUBSCRIPTION_VARIABLES_EXTENSION))
            .and_then(Value::as_object_mut)
        else {
            break;
        };
        let Some(name) = projected.keys().next_back().cloned() else {
            message
                .get_mut("payload")
                .and_then(Value::as_object_mut)
                .and_then(|payload| payload.get_mut("extensions"))
                .and_then(Value::as_object_mut)
                .map(|extensions| extensions.remove(INTERNAL_SUBSCRIPTION_VARIABLES_EXTENSION));
            break;
        };
        projected.remove(&name);
    }
}

async fn forward_complete(
    mut message: Value,
    _raw: &str,
    state: SharedConnectionState,
) -> Option<ws::Message> {
    if state.borrow().phase != ConnectionPhase::Ready {
        return state
            .borrow_mut()
            .terminate_with_public_close(4401, "Connection is not acknowledged");
    }
    let Some(id) = message.get("id").and_then(Value::as_str) else {
        return state
            .borrow_mut()
            .terminate_with_public_close(4400, "Subscription ID is required");
    };
    let internal_id = state
        .borrow()
        .operations
        .get(id)
        .map(|operation| operation.internal_id.clone());
    state.borrow_mut().remove_operation(id);
    let internal_id = internal_id?;
    message["id"] = Value::String(internal_id);
    let internal = state.borrow().internal_sink.clone();
    if let Some(internal) = internal
        && internal
            .send(ws::Message::Text(message.to_string().into()))
            .await
            .is_ok()
    {
        return None;
    }
    state
        .borrow_mut()
        .terminate_with_public_close(1011, "Subscription transport unavailable")
}

fn operation_limit_error(id: &str) -> ws::Message {
    ws::Message::Text(
        json!({
            "type": "error",
            "id": id,
            "payload": [{
                "message": "WebSocket operation limit exceeded",
                "extensions": {"code": "SUBSCRIPTION_LIMIT_EXCEEDED"}
            }]
        })
        .to_string()
        .into(),
    )
}

fn close_message(code: u16, description: &str) -> ws::Message {
    ws::Message::Close(Some(ws::CloseReason {
        code: code.into(),
        description: Some(description.to_owned()),
    }))
}

fn continuity_error(id: &str) -> Value {
    json!({"id": id, "type": "error", "payload": [{
        "message": "Subscription continuity lost; resubscribe and refetch",
        "extensions": {"code": "SUBSCRIPTION_CONTINUITY_LOST"}
    }]})
}

fn spawn_continuity_watch(
    state: Weak<RefCell<ConnectionState>>,
    sink: ws::WsSink,
    mut receiver: hive_router::tokio::sync::watch::Receiver<u64>,
) {
    let Some(initial) = state.upgrade() else {
        return;
    };
    let mut local = initial.borrow().local_loss.subscribe();
    drop(initial);
    rt::spawn(async move {
        loop {
            hive_router::tokio::select! {
                result = local.changed() => if result.is_err() { break; },
                result = receiver.changed() => if result.is_err() { break; },
                _ = sink.on_disconnect() => break,
            }
            let Some(state) = state.upgrade() else {
                break;
            };
            receiver.borrow_and_update();
            local.borrow_and_update();
            let (ids, internal) = {
                let mut state = state.borrow_mut();
                if state.phase == ConnectionPhase::Closed {
                    break;
                }
                let ids = state
                    .operations
                    .iter()
                    .filter(|(_, operation)| state.operation_lost(operation))
                    .map(|(id, operation)| (id.clone(), operation.internal_id.clone()))
                    .collect::<Vec<_>>();
                for (id, _) in &ids {
                    state.remove_operation(id);
                }
                (ids, state.internal_sink.clone())
            };
            for (id, internal_id) in ids {
                if sink
                    .send(ws::Message::Text(continuity_error(&id).to_string().into()))
                    .await
                    .is_err()
                {
                    state.borrow_mut().terminate_and_close_internal();
                    return;
                }
                if let Some(internal) = &internal {
                    let complete = json!({"id": internal_id, "type": "complete"});
                    let _ = internal
                        .send(ws::Message::Text(complete.to_string().into()))
                        .await;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct UnusedAuthentication;

    impl AuthenticationProvider for UnusedAuthentication {
        fn authenticate_bearer(
            &self,
            _: &str,
        ) -> Result<crate::AuthenticatedPrincipal, crate::AuthenticationError> {
            unreachable!("frame translation does not authenticate")
        }
    }

    fn operation_test_state(gateway: Arc<SubscriptionGateway>) -> SharedConnectionState {
        let mut operations = BTreeMap::new();
        for (id, query) in [
            ("sub", "subscription { changed }"),
            ("sibling", "subscription { other }"),
            ("query", "query { value }"),
            ("mutation", "mutation { update }"),
        ] {
            gateway.add_operation();
            operations.insert(
                id.into(),
                Operation {
                    internal_id: id.into(),
                    epoch: gateway.internal.continuity.epoch(),
                    subscription: selected_subscription(&json!({"payload": {"query": query}}))
                        .unwrap(),
                    lost: false,
                },
            );
        }
        Rc::new(RefCell::new(ConnectionState {
            gateway,
            phase: ConnectionPhase::Ready,
            internal_sink: None,
            operations,
            next_operation: 0,
            permit: None,
            local_loss: hive_router::tokio::sync::watch::channel(0).0,
        }))
    }

    fn test_gateway() -> Arc<SubscriptionGateway> {
        SubscriptionGateway::new(
            SubscriptionConfig::default(),
            Arc::new(UnusedAuthentication),
            InternalSubscriptionEndpoint::generate().unwrap(),
            "127.0.0.1:12345".parse().unwrap(),
            Arc::new(RouterMetrics::default()),
        )
    }

    fn next_message(id: &str, state: &SharedConnectionState) -> Value {
        serde_json::from_str(
            &public_server_message(
                json!({"id": id, "type": "next", "payload": {"data": {"value": 1}}})
                    .to_string()
                    .as_bytes(),
                state,
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn duplicate_id_closes_before_validation_or_other_operation_errors() {
        for (query, at_capacity, cooling_down, exhausted) in [
            (Some("subscription {"), false, false, false),
            (
                Some("query A { value } subscription B { changed }"),
                false,
                false,
                false,
            ),
            (None, false, false, false),
            (Some("subscription { changed }"), true, false, false),
            (Some("subscription { changed }"), false, true, false),
            (Some("subscription { changed }"), false, false, true),
        ] {
            let mut gateway = test_gateway();
            if at_capacity {
                Arc::get_mut(&mut gateway)
                    .unwrap()
                    .config
                    .max_operations_per_connection = 4;
            }
            let state = operation_test_state(gateway.clone());
            // Reproduce a live subscription before sending the duplicate.
            assert_eq!(next_message("sub", &state)["type"], "next");
            if cooling_down {
                gateway.internal.continuity.lost();
            }
            if exhausted {
                state.borrow_mut().next_operation = u64::MAX;
            }
            let mut duplicate = json!({"type": "subscribe", "id": "sub", "payload": {}});
            if let Some(query) = query {
                duplicate["payload"]["query"] = json!(query);
            }
            let result =
                futures::executor::block_on(forward_subscribe(duplicate, "", state.clone()));
            let Some(ws::Message::Close(Some(reason))) = result else {
                panic!("duplicate must close with 4409, even for {query:?}: {result:?}");
            };
            assert_eq!(u16::from(reason.code), 4409);
            assert_eq!(
                reason.description.as_deref(),
                Some("Subscriber already exists")
            );
            assert_eq!(state.borrow().phase, ConnectionPhase::Closed);
            assert!(state.borrow().operations.is_empty());
            assert_eq!(gateway.active_operations.load(Ordering::Acquire), 0);
            for kind in ["next", "error", "complete"] {
                let late = json!({"id": "sub", "type": kind});
                assert!(
                    public_server_message(late.to_string().as_bytes(), &state).is_none(),
                    "late {kind} after duplicate close"
                );
            }
        }
    }

    #[test]
    fn bounded_queue_overflow_only_retires_the_losing_subscription() {
        let gateway = test_gateway();
        let state = operation_test_state(gateway.clone());
        let other = operation_test_state(gateway.clone());
        let (tx, mut rx) = hive_router::tokio::sync::mpsc::channel(1);
        for _ in 0..2 {
            let frame = ws::Frame::Text(json!({"id": "sub", "type": "next"}).to_string().into());
            assert!(enqueue_subscription_frame(&tx, frame, &state).is_none());
        }
        assert!(state.borrow().operations["sub"].lost);
        assert_eq!(
            gateway.internal.continuity.epoch(),
            0,
            "local overflow must not publish runtime loss"
        );
        assert_eq!(next_message("sub", &state), continuity_error("sub"));
        for id in ["sibling", "query", "mutation"] {
            assert_eq!(next_message(id, &state)["type"], "next");
        }
        for id in ["sub", "sibling", "query", "mutation"] {
            assert_eq!(next_message(id, &other)["type"], "next");
        }
        for id in ["query", "mutation"] {
            let frame = ws::Frame::Text(json!({"id": id, "type": "next"}).to_string().into());
            assert!(
                enqueue_subscription_frame(&tx, frame, &state).is_some(),
                "one-shot results bypass a full subscription queue"
            );
        }
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn connection_pressure_spares_one_shots_and_other_connections() {
        let gateway = test_gateway();
        let state = operation_test_state(gateway.clone());
        let other = operation_test_state(gateway.clone());
        state.borrow_mut().lose_connection_subscriptions();
        for id in ["sub", "sibling"] {
            assert_eq!(next_message(id, &state), continuity_error(id));
        }
        for id in ["query", "mutation"] {
            assert_eq!(next_message(id, &state)["type"], "next");
        }
        for id in ["sub", "sibling", "query", "mutation"] {
            assert_eq!(next_message(id, &other)["type"], "next");
        }
        assert_eq!(gateway.internal.continuity.epoch(), 0);
    }

    #[test]
    fn hive_fallback_only_retires_subscriptions_on_each_connection() {
        let gateway = test_gateway();
        let states = [
            operation_test_state(gateway.clone()),
            operation_test_state(gateway.clone()),
        ];
        gateway.internal.continuity.lost();
        for state in states {
            for id in ["sub", "sibling"] {
                assert_eq!(next_message(id, &state), continuity_error(id));
            }
            for id in ["query", "mutation"] {
                assert_eq!(next_message(id, &state)["type"], "next");
                let complete: Value = serde_json::from_str(
                    &public_server_message(
                        json!({"id": id, "type": "complete"}).to_string().as_bytes(),
                        &state,
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(complete["type"], "complete");
            }
        }
    }

    #[test]
    fn operation_selection_uses_ast_and_operation_name() {
        let query = "fragment S on Query { value } subscription Watch { changed } query Read { ...S } mutation Write { update }";
        for (name, expected) in [("Watch", true), ("Read", false), ("Write", false)] {
            assert_eq!(
                selected_subscription(&json!({"payload": {"query": query, "operationName": name}})),
                Some(expected)
            );
        }
        assert_eq!(
            selected_subscription(&json!({"payload": {"query": query}})),
            None
        );
        assert_eq!(
            selected_subscription(&json!({"payload": {"query": "{ value }"}})),
            Some(false)
        );
    }

    #[test]
    fn terminal_error_releases_state_and_drops_late_frames_after_id_reuse() {
        assert_terminal_error(false);
    }

    #[test]
    fn continuity_loss_replaces_complete_and_drops_late_frames_after_id_reuse() {
        assert_terminal_error(true);
    }

    fn assert_terminal_error(continuity_lost: bool) {
        let gateway = SubscriptionGateway::new(
            SubscriptionConfig::default(),
            Arc::new(UnusedAuthentication),
            InternalSubscriptionEndpoint::generate().unwrap(),
            "127.0.0.1:12345".parse().unwrap(),
            Arc::new(RouterMetrics::default()),
        );
        let state = Rc::new(RefCell::new(ConnectionState {
            gateway: gateway.clone(),
            phase: ConnectionPhase::Ready,
            internal_sink: None,
            operations: BTreeMap::from([(
                "client".into(),
                Operation {
                    internal_id: "1".into(),
                    epoch: 0,
                    subscription: true,
                    lost: false,
                },
            )]),
            next_operation: 1,
            local_loss: hive_router::tokio::sync::watch::channel(0).0,
            permit: None,
        }));
        gateway.add_operation();
        if continuity_lost {
            gateway.internal.continuity.lost();
        }
        let error = if continuity_lost {
            json!({"id": "1", "type": "complete"})
        } else {
            json!({"id": "1", "type": "error", "payload": []})
        };
        let forwarded: Value = serde_json::from_str(
            &public_server_message(error.to_string().as_bytes(), &state).unwrap(),
        )
        .unwrap();
        assert_eq!(forwarded["id"], "client");
        if continuity_lost {
            assert_eq!(forwarded, continuity_error("client"));
        } else {
            assert_eq!(forwarded["type"], "error");
        }
        assert!(state.borrow().operations.is_empty());
        assert_eq!(gateway.active_operations.load(Ordering::Acquire), 0);

        state.borrow_mut().operations.insert(
            "client".into(),
            Operation {
                internal_id: "2".into(),
                epoch: gateway.internal.continuity.epoch(),
                subscription: true,
                lost: false,
            },
        );
        gateway.add_operation();
        for kind in ["next", "error", "complete"] {
            let late = json!({"id": "1", "type": kind});
            assert!(public_server_message(late.to_string().as_bytes(), &state).is_none());
        }
        assert_eq!(gateway.active_operations.load(Ordering::Acquire), 1);
        let complete = json!({"id": "2", "type": "complete"});
        let forwarded: Value = serde_json::from_str(
            &public_server_message(complete.to_string().as_bytes(), &state).unwrap(),
        )
        .unwrap();
        assert_eq!(forwarded["type"], "complete");
        assert_eq!(gateway.active_operations.load(Ordering::Acquire), 0);
    }

    #[test]
    fn connection_init_accepts_only_one_string_bearer_location() {
        let top = json!({
            "type": "connection_init",
            "payload": {"Authorization": "Bearer token"}
        });
        assert_eq!(connection_init_authorization(&top), Some("Bearer token"));
        let nested = json!({
            "type": "connection_init",
            "payload": {"headers": {"authorization": "Bearer nested"}}
        });
        assert_eq!(
            connection_init_authorization(&nested),
            Some("Bearer nested")
        );
        assert!(
            connection_init_authorization(&json!({
                "type": "connection_init",
                "payload": {"authorization": 7}
            }))
            .is_none()
        );
        assert!(
            connection_init_authorization(&json!({
                "type": "connection_init",
                "payload": {
                    "authorization": "Bearer one",
                    "Authorization": "Bearer two"
                }
            }))
            .is_none()
        );
    }

    #[test]
    fn internal_endpoint_is_random_and_redacted() {
        let first = InternalSubscriptionEndpoint::generate().unwrap();
        let second = InternalSubscriptionEndpoint::generate().unwrap();
        assert_ne!(first.path, second.path);
        assert!(first.authorizes("/graphql", None));
        assert!(!first.authorizes(&first.path, None));
        assert!(first.authorizes(&first.path, Some(&first.secret)));
        let debug = format!("{first:?}");
        assert!(!debug.contains(&first.secret));
        assert!(!debug.contains(&first.path));
    }

    #[test]
    fn internal_endpoint_path_reveals_nothing_about_its_secret() {
        let endpoint = InternalSubscriptionEndpoint::generate().unwrap();
        let segment = endpoint
            .path
            .strip_prefix("/_graphql-orm-router/")
            .and_then(|rest| rest.strip_suffix("/websocket"))
            .unwrap();
        assert_eq!(segment.len(), 32);
        for window in 12..=segment.len() {
            for start in 0..=segment.len() - window {
                assert!(
                    !endpoint.secret.contains(&segment[start..start + window]),
                    "the private path must be independent of the secret"
                );
            }
        }
        let wrong_length = &endpoint.secret[..endpoint.secret.len() - 1];
        assert!(!endpoint.authorizes(&endpoint.path, Some(wrong_length)));
        let mut flipped = endpoint.secret.clone().into_bytes();
        flipped[0] = if flipped[0] == b'0' { b'1' } else { b'0' };
        assert!(!endpoint.authorizes(&endpoint.path, Some(std::str::from_utf8(&flipped).unwrap())));
        assert!(!endpoint.authorizes(&endpoint.path, Some(segment)));
        assert!(endpoint.authorizes(&endpoint.path, Some(&endpoint.secret)));
    }

    #[test]
    fn subscription_gateway_replaces_reserved_metadata_with_bounded_scalars() {
        let mut message = json!({
            "id": "operation",
            "type": "subscribe",
            "payload": {
                "query": "subscription ($Id: ID!) { event(id: $Id) }",
                "variables": {"Id": "actual"},
                "extensions": {
                    INTERNAL_SUBSCRIPTION_VARIABLES_EXTENSION: {"Id": "spoofed"},
                    "client": true
                }
            }
        });

        inject_operation_variables(&mut message, 64 * 1024);

        assert_eq!(message["payload"]["variables"], json!({"Id": "actual"}));
        assert_eq!(
            message["payload"]["extensions"][INTERNAL_SUBSCRIPTION_VARIABLES_EXTENSION],
            json!({"Id": "actual"})
        );
        assert_eq!(message["payload"]["extensions"]["client"], true);
    }

    #[test]
    fn subscription_gateway_keeps_large_variable_payload_below_the_public_limit() {
        let mut message = json!({
            "id": "operation",
            "type": "subscribe",
            "payload": {
                "query": "subscription ($Content: String!) { event(content: $Content) }",
                "variables": {"Content": "x".repeat(48 * 1024)},
                "extensions": {
                    INTERNAL_SUBSCRIPTION_VARIABLES_EXTENSION: {"Content": "spoofed"}
                }
            }
        });
        let public_size = message.to_string().len();

        inject_operation_variables(&mut message, 64 * 1024);

        let internal_size = message.to_string().len();
        assert!(public_size < 64 * 1024);
        assert_eq!(
            message["payload"]["variables"]["Content"]
                .as_str()
                .unwrap()
                .len(),
            48 * 1024
        );
        assert!(
            message["payload"]["extensions"][INTERNAL_SUBSCRIPTION_VARIABLES_EXTENSION]
                .as_object()
                .is_some_and(serde_json::Map::is_empty)
        );
        assert!(internal_size < 64 * 1024);
    }

    #[test]
    fn connection_attempt_limiter_contains_churn_and_refills_gradually() {
        let start = Instant::now();
        let limiter = ConnectionAttemptLimiter::new_at(2, start);

        assert!(limiter.try_acquire_at(start));
        assert!(limiter.try_acquire_at(start));
        assert!(!limiter.try_acquire_at(start));
        assert!(limiter.try_acquire_at(start + std::time::Duration::from_millis(500)));
        assert!(!limiter.try_acquire_at(start + std::time::Duration::from_millis(500)));
        assert!(limiter.try_acquire_at(start + std::time::Duration::from_secs(1)));
    }
}
