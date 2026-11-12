use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

/// Process-local router metric values suitable for an authenticated status
/// surface or an embedding application's metrics adapter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RouterMetricsSnapshot {
    graphql_requests_total: u64,
    graphql_errors_total: u64,
    subgraph_requests_total: u64,
    subgraph_errors_total: u64,
    subgraph_latency_microseconds_total: u64,
    active_graph_version: u64,
    active_websocket_connections: usize,
    pending_websocket_connections: usize,
    active_subscriptions: usize,
    websocket_rejections_total: u64,
    schema_refresh_total: u64,
    composition_success_total: u64,
    composition_failure_total: u64,
    rejected_subgraphs_total: u64,
    authorization_denied_total: u64,
}

macro_rules! snapshot_getter {
    ($name:ident, $field:ident, $type:ty, $doc:literal) => {
        #[doc = $doc]
        pub fn $name(&self) -> $type {
            self.$field
        }
    };
}

impl RouterMetricsSnapshot {
    snapshot_getter!(
        active_graph_version,
        active_graph_version,
        u64,
        "Current process-local active graph version."
    );
    snapshot_getter!(
        graphql_requests_total,
        graphql_requests_total,
        u64,
        "Total GraphQL HTTP requests observed by the execution plugin."
    );
    snapshot_getter!(
        graphql_errors_total,
        graphql_errors_total,
        u64,
        "Total GraphQL errors observed across those requests."
    );
    snapshot_getter!(
        subgraph_requests_total,
        subgraph_requests_total,
        u64,
        "Total HTTP requests sent through the subgraph execution hook."
    );
    snapshot_getter!(
        subgraph_errors_total,
        subgraph_errors_total,
        u64,
        "Total non-successful subgraph HTTP responses."
    );
    snapshot_getter!(
        subgraph_latency_microseconds_total,
        subgraph_latency_microseconds_total,
        u64,
        "Cumulative subgraph response latency in microseconds."
    );
    snapshot_getter!(
        active_websocket_connections,
        active_websocket_connections,
        usize,
        "Current authenticated public GraphQL WebSocket connections. A socket is counted here only after its `connection_init` credential validates."
    );
    snapshot_getter!(
        pending_websocket_connections,
        pending_websocket_connections,
        usize,
        "Current public GraphQL WebSocket connections that have not authenticated yet."
    );
    snapshot_getter!(
        active_subscriptions,
        active_subscriptions,
        usize,
        "Current public WebSocket subscription operations."
    );
    snapshot_getter!(
        websocket_rejections_total,
        websocket_rejections_total,
        u64,
        "Total public WebSocket upgrades rejected by an admission limit."
    );
    snapshot_getter!(
        schema_refresh_total,
        schema_refresh_total,
        u64,
        "Total schema refresh rounds, including manual refresh."
    );
    snapshot_getter!(
        composition_success_total,
        composition_success_total,
        u64,
        "Total complete candidates admitted, including startup."
    );
    snapshot_getter!(
        composition_failure_total,
        composition_failure_total,
        u64,
        "Total complete candidates rejected during graph construction."
    );
    snapshot_getter!(
        rejected_subgraphs_total,
        rejected_subgraphs_total,
        u64,
        "Total subgraph candidate rejection observations."
    );
    snapshot_getter!(
        authorization_denied_total,
        authorization_denied_total,
        u64,
        "Total router authorization denials."
    );
}

#[derive(Debug, Default)]
pub(crate) struct RouterMetrics {
    graphql_requests_total: AtomicU64,
    graphql_errors_total: AtomicU64,
    subgraph_requests_total: AtomicU64,
    subgraph_errors_total: AtomicU64,
    subgraph_latency_microseconds_total: AtomicU64,
    active_graph_version: AtomicU64,
    active_websocket_connections: AtomicUsize,
    pending_websocket_connections: AtomicUsize,
    active_subscriptions: AtomicUsize,
    websocket_rejections_total: AtomicU64,
    schema_refresh_total: AtomicU64,
    composition_success_total: AtomicU64,
    composition_failure_total: AtomicU64,
    rejected_subgraphs_total: AtomicU64,
    authorization_denied_total: AtomicU64,
    byte_routes: OnceLock<Vec<Arc<ByteRouteCounters>>>,
}

impl RouterMetrics {
    /// Installs one counter set per configured byte route, in route order.
    pub(crate) fn install_byte_routes(&self, prefixes: &[String]) -> Vec<Arc<ByteRouteCounters>> {
        self.byte_routes
            .get_or_init(|| {
                prefixes
                    .iter()
                    .map(|prefix| Arc::new(ByteRouteCounters::new(prefix.clone())))
                    .collect()
            })
            .clone()
    }

    pub(crate) fn byte_route_snapshots(&self) -> Vec<ByteRouteMetricsSnapshot> {
        self.byte_routes
            .get()
            .map(|routes| routes.iter().map(|route| route.snapshot()).collect())
            .unwrap_or_default()
    }

    pub(crate) fn snapshot(&self) -> RouterMetricsSnapshot {
        RouterMetricsSnapshot {
            graphql_requests_total: self.graphql_requests_total.load(Ordering::Relaxed),
            graphql_errors_total: self.graphql_errors_total.load(Ordering::Relaxed),
            subgraph_requests_total: self.subgraph_requests_total.load(Ordering::Relaxed),
            subgraph_errors_total: self.subgraph_errors_total.load(Ordering::Relaxed),
            subgraph_latency_microseconds_total: self
                .subgraph_latency_microseconds_total
                .load(Ordering::Relaxed),
            active_graph_version: self.active_graph_version.load(Ordering::Relaxed),
            active_websocket_connections: self.active_websocket_connections.load(Ordering::Relaxed),
            pending_websocket_connections: self
                .pending_websocket_connections
                .load(Ordering::Relaxed),
            active_subscriptions: self.active_subscriptions.load(Ordering::Relaxed),
            websocket_rejections_total: self.websocket_rejections_total.load(Ordering::Relaxed),
            schema_refresh_total: self.schema_refresh_total.load(Ordering::Relaxed),
            composition_success_total: self.composition_success_total.load(Ordering::Relaxed),
            composition_failure_total: self.composition_failure_total.load(Ordering::Relaxed),
            rejected_subgraphs_total: self.rejected_subgraphs_total.load(Ordering::Relaxed),
            authorization_denied_total: self.authorization_denied_total.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn graphql_request(&self) {
        self.graphql_requests_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn graphql_error(&self) {
        self.graphql_errors_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn subgraph_request(&self) {
        self.subgraph_requests_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn subgraph_response(&self, elapsed_microseconds: u64, successful: bool) {
        self.subgraph_latency_microseconds_total
            .fetch_add(elapsed_microseconds, Ordering::Relaxed);
        if !successful {
            self.subgraph_errors_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn websocket_connected(&self) {
        self.active_websocket_connections
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn websocket_disconnected(&self) {
        decrement(&self.active_websocket_connections, 1);
    }

    pub(crate) fn websocket_pending(&self) {
        self.pending_websocket_connections
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn websocket_pending_ended(&self) {
        decrement(&self.pending_websocket_connections, 1);
    }

    pub(crate) fn websocket_rejected(&self) {
        self.websocket_rejections_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn subscription_started(&self) {
        self.active_subscriptions.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn subscriptions_ended(&self, count: usize) {
        if count != 0 {
            decrement(&self.active_subscriptions, count);
        }
    }

    pub(crate) fn graph_activated(&self, version: u64) {
        self.active_graph_version.store(version, Ordering::Relaxed);
    }

    pub(crate) fn schema_refresh(&self) {
        self.schema_refresh_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn composition_success(&self) {
        self.composition_success_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn composition_failure(&self) {
        self.composition_failure_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn subgraphs_rejected(&self, rejected_subgraphs: usize) {
        self.rejected_subgraphs_total.fetch_add(
            u64::try_from(rejected_subgraphs).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    pub(crate) fn authorization_denied(&self, count: usize) {
        self.authorization_denied_total
            .fetch_add(u64::try_from(count).unwrap_or(u64::MAX), Ordering::Relaxed);
    }
}

/// Process-local counters for one byte route, labelled by its prefix.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ByteRouteMetricsSnapshot {
    prefix: String,
    requests_total: u64,
    rejected_total: u64,
    upstream_errors_total: u64,
    aborted_total: u64,
    bytes_total: u64,
    active_requests: usize,
}

impl ByteRouteMetricsSnapshot {
    /// The route's configured path prefix, used as its metric label.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    snapshot_getter!(
        requests_total,
        requests_total,
        u64,
        "Total requests claimed by the route, including refused ones."
    );
    snapshot_getter!(
        rejected_total,
        rejected_total,
        u64,
        "Total requests the router refused before contacting the upstream."
    );
    snapshot_getter!(
        upstream_errors_total,
        upstream_errors_total,
        u64,
        "Total requests answered with a router error after contacting the upstream: unreachable, timed out, or a declared size over the limit."
    );
    snapshot_getter!(
        aborted_total,
        aborted_total,
        u64,
        "Total responses whose body stream ended before completion."
    );
    snapshot_getter!(
        bytes_total,
        bytes_total,
        u64,
        "Total upstream response body bytes handed to clients."
    );
    snapshot_getter!(
        active_requests,
        active_requests,
        usize,
        "Current requests holding one of the route's concurrency slots."
    );
}

#[derive(Debug)]
pub(crate) struct ByteRouteCounters {
    prefix: String,
    requests_total: AtomicU64,
    rejected_total: AtomicU64,
    upstream_errors_total: AtomicU64,
    aborted_total: AtomicU64,
    bytes_total: AtomicU64,
    pub(crate) active_requests: AtomicUsize,
}

impl ByteRouteCounters {
    fn new(prefix: String) -> Self {
        Self {
            prefix,
            requests_total: AtomicU64::new(0),
            rejected_total: AtomicU64::new(0),
            upstream_errors_total: AtomicU64::new(0),
            aborted_total: AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
            active_requests: AtomicUsize::new(0),
        }
    }

    fn snapshot(&self) -> ByteRouteMetricsSnapshot {
        ByteRouteMetricsSnapshot {
            prefix: self.prefix.clone(),
            requests_total: self.requests_total.load(Ordering::Relaxed),
            rejected_total: self.rejected_total.load(Ordering::Relaxed),
            upstream_errors_total: self.upstream_errors_total.load(Ordering::Relaxed),
            aborted_total: self.aborted_total.load(Ordering::Relaxed),
            bytes_total: self.bytes_total.load(Ordering::Relaxed),
            active_requests: self.active_requests.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn request(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn rejected(&self) {
        self.rejected_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn upstream_error(&self) {
        self.upstream_errors_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn aborted(&self) {
        self.aborted_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn bytes(&self, count: usize) {
        self.bytes_total
            .fetch_add(u64::try_from(count).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// Takes one concurrency slot, or returns false when all are in use.
    pub(crate) fn try_acquire(&self, maximum: usize) -> bool {
        let mut current = self.active_requests.load(Ordering::Acquire);
        loop {
            if current >= maximum {
                return false;
            }
            match self.active_requests.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    pub(crate) fn release(&self) {
        decrement(&self.active_requests, 1);
    }
}

fn decrement(counter: &AtomicUsize, count: usize) {
    // Keep the atomic saturating update compatible with the Rust 1.90 MSRV;
    // current Rust deprecates fetch_update in favor of the newer try_update.
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        match counter.compare_exchange_weak(
            current,
            current.saturating_sub(count),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_and_subscription_gauges_return_to_baseline() {
        let metrics = RouterMetrics::default();
        metrics.websocket_connected();
        metrics.websocket_connected();
        metrics.subscription_started();
        metrics.subscription_started();
        metrics.websocket_rejected();
        assert_eq!(metrics.snapshot().active_websocket_connections(), 2);
        assert_eq!(metrics.snapshot().active_subscriptions(), 2);
        assert_eq!(metrics.snapshot().websocket_rejections_total(), 1);

        metrics.websocket_disconnected();
        metrics.websocket_disconnected();
        metrics.subscriptions_ended(2);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.active_websocket_connections(), 0);
        assert_eq!(snapshot.active_subscriptions(), 0);
    }

    #[test]
    fn pending_and_authenticated_websocket_gauges_are_separate() {
        let metrics = RouterMetrics::default();
        metrics.websocket_pending();
        metrics.websocket_pending();
        assert_eq!(metrics.snapshot().pending_websocket_connections(), 2);
        assert_eq!(metrics.snapshot().active_websocket_connections(), 0);
        metrics.websocket_connected();
        metrics.websocket_pending_ended();
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.pending_websocket_connections(), 1);
        assert_eq!(snapshot.active_websocket_connections(), 1);
        metrics.websocket_pending_ended();
        metrics.websocket_pending_ended();
        metrics.websocket_disconnected();
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.pending_websocket_connections(), 0);
        assert_eq!(snapshot.active_websocket_connections(), 0);
    }

    #[test]
    fn byte_route_counters_are_labelled_and_bounded() {
        let metrics = RouterMetrics::default();
        assert!(metrics.byte_route_snapshots().is_empty());
        let routes = metrics.install_byte_routes(&["/files".to_owned(), "/exports".to_owned()]);
        routes[0].request();
        routes[0].request();
        routes[0].rejected();
        routes[0].bytes(10);
        routes[1].upstream_error();
        routes[1].aborted();
        assert!(routes[1].try_acquire(1));
        assert!(!routes[1].try_acquire(1));
        let snapshots = metrics.byte_route_snapshots();
        assert_eq!(snapshots[0].prefix(), "/files");
        assert_eq!(snapshots[0].requests_total(), 2);
        assert_eq!(snapshots[0].rejected_total(), 1);
        assert_eq!(snapshots[0].bytes_total(), 10);
        assert_eq!(snapshots[1].prefix(), "/exports");
        assert_eq!(snapshots[1].upstream_errors_total(), 1);
        assert_eq!(snapshots[1].aborted_total(), 1);
        assert_eq!(snapshots[1].active_requests(), 1);
        routes[1].release();
        routes[1].release();
        assert_eq!(metrics.byte_route_snapshots()[1].active_requests(), 0);
    }

    #[test]
    fn gauges_never_wrap_and_graph_version_tracks_activation() {
        let metrics = RouterMetrics::default();
        metrics.websocket_disconnected();
        metrics.subscriptions_ended(usize::MAX);
        metrics.graph_activated(17);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.active_websocket_connections(), 0);
        assert_eq!(snapshot.active_subscriptions(), 0);
        assert_eq!(snapshot.active_graph_version(), 17);
    }
}
