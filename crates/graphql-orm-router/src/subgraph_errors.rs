use std::{
    collections::BTreeSet,
    num::NonZeroU32,
    sync::Mutex,
    time::{Duration, Instant},
};

use hive_router::{
    GraphQLError, sonic_rs::JsonValueTrait, telemetry::logging::request_id::REQUEST_IDENTIFIERS,
};
use serde::Deserialize;

const LOG_TARGET: &str = "graphql_orm_router::subgraph_errors";
const OPERATION_NAME_CORRELATION: &str = "client_operation_name";
const WINDOW: Duration = Duration::from_secs(1);
const MASKED_MESSAGE: &str = "Unexpected error";
const HANDLED_ERROR: &str = "graphqlOrmRouterSubgraphError";

/// Client-facing message policy for GraphQL errors returned by a downstream fetch.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum SubgraphErrorMasking {
    /// Keep the engine's existing masking and response format.
    #[default]
    All,
    /// Pass through every downstream GraphQL error message. For trusted deployments only.
    None,
    /// Pass through messages only when the subgraph supplied an exact matching error code.
    AllowlistByCode {
        /// Subgraph `extensions.code` values whose messages are safe to disclose.
        codes: BTreeSet<String>,
    },
}

impl SubgraphErrorMasking {
    pub(crate) fn prepare(&self, error: &mut GraphQLError) {
        let allow_message = match self {
            Self::All => return,
            Self::None => true,
            Self::AllowlistByCode { codes } => error
                .extensions
                .code
                .as_ref()
                .is_some_and(|code| codes.contains(code)),
        };
        // Carry the decision through Hive's entity error normalization, which
        // can clone the error. Replace any downstream value at this key.
        error.extensions.extensions.clear();
        error
            .extensions
            .extensions
            .insert(HANDLED_ERROR.to_owned(), allow_message.into());
    }

    pub(crate) fn finish(&self, error: &mut GraphQLError) -> bool {
        if *self == Self::All {
            return false;
        }
        let Some(allow_message) = error.extensions.extensions.remove(HANDLED_ERROR) else {
            return false;
        };
        if allow_message.as_bool() != Some(true) {
            error.message = MASKED_MESSAGE.to_owned();
        }
        // Hive masks errors with a service marker immediately after this
        // hook. The explicit policies own these messages and keep only code.
        error.extensions.service_name = None;
        error.extensions.affected_path = None;
        true
    }
}

/// How much of a downstream GraphQL error message to include in logs.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum SubgraphErrorLogMessage {
    /// Log the original message in full.
    Full,
    /// Log at most the first 256 Unicode characters.
    #[default]
    Truncated,
    /// Replace the message with a fixed placeholder.
    Redacted,
}

/// Per-router logging policy for downstream GraphQL errors.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct SubgraphErrorLogConfig {
    message: SubgraphErrorLogMessage,
    max_events_per_second: NonZeroU32,
}

impl Default for SubgraphErrorLogConfig {
    fn default() -> Self {
        Self {
            message: SubgraphErrorLogMessage::default(),
            max_events_per_second: NonZeroU32::new(20).unwrap(),
        }
    }
}

impl SubgraphErrorLogConfig {
    /// Creates the default policy of 20 events per second and truncated messages.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets how much of the downstream error message goes into the log.
    #[must_use]
    pub fn with_message(mut self, message: SubgraphErrorLogMessage) -> Self {
        self.message = message;
        self
    }

    /// Sets the shared limit on individual error events per one-second interval.
    #[must_use]
    pub fn with_max_events_per_second(mut self, maximum: NonZeroU32) -> Self {
        self.max_events_per_second = maximum;
        self
    }
}

pub(crate) fn record_client_operation_name(operation_name: Option<&str>) {
    if let Some(name) = operation_name.map(|name| prefix(name, 128))
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && tracing::enabled!(target: LOG_TARGET, tracing::Level::WARN)
    {
        // Hive's generic plugin context is shared by all operations on a
        // WebSocket. The identifiers are task-local to a single operation.
        let _ = REQUEST_IDENTIFIERS.try_with(|ids| {
            ids.set_correlation(OPERATION_NAME_CORRELATION, name);
        });
    }
}

pub(crate) struct SubgraphErrorLogger {
    config: SubgraphErrorLogConfig,
    window: Mutex<LogWindow>,
}

struct LogWindow {
    start: Instant,
    logged: u32,
    dropped: u64,
}

impl SubgraphErrorLogger {
    pub(crate) fn new(config: SubgraphErrorLogConfig) -> Self {
        Self {
            config,
            window: Mutex::new(LogWindow {
                start: Instant::now(),
                logged: 0,
                dropped: 0,
            }),
        }
    }

    pub(crate) fn log(&self, subgraph: &str, error: &GraphQLError) {
        if !tracing::enabled!(target: LOG_TARGET, tracing::Level::WARN) {
            return;
        }
        {
            let mut window = self.window.lock().expect("subgraph error log window");
            let now = Instant::now();
            if now.duration_since(window.start) >= WINDOW {
                window.start = now;
                window.logged = 0;
            }
            if window.logged == self.config.max_events_per_second.get() {
                window.dropped = window.dropped.saturating_add(1);
                return;
            }
            window.logged += 1;
        }

        let identifiers = REQUEST_IDENTIFIERS.try_with(std::sync::Arc::clone).ok();
        let operation_name = identifiers.as_ref().and_then(|ids| {
            ids.correlations
                .lock()
                .ok()?
                .get(OPERATION_NAME_CORRELATION)
                .cloned()
        });
        let message = match self.config.message {
            SubgraphErrorLogMessage::Full => &error.message,
            SubgraphErrorLogMessage::Truncated => prefix(&error.message, 256),
            SubgraphErrorLogMessage::Redacted => "[redacted]",
        };
        let response_path =
            serde_json::to_string(&error.path).expect("a GraphQL error path always serializes");
        tracing::warn!(
            target: LOG_TARGET,
            // Hive also adds `request_id` when it formats the event.
            graphql_request_id = identifiers.as_deref().map(|ids| ids.req_id()),
            operation_name = operation_name.as_deref(),
            subgraph,
            response_path = %response_path,
            "extensions.code" = error.extensions.code.as_deref(),
            subgraph_error_message = message,
            "downstream GraphQL error"
        );
    }

    pub(crate) async fn report_drops(&self) {
        let mut ticks = hive_router::tokio::time::interval(WINDOW);
        loop {
            ticks.tick().await;
            self.flush_drops();
        }
    }

    pub(crate) fn flush_drops(&self) {
        let dropped = std::mem::take(
            &mut self
                .window
                .lock()
                .expect("subgraph error log window")
                .dropped,
        );
        log_dropped(dropped);
    }
}

fn log_dropped(dropped: u64) {
    if dropped > 0 {
        tracing::warn!(target: LOG_TARGET, dropped, "downstream GraphQL error logs rate limited");
    }
}

fn prefix(text: &str, max_chars: usize) -> &str {
    text.char_indices()
        .nth(max_chars)
        .map_or(text, |(end, _)| &text[..end])
}
