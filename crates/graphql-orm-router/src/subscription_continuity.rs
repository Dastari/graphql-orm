//! The pinned Hive adapter reports both executor overflow and broadcast lag via
//! synchronous OTel counters. Observe those calls, independently of exporters.
//! They carry no operation ID: conservatively invalidate every active operation
//! on this runtime rather than risk attributing a loss to the wrong subscriber.
use std::sync::Arc;

use hive_router::{
    telemetry::{
        TelemetryContext,
        metrics::{Metrics, catalog::names, subscription_metrics::SubscriptionMetrics},
    },
    tokio::sync::watch,
};
use opentelemetry::{
    KeyValue,
    metrics::{
        Counter, InstrumentBuilder, InstrumentProvider, Meter, SyncInstrument, UpDownCounter,
    },
};

#[derive(Clone, Debug)]
pub(crate) struct Continuity(watch::Sender<u64>);

impl Default for Continuity {
    fn default() -> Self {
        Self(watch::channel(0).0)
    }
}

impl Continuity {
    pub(crate) fn lost(&self) {
        self.0.send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }
    pub(crate) fn epoch(&self) -> u64 {
        *self.0.borrow()
    }
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.subscribe()
    }
    pub(crate) fn install(&self, context: &mut TelemetryContext) {
        let meter = Meter::new(Arc::new(LossInstruments {
            continuity: self.clone(),
            delegate: context.meter().cloned(),
        }));
        let mut metrics = Metrics::new(context.meter());
        metrics.subscriptions = SubscriptionMetrics::new(Some(&meter));
        context.metrics = Arc::new(metrics);
    }
}

struct LossInstruments {
    continuity: Continuity,
    delegate: Option<Meter>,
}
impl InstrumentProvider for LossInstruments {
    fn u64_counter(&self, builder: InstrumentBuilder<'_, Counter<u64>>) -> Counter<u64> {
        let loss = matches!(
            builder.name.as_ref(),
            names::SUBSCRIPTIONS_SUBGRAPHS_DROPPED_MESSAGES_TOTAL
                | names::SUBSCRIPTIONS_CLIENTS_LAGGED_MESSAGES_TOTAL
        );
        let delegate = self.delegate.as_ref().map(|meter| {
            let mut counter = meter.u64_counter(builder.name);
            if let Some(description) = builder.description {
                counter = counter.with_description(description);
            }
            if let Some(unit) = builder.unit {
                counter = counter.with_unit(unit);
            }
            counter.build()
        });
        Counter::new(Arc::new(LossCounter {
            continuity: loss.then(|| self.continuity.clone()),
            delegate,
        }))
    }
    fn i64_up_down_counter(
        &self,
        builder: InstrumentBuilder<'_, UpDownCounter<i64>>,
    ) -> UpDownCounter<i64> {
        let meter = self
            .delegate
            .clone()
            .unwrap_or_else(|| Meter::new(Arc::new(NoMetrics)));
        let mut counter = meter.i64_up_down_counter(builder.name);
        if let Some(description) = builder.description {
            counter = counter.with_description(description);
        }
        if let Some(unit) = builder.unit {
            counter = counter.with_unit(unit);
        }
        counter.build()
    }
}
struct NoMetrics;
impl InstrumentProvider for NoMetrics {}
struct LossCounter {
    continuity: Option<Continuity>,
    delegate: Option<Counter<u64>>,
}
impl SyncInstrument<u64> for LossCounter {
    fn measure(&self, value: u64, attributes: &[KeyValue]) {
        if value != 0
            && let Some(continuity) = &self.continuity
        {
            continuity.lost();
        }
        if let Some(delegate) = &self.delegate {
            delegate.add(value, attributes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_router::{
        executor::executors::subscription_buffer::{SendOutcome, try_send_or_drop},
        telemetry::metrics::subscription_metrics::SubscriptionTransport,
    };

    #[test]
    fn pinned_executor_overflow_and_broadcast_lag_signal_without_exporters() {
        let continuity = Continuity::default();
        let mut context =
            TelemetryContext::from_propagation_config(&Default::default(), &Default::default());
        continuity.install(&mut context);
        let (tx, _rx) = hive_router::tokio::sync::mpsc::channel(1);
        assert!(matches!(
            try_send_or_drop(
                &tx,
                (),
                &context,
                SubscriptionTransport::HttpSse,
                "secret-subgraph",
                "secret-url"
            ),
            SendOutcome::Sent
        ));
        assert_eq!(continuity.epoch(), 0);
        assert!(matches!(
            try_send_or_drop(
                &tx,
                (),
                &context,
                SubscriptionTransport::HttpSse,
                "secret-subgraph",
                "secret-url"
            ),
            SendOutcome::Dropped
        ));
        assert_eq!(continuity.epoch(), 1);
        context
            .metrics
            .subscriptions
            .record_client_lag(SubscriptionTransport::WebSocket, 7);
        assert_eq!(continuity.epoch(), 2);
        context
            .metrics
            .subscriptions
            .record_client_sent(SubscriptionTransport::WebSocket);
        assert_eq!(continuity.epoch(), 2);
    }
}
