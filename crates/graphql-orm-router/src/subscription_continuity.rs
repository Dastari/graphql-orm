//! The pinned Hive adapter reports both executor overflow and broadcast lag via
//! synchronous OTel counters. Observe those calls, independently of exporters.
//! They carry only transport identity: invalidate active subscriptions, never
//! one-shot work. Coalesce sustained losses and gate subscription admission until
//! one second of quiet, so suppressed notifications cannot hide a new loss.
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

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
pub(crate) struct Continuity {
    epochs: watch::Sender<u64>,
    last_loss: Arc<Mutex<Option<Instant>>>,
}

const LOSS_QUIET_PERIOD: Duration = Duration::from_secs(1);

impl Default for Continuity {
    fn default() -> Self {
        Self {
            epochs: watch::channel(0).0,
            last_loss: Default::default(),
        }
    }
}

impl Continuity {
    pub(crate) fn lost(&self) {
        self.lost_at(Instant::now());
    }
    fn lost_at(&self, now: Instant) {
        let mut last = self
            .last_loss
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if last.is_none_or(|previous| now.saturating_duration_since(previous) >= LOSS_QUIET_PERIOD)
        {
            self.epochs
                .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        }
        // Concurrent callers may acquire the mutex in a different order from
        // when they sampled the clock. Never shorten the quiet interval.
        *last = Some(last.map_or(now, |previous| previous.max(now)));
    }
    pub(crate) fn subscription_epoch(&self) -> Option<u64> {
        let last = self
            .last_loss
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if last.is_some_and(|last| last.elapsed() < LOSS_QUIET_PERIOD) {
            None
        } else {
            Some(self.epoch())
        }
    }
    pub(crate) fn epoch(&self) -> u64 {
        *self.epochs.borrow()
    }
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.epochs.subscribe()
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
        let builder_name_is_client_lag =
            builder.name.as_ref() == names::SUBSCRIPTIONS_CLIENTS_LAGGED_MESSAGES_TOTAL;
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
            client_lag: builder_name_is_client_lag,
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
    client_lag: bool,
    continuity: Option<Continuity>,
    delegate: Option<Counter<u64>>,
}
impl SyncInstrument<u64> for LossCounter {
    fn measure(&self, value: u64, attributes: &[KeyValue]) {
        // HTTP receiver lag cannot skip frames for a WebSocket receiver.
        let relevant = !self.client_lag
            || attributes.iter().any(|attribute| {
                attribute.key.as_str() == "subscription.transport"
                    && attribute.value.as_str() == "websocket"
            });
        if relevant
            && value != 0
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
    fn http_client_lag_does_not_invalidate_websocket_subscribers() {
        let continuity = Continuity::default();
        let mut context =
            TelemetryContext::from_propagation_config(&Default::default(), &Default::default());
        continuity.install(&mut context);
        context
            .metrics
            .subscriptions
            .record_client_lag(SubscriptionTransport::HttpSse, 3);
        assert_eq!(continuity.epoch(), 0);
        context
            .metrics
            .subscriptions
            .record_client_lag(SubscriptionTransport::WebSocket, 3);
        assert_eq!(continuity.epoch(), 1);
    }

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
        assert_eq!(continuity.epoch(), 1);
        context
            .metrics
            .subscriptions
            .record_client_sent(SubscriptionTransport::WebSocket);
        assert_eq!(continuity.epoch(), 1);
    }
}

#[cfg(test)]
mod debounce_tests {
    use super::*;
    #[test]
    fn sustained_loss_is_coalesced_until_a_full_quiet_period() {
        let continuity = Continuity::default();
        let now = Instant::now();
        continuity.lost_at(now);
        for offset in 1..1000 {
            continuity.lost_at(now + Duration::from_millis(offset));
        }
        assert_eq!(continuity.epoch(), 1);
        assert!(continuity.subscription_epoch().is_none());
        // Admission reopens only after quiet, retaining the epoch so operations
        // that predate the loss cannot be mistaken for new streams.
        *continuity.last_loss.lock().unwrap() = Some(now - Duration::from_secs(2));
        assert_eq!(continuity.subscription_epoch(), Some(1));
        continuity.lost_at(now + Duration::from_secs(2));
        assert_eq!(continuity.epoch(), 2);
    }
}
