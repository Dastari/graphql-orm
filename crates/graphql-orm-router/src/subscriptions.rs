use std::{
    cell::RefCell,
    collections::BTreeSet,
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

#[derive(Clone)]
pub(crate) struct InternalSubscriptionEndpoint {
    pub(crate) path: String,
    secret: String,
}

impl InternalSubscriptionEndpoint {
    pub(crate) fn generate() -> Result<Self, RouterError> {
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|_| {
            RouterError::new(
                RouterErrorKind::Runtime,
                "failed to initialize the private subscription transport",
            )
        })?;
        let secret = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(Self {
            path: format!("/_graphql-orm-router/{}/websocket", &secret[..32]),
            secret,
        })
    }

    pub(crate) fn authorizes(&self, path: &str, supplied: Option<&str>) -> bool {
        path != self.path || supplied.is_some_and(|supplied| supplied == self.secret)
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
    active_connections: AtomicUsize,
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
            active_operations: AtomicUsize::new(0),
            connection_attempts,
            metrics,
        })
    }

    fn try_reserve_connection_attempt(&self) -> bool {
        self.connection_attempts.try_acquire()
    }

    fn try_reserve_connection(self: &Arc<Self>) -> Option<ConnectionPermit> {
        let mut current = self.active_connections.load(Ordering::Acquire);
        loop {
            if current >= self.config.max_connections {
                return None;
            }
            match self.active_connections.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.metrics.websocket_connected();
                    return Some(ConnectionPermit {
                        gateway: self.clone(),
                    });
                }
                Err(observed) => current = observed,
            }
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

struct ConnectionPermit {
    gateway: Arc<SubscriptionGateway>,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.gateway
            .active_connections
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
    operations: BTreeSet<String>,
    permit: Option<ConnectionPermit>,
}

struct ConnectionTermination {
    first: bool,
    internal_sink: Option<ws::WsSink>,
}

impl ConnectionState {
    fn remove_operation(&mut self, id: &str) {
        if self.operations.remove(id) {
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
    let Some(permit) = gateway.try_reserve_connection() else {
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
        operations: BTreeSet::new(),
        permit: Some(permit),
    }));

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
    let now = gateway.authentication.current_time();
    if expires_at <= now {
        return state
            .borrow_mut()
            .terminate_with_public_close(4401, "Bearer credential expired");
    }
    state.borrow_mut().phase = ConnectionPhase::Connecting;
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
    let receiver = connection.seal().receiver();
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
    receiver: hive_router::ntex::channel::mpsc::Receiver<
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
                observe_server_message(&bytes, &state);
                let Ok(text) = String::from_utf8(bytes.to_vec()) else {
                    break;
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

fn observe_server_message(bytes: &[u8], state: &SharedConnectionState) {
    let Ok(message) = serde_json::from_slice::<Value>(bytes) else {
        return;
    };
    match message.get("type").and_then(Value::as_str) {
        Some("connection_ack") => {
            let mut state = state.borrow_mut();
            if state.phase == ConnectionPhase::Connecting {
                state.phase = ConnectionPhase::Ready;
            }
        }
        Some("error" | "complete") => {
            if let Some(id) = message.get("id").and_then(Value::as_str) {
                state.borrow_mut().remove_operation(id);
            }
        }
        _ => {}
    }
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
        if state.operations.contains(&id) {
            return state.terminate_with_public_close(4409, "Subscriber already exists");
        }
        if state.operations.len() >= state.gateway.config.max_operations_per_connection {
            return Some(operation_limit_error(&id));
        }
        state.operations.insert(id.clone());
        state.gateway.add_operation();
    }
    inject_operation_variables(&mut message);
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

fn inject_operation_variables(message: &mut Value) {
    let Some(payload) = message.get_mut("payload").and_then(Value::as_object_mut) else {
        return;
    };
    let variables = payload
        .get("variables")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let extensions = payload.entry("extensions").or_insert_with(|| json!({}));
    let Some(extensions) = extensions.as_object_mut() else {
        return;
    };
    // The public gateway is the only writer trusted by the private Hive
    // endpoint. Always replace a client-supplied value for this reserved key.
    extensions.insert(
        INTERNAL_SUBSCRIPTION_VARIABLES_EXTENSION.to_owned(),
        variables,
    );
}

async fn forward_complete(
    message: Value,
    raw: &str,
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
    state.borrow_mut().remove_operation(id);
    let internal = state.borrow().internal_sink.clone();
    if let Some(internal) = internal
        && internal
            .send(ws::Message::Text(raw.to_owned().into()))
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn subscription_gateway_overwrites_reserved_variable_metadata() {
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

        inject_operation_variables(&mut message);

        assert_eq!(
            message["payload"]["extensions"][INTERNAL_SUBSCRIPTION_VARIABLES_EXTENSION],
            json!({"Id": "actual"})
        );
        assert_eq!(message["payload"]["extensions"]["client"], true);
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
