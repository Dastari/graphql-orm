use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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
    active_subscriptions: usize,
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
        "Current public GraphQL WebSocket connections."
    );
    snapshot_getter!(
        active_subscriptions,
        active_subscriptions,
        usize,
        "Current public WebSocket subscription operations."
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
    active_subscriptions: AtomicUsize,
    schema_refresh_total: AtomicU64,
    composition_success_total: AtomicU64,
    composition_failure_total: AtomicU64,
    rejected_subgraphs_total: AtomicU64,
    authorization_denied_total: AtomicU64,
}

impl RouterMetrics {
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
            active_subscriptions: self.active_subscriptions.load(Ordering::Relaxed),
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

fn decrement(counter: &AtomicUsize, count: usize) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_sub(count))
    });
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
        assert_eq!(metrics.snapshot().active_websocket_connections(), 2);
        assert_eq!(metrics.snapshot().active_subscriptions(), 2);

        metrics.websocket_disconnected();
        metrics.websocket_disconnected();
        metrics.subscriptions_ended(2);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.active_websocket_connections(), 0);
        assert_eq!(snapshot.active_subscriptions(), 0);
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
