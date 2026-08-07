use std::{
    collections::BTreeMap,
    fmt,
    sync::Arc,
    time::{Duration, SystemTime},
};

use arc_swap::ArcSwap;
use futures::{StreamExt, lock::Mutex};
use graphql_orm_router_protocol::{RootOperationType, SubgraphDescriptor};
use reqwest::{
    Client,
    header::{ETAG, IF_NONE_MATCH},
};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    NetworkPolicy, RouterError, RouterErrorKind, StaticSubgraph,
    auth::AuthorizationCatalog,
    federation::{ActiveGraph, CandidateSubgraph, FederationError, build_active_graph},
    metrics::{RouterMetrics, RouterMetricsSnapshot},
    server::ActiveGraphIdentity,
};

const MAX_PROTOCOL_BYTES: usize = 2 * 1024 * 1024;

/// Durable source category for one process-local subgraph record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SubgraphSourceKind {
    /// Rebuilt from `RouterConfig` on every process start.
    Static,
    /// Process-local registration that must re-register after restart.
    Dynamic,
}

/// Current admission state of one known subgraph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SubgraphRuntimeState {
    /// Known configuration has not yet entered a complete candidate.
    Registered,
    /// A changed input is being evaluated.
    Candidate,
    /// The last-known-good input participates in the active graph.
    Active,
    /// A refresh failed; the last-known-good input remains active.
    Unhealthy,
    /// A changed candidate was rejected; the last-known-good input remains active.
    Rejected,
    /// An explicit removal succeeded and polling is disabled for this process.
    Disabled,
}

/// Safe process-local status for one configured subgraph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubgraphStatus {
    name: String,
    id: Option<String>,
    source_kind: SubgraphSourceKind,
    state: SubgraphRuntimeState,
    active: bool,
    active_fingerprint: Option<String>,
    observed_fingerprint: Option<String>,
    last_error: Option<String>,
    last_successful_refresh: Option<SystemTime>,
}

impl SubgraphStatus {
    /// Configured stable subgraph name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Stable protocol identity when the source publishes a descriptor.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Whether configuration or a process-local registration owns the source.
    pub fn source_kind(&self) -> SubgraphSourceKind {
        self.source_kind
    }

    /// Most recent admission or health state.
    pub fn state(&self) -> SubgraphRuntimeState {
        self.state
    }

    /// Whether the last-known-good input participates in the active graph.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Canonical router-relevant fingerprint of the active input.
    pub fn active_fingerprint(&self) -> Option<&str> {
        self.active_fingerprint.as_deref()
    }

    /// Canonical fingerprint most recently observed from a rejected candidate.
    pub fn observed_fingerprint(&self) -> Option<&str> {
        self.observed_fingerprint.as_deref()
    }

    /// Redacted latest fetch or admission diagnostic.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Last time this source completed an unchanged or activated refresh.
    pub fn last_successful_refresh(&self) -> Option<SystemTime> {
        self.last_successful_refresh
    }
}

/// Safe process-local graph and subgraph lifecycle status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouterStatus {
    active_graph: ActiveGraphIdentity,
    subgraphs: Vec<SubgraphStatus>,
    last_successful_composition: SystemTime,
    last_composition_error: Option<String>,
}

impl RouterStatus {
    /// Identity of the complete executable graph currently selected by new work.
    pub fn active_graph(&self) -> &ActiveGraphIdentity {
        &self.active_graph
    }

    /// Deterministically ordered known subgraph states.
    pub fn subgraphs(&self) -> &[SubgraphStatus] {
        &self.subgraphs
    }

    /// Last time a complete executable graph was successfully constructed.
    pub fn last_successful_composition(&self) -> SystemTime {
        self.last_successful_composition
    }

    /// Safe latest complete-candidate rejection diagnostic, if any.
    pub fn last_composition_error(&self) -> Option<&str> {
        self.last_composition_error.as_deref()
    }
}

/// Result of one serialized conditional refresh round.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SchemaRefreshOutcome {
    /// No router-relevant input changed; fetch health may still have recovered.
    Unchanged,
    /// A complete changed candidate was admitted atomically.
    Activated,
    /// Changed input was observed but the complete candidate was rejected.
    Rejected,
}

/// Process-local control handle for refresh, removal, and safe status.
#[derive(Clone)]
pub struct RouterHandle {
    lifecycle: Arc<GraphLifecycle>,
}

impl fmt::Debug for RouterHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RouterHandle(..)")
    }
}

impl RouterHandle {
    pub(crate) fn new(lifecycle: Arc<GraphLifecycle>) -> Self {
        Self { lifecycle }
    }

    /// Forces one serialized conditional refresh of every enabled subgraph.
    pub async fn refresh(&self) -> Result<SchemaRefreshOutcome, RouterError> {
        self.lifecycle.refresh().await
    }

    /// Explicitly removes one subgraph through complete candidate admission.
    ///
    /// Failure preserves the exact active graph. Successful removal is
    /// process-local; static configuration restores the subgraph on restart.
    pub async fn remove_subgraph(&self, name: &str) -> Result<ActiveGraphIdentity, RouterError> {
        self.lifecycle.remove_subgraph(name).await
    }

    /// Returns safe lifecycle state without credentials, SDL, or variables.
    pub async fn status(&self) -> RouterStatus {
        self.lifecycle.status().await
    }

    /// Returns a point-in-time process-local metrics snapshot.
    pub fn metrics(&self) -> RouterMetricsSnapshot {
        self.lifecycle.metrics().snapshot()
    }

    /// Returns whether this process currently owns a complete active graph.
    pub fn is_ready(&self) -> bool {
        true
    }
}

pub(crate) struct ActiveRouterGraph {
    pub(crate) graph: Arc<ActiveGraph>,
    pub(crate) authorization: Option<Arc<AuthorizationCatalog>>,
}

#[derive(Clone)]
pub(crate) struct FetchedSubgraph {
    pub(crate) candidate: CandidateSubgraph,
    pub(crate) descriptor: Option<SubgraphDescriptor>,
    sdl_etag: Option<String>,
    descriptor_etag: Option<String>,
    fingerprint: String,
}

impl FetchedSubgraph {
    pub(crate) fn from_dynamic_parts(
        config: &StaticSubgraph,
        sdl: String,
        descriptor: SubgraphDescriptor,
        sdl_etag: Option<String>,
        descriptor_etag: Option<String>,
    ) -> Result<Self, RouterError> {
        if sdl.trim().is_empty() {
            return Err(RouterError::new(
                RouterErrorKind::SchemaFetch,
                format!("SDL for subgraph `{}` is empty", config.name),
            ));
        }
        let fingerprint = canonical_input_fingerprint(&sdl, Some(&descriptor))?;
        Ok(Self {
            candidate: CandidateSubgraph::new(&config.name, &config.graphql_url, &sdl),
            descriptor: Some(descriptor),
            sdl_etag,
            descriptor_etag,
            fingerprint,
        })
    }
}

#[derive(Clone)]
struct SourceRecord {
    config: StaticSubgraph,
    fetched: FetchedSubgraph,
    observed: Option<FetchedSubgraph>,
    client: Option<Client>,
    source_kind: SubgraphSourceKind,
    execution_policy: Option<NetworkPolicy>,
    state: SubgraphRuntimeState,
    active: bool,
    observed_fingerprint: Option<String>,
    last_error: Option<String>,
    last_successful_refresh: Option<SystemTime>,
}

#[derive(Clone)]
struct AdmissionAttempt {
    id: String,
    state: SubgraphRuntimeState,
    last_error: Option<String>,
}

struct LifecycleState {
    sources: BTreeMap<String, SourceRecord>,
    admission_attempts: BTreeMap<String, AdmissionAttempt>,
    last_successful_composition: SystemTime,
    last_composition_error: Option<String>,
}

pub(crate) struct GraphLifecycle {
    active: ArcSwap<ActiveRouterGraph>,
    state: Mutex<LifecycleState>,
    client: Client,
    max_sdl_bytes: usize,
    refresh_attempts: usize,
    retry_delay: Duration,
    poll_interval: Duration,
    authorization_required: bool,
    subscriptions_enabled: bool,
    metrics: Arc<RouterMetrics>,
}

impl GraphLifecycle {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        subgraphs: &[StaticSubgraph],
        fetched: Vec<FetchedSubgraph>,
        client: Client,
        max_sdl_bytes: usize,
        refresh_attempts: usize,
        retry_delay: Duration,
        poll_interval: Duration,
        authorization_required: bool,
        subscriptions_enabled: bool,
    ) -> Result<Arc<Self>, RouterError> {
        if subgraphs.len() != fetched.len()
            || subgraphs
                .iter()
                .zip(&fetched)
                .any(|(config, input)| config.name != input.candidate.name)
        {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "fetched subgraph inputs do not match configured sources",
            ));
        }
        let graph = build_snapshot(
            fetched.iter().map(|input| &input.candidate),
            fetched.iter().filter_map(|input| input.descriptor.as_ref()),
            1,
            authorization_required,
            subscriptions_enabled,
        )?;
        let now = SystemTime::now();
        let metrics = Arc::new(RouterMetrics::default());
        metrics.graph_activated(graph.graph.version);
        metrics.composition_success();
        let sources = subgraphs
            .iter()
            .cloned()
            .zip(fetched)
            .map(|(config, fetched)| {
                (
                    config.name.clone(),
                    SourceRecord {
                        config,
                        fetched,
                        observed: None,
                        client: None,
                        source_kind: SubgraphSourceKind::Static,
                        execution_policy: None,
                        state: SubgraphRuntimeState::Active,
                        active: true,
                        observed_fingerprint: None,
                        last_error: None,
                        last_successful_refresh: Some(now),
                    },
                )
            })
            .collect();
        Ok(Arc::new(Self {
            active: ArcSwap::from(graph),
            state: Mutex::new(LifecycleState {
                sources,
                admission_attempts: BTreeMap::new(),
                last_successful_composition: now,
                last_composition_error: None,
            }),
            client,
            max_sdl_bytes,
            refresh_attempts,
            retry_delay,
            poll_interval,
            authorization_required,
            subscriptions_enabled,
            metrics,
        }))
    }

    pub(crate) fn load(&self) -> Arc<ActiveRouterGraph> {
        self.active.load_full()
    }

    pub(crate) fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    pub(crate) fn metrics(&self) -> Arc<RouterMetrics> {
        self.metrics.clone()
    }

    pub(crate) async fn refresh(&self) -> Result<SchemaRefreshOutcome, RouterError> {
        self.metrics.schema_refresh();
        let mut state = self.state.lock().await;
        let enabled = state
            .sources
            .iter()
            .filter(|(_, source)| source.active)
            .map(|(name, source)| {
                (
                    name.clone(),
                    source.config.clone(),
                    source.observed.as_ref().unwrap_or(&source.fetched).clone(),
                    source.client.clone(),
                )
            })
            .collect::<Vec<_>>();
        let mut successful = BTreeMap::new();
        let mut failures = BTreeMap::new();
        for (name, config, previous, client) in enabled {
            match self
                .fetch_with_retries(client.as_ref().unwrap_or(&self.client), &config, &previous)
                .await
            {
                Ok(fetched) => {
                    successful.insert(name, fetched);
                }
                Err(error) => {
                    failures.insert(name, error.to_string());
                }
            }
        }

        let now = SystemTime::now();
        for (name, error) in failures {
            if let Some(source) = state.sources.get_mut(&name) {
                source.state = SubgraphRuntimeState::Unhealthy;
                source.last_error = Some(error);
            }
        }

        let mut changed = BTreeMap::new();
        for (name, fetched) in successful {
            let Some(source) = state.sources.get_mut(&name) else {
                continue;
            };
            if fetched.fingerprint == source.fetched.fingerprint {
                source.fetched.sdl_etag = fetched.sdl_etag;
                source.fetched.descriptor_etag = fetched.descriptor_etag;
                source.observed = None;
                source.state = SubgraphRuntimeState::Active;
                source.observed_fingerprint = None;
                source.last_error = None;
                source.last_successful_refresh = Some(now);
            } else if source
                .observed
                .as_ref()
                .is_some_and(|observed| observed.fingerprint == fetched.fingerprint)
            {
                source.observed = Some(fetched);
                source.state = SubgraphRuntimeState::Rejected;
                source.last_successful_refresh = Some(now);
            } else {
                source.state = SubgraphRuntimeState::Candidate;
                changed.insert(name, fetched);
            }
        }
        if changed.is_empty() {
            return Ok(SchemaRefreshOutcome::Unchanged);
        }

        let next_version = self.active.load().graph.version.saturating_add(1);
        let candidate_inputs = state
            .sources
            .iter()
            .filter(|(_, source)| source.active)
            .map(|(name, source)| changed.get(name).unwrap_or(&source.fetched))
            .cloned()
            .collect::<Vec<_>>();
        let candidate = build_snapshot(
            candidate_inputs.iter().map(|input| &input.candidate),
            candidate_inputs
                .iter()
                .filter_map(|input| input.descriptor.as_ref()),
            next_version,
            self.authorization_required,
            self.subscriptions_enabled,
        );
        let candidate = match candidate {
            Ok(candidate) => candidate,
            Err(error) => {
                let details = error.to_string();
                let rejected_count = changed.len();
                for (name, fetched) in changed {
                    if let Some(source) = state.sources.get_mut(&name) {
                        source.state = SubgraphRuntimeState::Rejected;
                        source.observed_fingerprint = Some(fetched.fingerprint.clone());
                        source.observed = Some(fetched);
                        source.last_error = Some(details.clone());
                    }
                }
                state.last_composition_error = Some(details);
                self.metrics.composition_failure();
                self.metrics.subgraphs_rejected(rejected_count);
                return Ok(SchemaRefreshOutcome::Rejected);
            }
        };

        for (name, fetched) in changed {
            if let Some(source) = state.sources.get_mut(&name) {
                source.fetched = fetched;
                source.observed = None;
                source.state = SubgraphRuntimeState::Active;
                source.observed_fingerprint = None;
                source.last_error = None;
                source.last_successful_refresh = Some(now);
            }
        }
        self.metrics.graph_activated(candidate.graph.version);
        self.active.store(candidate);
        self.metrics.composition_success();
        state.last_successful_composition = now;
        state.last_composition_error = None;
        Ok(SchemaRefreshOutcome::Activated)
    }

    pub(crate) async fn remove_subgraph(
        &self,
        name: &str,
    ) -> Result<ActiveGraphIdentity, RouterError> {
        let mut state = self.state.lock().await;
        let Some(source) = state.sources.get(name) else {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                format!("unknown subgraph `{name}`"),
            ));
        };
        if !source.active {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                format!("subgraph `{name}` is already disabled"),
            ));
        }
        let inputs = state
            .sources
            .iter()
            .filter(|(candidate, source)| source.active && candidate.as_str() != name)
            .map(|(_, source)| source.fetched.clone())
            .collect::<Vec<_>>();
        let next_version = self.active.load().graph.version.saturating_add(1);
        let candidate = build_snapshot(
            inputs.iter().map(|input| &input.candidate),
            inputs.iter().filter_map(|input| input.descriptor.as_ref()),
            next_version,
            self.authorization_required,
            self.subscriptions_enabled,
        );
        let candidate = match candidate {
            Ok(candidate) => candidate,
            Err(error) => {
                let details = error.to_string();
                if let Some(source) = state.sources.get_mut(name) {
                    source.state = SubgraphRuntimeState::Rejected;
                    source.last_error = Some(details.clone());
                }
                state.last_composition_error = Some(details);
                self.metrics.composition_failure();
                self.metrics.subgraphs_rejected(1);
                return Err(error);
            }
        };
        if let Some(source) = state.sources.get_mut(name) {
            source.active = false;
            source.observed = None;
            source.state = SubgraphRuntimeState::Disabled;
            source.observed_fingerprint = None;
            source.last_error = None;
        }
        let identity = ActiveGraphIdentity::from(candidate.graph.as_ref());
        self.metrics.graph_activated(candidate.graph.version);
        self.active.store(candidate);
        self.metrics.composition_success();
        state.last_successful_composition = SystemTime::now();
        state.last_composition_error = None;
        Ok(identity)
    }

    pub(crate) async fn register_dynamic(
        &self,
        config: StaticSubgraph,
        fetched: FetchedSubgraph,
        client: Client,
        execution_policy: NetworkPolicy,
    ) -> Result<ActiveGraphIdentity, RouterError> {
        let mut state = self.state.lock().await;
        if state
            .sources
            .get(&config.name)
            .is_some_and(|source| source.active)
        {
            return Err(RouterError::new(
                RouterErrorKind::Registration,
                format!("subgraph `{}` is already active", config.name),
            ));
        }
        if fetched.candidate.name != config.name {
            return Err(RouterError::new(
                RouterErrorKind::Registration,
                "dynamic candidate identity does not match its registration",
            ));
        }
        state.admission_attempts.remove(&config.name);
        let now = SystemTime::now();
        let next_version = self.active.load().graph.version.saturating_add(1);
        let mut inputs = state
            .sources
            .values()
            .filter(|source| source.active)
            .map(|source| source.fetched.clone())
            .collect::<Vec<_>>();
        inputs.push(fetched.clone());
        let candidate = build_snapshot(
            inputs.iter().map(|input| &input.candidate),
            inputs.iter().filter_map(|input| input.descriptor.as_ref()),
            next_version,
            self.authorization_required,
            self.subscriptions_enabled,
        );
        let name = fetched.candidate.name.clone();
        let fingerprint = fetched.fingerprint.clone();
        let record = |state, active, observed_fingerprint, last_error| SourceRecord {
            config,
            fetched,
            observed: None,
            client: Some(client),
            source_kind: SubgraphSourceKind::Dynamic,
            execution_policy: Some(execution_policy),
            state,
            active,
            observed_fingerprint,
            last_error,
            last_successful_refresh: active.then_some(now),
        };
        let candidate = match candidate {
            Ok(candidate) => candidate,
            Err(error) => {
                let details = error.to_string();
                state.sources.insert(
                    name,
                    record(
                        SubgraphRuntimeState::Rejected,
                        false,
                        Some(fingerprint),
                        Some(details.clone()),
                    ),
                );
                state.last_composition_error = Some(details);
                self.metrics.composition_failure();
                return Err(error);
            }
        };
        state
            .sources
            .insert(name, record(SubgraphRuntimeState::Active, true, None, None));
        let identity = ActiveGraphIdentity::from(candidate.graph.as_ref());
        self.metrics.graph_activated(candidate.graph.version);
        self.active.store(candidate);
        self.metrics.composition_success();
        state.last_successful_composition = now;
        state.last_composition_error = None;
        Ok(identity)
    }

    pub(crate) async fn record_registration_candidate(&self, name: &str, id: &str) {
        self.record_registration_state(name, id, SubgraphRuntimeState::Candidate)
            .await;
    }

    pub(crate) async fn record_registration(&self, name: &str, id: &str) {
        self.record_registration_state(name, id, SubgraphRuntimeState::Registered)
            .await;
    }

    async fn record_registration_state(
        &self,
        name: &str,
        id: &str,
        registration_state: SubgraphRuntimeState,
    ) {
        let mut state = self.state.lock().await;
        if let Some(source) = state.sources.get_mut(name) {
            if !source.active {
                source.state = registration_state;
                source.observed_fingerprint = None;
                source.last_error = None;
            }
            state.admission_attempts.remove(name);
            return;
        }
        state.admission_attempts.insert(
            name.to_owned(),
            AdmissionAttempt {
                id: id.to_owned(),
                state: registration_state,
                last_error: None,
            },
        );
    }

    pub(crate) async fn record_registration_rejected(
        &self,
        name: &str,
        id: &str,
        error: &RouterError,
    ) {
        self.metrics.subgraphs_rejected(1);
        let mut state = self.state.lock().await;
        if let Some(source) = state.sources.get_mut(name) {
            if !source.active {
                source.state = SubgraphRuntimeState::Rejected;
                source.last_error = Some(error.to_string());
            }
            state.admission_attempts.remove(name);
            return;
        }
        state.admission_attempts.insert(
            name.to_owned(),
            AdmissionAttempt {
                id: id.to_owned(),
                state: SubgraphRuntimeState::Rejected,
                last_error: Some(error.to_string()),
            },
        );
    }

    pub(crate) async fn validate_execution_target(
        &self,
        name: &str,
        endpoint: &str,
    ) -> Result<(), RouterError> {
        let policy = {
            let state = self.state.lock().await;
            let Some(source) = state.sources.get(name) else {
                return Err(RouterError::new(
                    RouterErrorKind::NetworkPolicy,
                    "subgraph execution target is not registered",
                ));
            };
            let Some(policy) = &source.execution_policy else {
                return Ok(());
            };
            if source.config.graphql_url != endpoint {
                return Err(RouterError::new(
                    RouterErrorKind::NetworkPolicy,
                    "dynamic GraphQL endpoint changed after admission",
                ));
            }
            policy.clone()
        };
        policy
            .resolve_url(endpoint, "dynamic GraphQL endpoint")
            .await
            .map(|_| ())
    }

    pub(crate) async fn status(&self) -> RouterStatus {
        let state = self.state.lock().await;
        let active = self.active.load_full();
        let mut subgraphs = state
            .sources
            .iter()
            .map(|(name, source)| SubgraphStatus {
                name: name.clone(),
                id: source
                    .fetched
                    .descriptor
                    .as_ref()
                    .map(|descriptor| descriptor.subgraph.id.as_str().to_owned()),
                source_kind: source.source_kind,
                state: source.state,
                active: source.active,
                active_fingerprint: source.active.then(|| source.fetched.fingerprint.clone()),
                observed_fingerprint: source.observed_fingerprint.clone(),
                last_error: source.last_error.clone(),
                last_successful_refresh: source.last_successful_refresh,
            })
            .collect::<Vec<_>>();
        subgraphs.extend(
            state
                .admission_attempts
                .iter()
                .filter(|(name, _)| !state.sources.contains_key(*name))
                .map(|(name, attempt)| SubgraphStatus {
                    name: name.clone(),
                    id: Some(attempt.id.clone()),
                    source_kind: SubgraphSourceKind::Dynamic,
                    state: attempt.state,
                    active: false,
                    active_fingerprint: None,
                    observed_fingerprint: None,
                    last_error: attempt.last_error.clone(),
                    last_successful_refresh: None,
                }),
        );
        subgraphs.sort_by(|left, right| left.name.cmp(&right.name));
        RouterStatus {
            active_graph: ActiveGraphIdentity::from(active.graph.as_ref()),
            subgraphs,
            last_successful_composition: state.last_successful_composition,
            last_composition_error: state.last_composition_error.clone(),
        }
    }

    async fn fetch_with_retries(
        &self,
        client: &Client,
        config: &StaticSubgraph,
        previous: &FetchedSubgraph,
    ) -> Result<FetchedSubgraph, RouterError> {
        let mut last_error = None;
        for attempt in 0..self.refresh_attempts {
            match fetch_refresh(client, config, previous, self.max_sdl_bytes).await {
                Ok(fetched) => return Ok(fetched),
                Err(error) => last_error = Some(error),
            }
            if attempt + 1 < self.refresh_attempts && !self.retry_delay.is_zero() {
                hive_router::tokio::time::sleep(self.retry_delay).await;
            }
        }
        Err(last_error.expect("at least one refresh attempt is configured"))
    }
}

pub(crate) async fn fetch_initial(
    client: &Client,
    subgraph: &StaticSubgraph,
    max_sdl_bytes: usize,
) -> Result<FetchedSubgraph, RouterError> {
    fetch_pair(client, subgraph, None, max_sdl_bytes).await
}

async fn fetch_refresh(
    client: &Client,
    subgraph: &StaticSubgraph,
    previous: &FetchedSubgraph,
    max_sdl_bytes: usize,
) -> Result<FetchedSubgraph, RouterError> {
    fetch_pair(client, subgraph, Some(previous), max_sdl_bytes).await
}

async fn fetch_pair(
    client: &Client,
    subgraph: &StaticSubgraph,
    previous: Option<&FetchedSubgraph>,
    max_sdl_bytes: usize,
) -> Result<FetchedSubgraph, RouterError> {
    let sdl_response = fetch_bounded(
        client,
        &subgraph.sdl_url,
        &subgraph.schema_headers,
        "application/graphql, text/plain;q=0.9",
        previous.and_then(|input| input.sdl_etag.as_deref()),
        max_sdl_bytes,
        &subgraph.name,
        "SDL",
        RouterErrorKind::SchemaFetch,
    )
    .await?;
    let (sdl, sdl_etag) = match sdl_response {
        ConditionalBody::NotModified => {
            let previous = previous.ok_or_else(|| {
                RouterError::new(
                    RouterErrorKind::SchemaFetch,
                    format!(
                        "SDL endpoint for subgraph `{}` returned 304 without prior state",
                        subgraph.name
                    ),
                )
            })?;
            (
                previous.candidate.sdl.to_string(),
                previous.sdl_etag.clone(),
            )
        }
        ConditionalBody::Modified { bytes, etag } => {
            let sdl = String::from_utf8(bytes).map_err(|_| {
                RouterError::new(
                    RouterErrorKind::SchemaFetch,
                    format!("SDL for subgraph `{}` is not UTF-8", subgraph.name),
                )
            })?;
            if sdl.trim().is_empty() {
                return Err(RouterError::new(
                    RouterErrorKind::SchemaFetch,
                    format!("SDL for subgraph `{}` is empty", subgraph.name),
                ));
            }
            (sdl, etag)
        }
    };

    let (descriptor, descriptor_etag) = match &subgraph.protocol_url {
        Some(url) => {
            let response = fetch_bounded(
                client,
                url,
                &subgraph.schema_headers,
                "application/json",
                previous.and_then(|input| input.descriptor_etag.as_deref()),
                MAX_PROTOCOL_BYTES,
                &subgraph.name,
                "router descriptor",
                RouterErrorKind::AuthorizationMetadata,
            )
            .await?;
            match response {
                ConditionalBody::NotModified => {
                    let previous = previous.ok_or_else(|| {
                        RouterError::new(
                            RouterErrorKind::AuthorizationMetadata,
                            format!(
                                "router descriptor endpoint for subgraph `{}` returned 304 without prior state",
                                subgraph.name
                            ),
                        )
                    })?;
                    (
                        previous.descriptor.clone(),
                        previous.descriptor_etag.clone(),
                    )
                }
                ConditionalBody::Modified { bytes, etag } => {
                    let json = std::str::from_utf8(&bytes).map_err(|_| {
                        RouterError::new(
                            RouterErrorKind::AuthorizationMetadata,
                            format!(
                                "router descriptor for subgraph `{}` is not UTF-8",
                                subgraph.name
                            ),
                        )
                    })?;
                    let descriptor =
                        SubgraphDescriptor::from_json_compatible(json).map_err(|error| {
                            RouterError::new(
                                RouterErrorKind::AuthorizationMetadata,
                                format!(
                                    "router descriptor for subgraph `{}` is invalid: {error}",
                                    subgraph.name
                                ),
                            )
                        })?;
                    if descriptor.subgraph.name.as_str() != subgraph.name {
                        return Err(RouterError::new(
                            RouterErrorKind::AuthorizationMetadata,
                            format!(
                                "router descriptor identity does not match configured subgraph `{}`",
                                subgraph.name
                            ),
                        ));
                    }
                    (Some(descriptor), etag)
                }
            }
        }
        None => (None, None),
    };

    let mut candidate = CandidateSubgraph::new(&subgraph.name, &subgraph.graphql_url, &sdl);
    candidate.revision = sdl_etag.clone();
    let fingerprint = canonical_input_fingerprint(&sdl, descriptor.as_ref())?;
    Ok(FetchedSubgraph {
        candidate,
        descriptor,
        sdl_etag,
        descriptor_etag,
        fingerprint,
    })
}

enum ConditionalBody {
    NotModified,
    Modified {
        bytes: Vec<u8>,
        etag: Option<String>,
    },
}

#[allow(clippy::too_many_arguments)]
async fn fetch_bounded(
    client: &Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    accept: &str,
    etag: Option<&str>,
    max_bytes: usize,
    subgraph: &str,
    resource: &str,
    kind: RouterErrorKind,
) -> Result<ConditionalBody, RouterError> {
    let mut request = client
        .get(url)
        .header("accept", accept)
        .header("user-agent", "graphql-orm-router/0.1");
    for (name, value) in headers {
        request = request.header(name, value);
    }
    if let Some(etag) = etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    let response = request.send().await.map_err(|_| {
        RouterError::new(
            kind,
            format!("failed to fetch {resource} for subgraph `{subgraph}`"),
        )
    })?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(ConditionalBody::NotModified);
    }
    if !response.status().is_success() {
        return Err(RouterError::new(
            kind,
            format!(
                "{resource} endpoint for subgraph `{subgraph}` returned HTTP {}",
                response.status()
            ),
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(RouterError::new(
            kind,
            format!("{resource} for subgraph `{subgraph}` exceeds its configured size limit"),
        ));
    }
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            RouterError::new(
                kind,
                format!("failed while reading {resource} for subgraph `{subgraph}`"),
            )
        })?;
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(RouterError::new(
                kind,
                format!("{resource} for subgraph `{subgraph}` exceeds its configured size limit"),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(ConditionalBody::Modified { bytes, etag })
}

fn canonical_input_fingerprint(
    sdl: &str,
    descriptor: Option<&SubgraphDescriptor>,
) -> Result<String, RouterError> {
    let document = cynic_parser::parse_type_system_document(sdl).map_err(|error| {
        RouterError::new(
            RouterErrorKind::SchemaFetch,
            format!("fetched SDL is invalid: {error}"),
        )
    })?;
    let normalized_sdl = document.pretty_printer().sorted().to_string();
    let mut hasher = Sha256::new();
    write_fingerprint_part(&mut hasher, b"graphql-orm-router-input-v1");
    write_fingerprint_part(&mut hasher, normalized_sdl.as_bytes());
    if let Some(descriptor) = descriptor {
        let mut canonical = descriptor.clone();
        canonical.canonicalize();
        let metadata = serde_json::to_vec(&json!({
            "protocol_version": canonical.protocol_version,
            "subgraph": canonical.subgraph,
            "capabilities": canonical.capabilities,
            "required_semantics": canonical.required_semantics,
            "operations": canonical.operations,
        }))
        .expect("canonical protocol metadata always serializes");
        write_fingerprint_part(&mut hasher, &metadata);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn write_fingerprint_part(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn build_snapshot<'a>(
    inputs: impl IntoIterator<Item = &'a CandidateSubgraph>,
    descriptors: impl IntoIterator<Item = &'a SubgraphDescriptor>,
    version: u64,
    authorization_required: bool,
    subscriptions_enabled: bool,
) -> Result<Arc<ActiveRouterGraph>, RouterError> {
    let inputs = inputs.into_iter().cloned().collect::<Vec<_>>();
    let descriptors = descriptors.into_iter().cloned().collect::<Vec<_>>();
    let graph = build_active_graph(&inputs, version).map_err(map_federation_error)?;
    if authorization_required && descriptors.len() != inputs.len() {
        return Err(RouterError::new(
            RouterErrorKind::AuthorizationMetadata,
            "authenticated graph requires every subgraph descriptor",
        ));
    }
    if subscriptions_enabled {
        validate_subscription_metadata(&graph, &descriptors)?;
    }
    let authorization = authorization_required
        .then(|| AuthorizationCatalog::build(&graph, &descriptors))
        .transpose()?
        .map(Arc::new);
    Ok(Arc::new(ActiveRouterGraph {
        graph,
        authorization,
    }))
}

fn validate_subscription_metadata(
    graph: &ActiveGraph,
    descriptors: &[SubgraphDescriptor],
) -> Result<(), RouterError> {
    if graph
        .hive
        .snapshot()
        .planner
        .supergraph
        .subscription_type
        .is_none()
    {
        return Err(RouterError::new(
            RouterErrorKind::InvalidConfiguration,
            "subscriptions are enabled but the composed graph has no subscription root",
        ));
    }
    let mut declared = false;
    for descriptor in descriptors {
        let owns_subscription = descriptor
            .operations
            .iter()
            .any(|operation| operation.root_type == RootOperationType::Subscription);
        if owns_subscription && !descriptor.capabilities.subscriptions {
            return Err(RouterError::new(
                RouterErrorKind::AuthorizationMetadata,
                format!(
                    "subgraph `{}` owns subscription fields without advertising subscription capability",
                    descriptor.subgraph.name.as_str()
                ),
            ));
        }
        declared |= owns_subscription;
    }
    if !declared {
        return Err(RouterError::new(
            RouterErrorKind::AuthorizationMetadata,
            "the composed subscription root has no protocol ownership declaration",
        ));
    }
    Ok(())
}

fn map_federation_error(error: FederationError) -> RouterError {
    let kind = match error {
        FederationError::Runtime { .. } => RouterErrorKind::Runtime,
        _ => RouterErrorKind::Composition,
    };
    RouterError::new(kind, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphql_orm_router_protocol::{
        AdvertisedEndpoint, AuthorizationRequirement, CapabilitySet, DescriptorFingerprints,
        Fingerprint, GraphqlEndpoints, OperationDescriptor, ProtocolVersion, SchemaAdvertisement,
        SubgraphId, SubgraphIdentity, SubgraphName,
    };
    use reqwest::redirect::Policy;
    use std::{
        io::{Read, Write},
        net::{SocketAddr, TcpListener},
        sync::{
            Mutex as StdMutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        thread::{self, JoinHandle},
    };

    const PRODUCT: &str = r#"
        extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key"])
        type Query { product(id: ID!): Product }
        type Product @key(fields: "id") { id: ID!, name: String! }
    "#;
    const REVIEWS: &str = r#"
        extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key", "@external"])
        type Product @key(fields: "id") { id: ID! @external, reviews: [Review!]! }
        type Review { body: String! }
    "#;
    const CONFLICTING_REVIEWS: &str = r#"
        extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key"])
        type Query { review: String }
        type Product @key(fields: "id") { id: Int! }
    "#;

    #[test]
    fn canonical_input_ignores_formatting_and_advertised_endpoint_noise() {
        let first = "type Product { name: String!, id: ID! } type Query { product: Product, value: String! }";
        let second = "\n type Query { value: String! product: Product }\n type Product { id: ID! name: String! }\n";
        let first_descriptor =
            endpoint_noise_descriptor("http://old.internal/graphql", "http://old.internal/sdl");
        let second_descriptor =
            endpoint_noise_descriptor("https://new.example/graphql", "https://new.example/sdl");
        assert_eq!(
            canonical_input_fingerprint(first, Some(&first_descriptor)).unwrap(),
            canonical_input_fingerprint(second, Some(&second_descriptor)).unwrap()
        );
    }

    fn endpoint_noise_descriptor(graphql: &str, schema: &str) -> SubgraphDescriptor {
        let endpoint = |value: &str| AdvertisedEndpoint::try_from(value.to_owned()).unwrap();
        let mut descriptor = SubgraphDescriptor {
            protocol_version: ProtocolVersion { major: 1, minor: 0 },
            subgraph: SubgraphIdentity {
                id: SubgraphId::try_from("products-service".to_owned()).unwrap(),
                name: SubgraphName::try_from("products".to_owned()).unwrap(),
            },
            graphql: GraphqlEndpoints {
                http: endpoint(graphql),
                websocket: None,
            },
            schema: SchemaAdvertisement {
                url: endpoint(schema),
            },
            capabilities: CapabilitySet {
                subscriptions: false,
                authorization_metadata: true,
                schema_fingerprints: true,
            },
            required_semantics: vec!["authorizationMetadata".to_owned()],
            operations: vec![OperationDescriptor {
                root_type: RootOperationType::Query,
                field_name: "value".to_owned(),
                arguments: Vec::new(),
                authorization: AuthorizationRequirement::Public,
            }],
            fingerprints: DescriptorFingerprints {
                schema: Fingerprint::sha256("schema"),
                authorization: Fingerprint::sha256("authorization"),
                combined: Fingerprint::sha256("combined"),
            },
        };
        descriptor.fingerprints.authorization = descriptor.authorization_fingerprint();
        descriptor.fingerprints.combined = descriptor.combined_fingerprint();
        descriptor
    }

    #[ntex::test]
    async fn prefetch_registration_states_are_safe_and_process_local() {
        let endpoint = MutableEndpoint::new("type Query { value: String! }", "v1");
        let config = StaticSubgraph::new("values", "http://values.test/graphql", endpoint.url());
        let lifecycle = lifecycle(&[config], 1).await;

        lifecycle
            .record_registration("reviews", "reviews-service")
            .await;
        let registered = lifecycle.status().await;
        let registration = registered
            .subgraphs()
            .iter()
            .find(|status| status.name() == "reviews")
            .unwrap();
        assert_eq!(registration.id(), Some("reviews-service"));
        assert_eq!(registration.source_kind(), SubgraphSourceKind::Dynamic);
        assert_eq!(registration.state(), SubgraphRuntimeState::Registered);
        assert!(!registration.is_active());

        lifecycle
            .record_registration_candidate("reviews", "reviews-service")
            .await;
        assert_eq!(
            lifecycle
                .status()
                .await
                .subgraphs()
                .iter()
                .find(|status| status.name() == "reviews")
                .unwrap()
                .state(),
            SubgraphRuntimeState::Candidate
        );

        lifecycle
            .record_registration_rejected(
                "reviews",
                "reviews-service",
                &RouterError::new(RouterErrorKind::Registration, "safe admission failure"),
            )
            .await;
        let rejected = lifecycle.status().await;
        let registration = rejected
            .subgraphs()
            .iter()
            .find(|status| status.name() == "reviews")
            .unwrap();
        assert_eq!(registration.state(), SubgraphRuntimeState::Rejected);
        assert_eq!(registration.last_error(), Some("safe admission failure"));
        assert!(registration.active_fingerprint().is_none());
    }

    #[ntex::test]
    async fn conditional_refresh_activation_health_and_failed_removal_preserve_lkg() {
        let endpoint = MutableEndpoint::new("type Query { value: String! }", "v1");
        let config = StaticSubgraph::new("values", "http://values.test/graphql", endpoint.url());
        let lifecycle = lifecycle(&[config], 1).await;
        let handle = RouterHandle::new(lifecycle.clone());
        let initial = handle.status().await.active_graph().clone();

        assert_eq!(
            handle.refresh().await.unwrap(),
            SchemaRefreshOutcome::Unchanged
        );
        assert_eq!(handle.status().await.active_graph(), &initial);
        assert!(endpoint.conditional_requests() >= 1);

        endpoint.set("type Query { value: String!, version: Int! }", "v2", 200);
        assert_eq!(
            handle.refresh().await.unwrap(),
            SchemaRefreshOutcome::Activated
        );
        let activated = handle.status().await;
        assert_eq!(activated.active_graph().version(), 2);
        assert_ne!(
            activated.active_graph().fingerprint(),
            initial.fingerprint()
        );

        endpoint.set("type Query { broken: }", "v3", 200);
        assert_eq!(
            handle.refresh().await.unwrap(),
            SchemaRefreshOutcome::Unchanged
        );
        let unhealthy = handle.status().await;
        assert_eq!(unhealthy.active_graph(), activated.active_graph());
        assert_eq!(
            unhealthy.subgraphs()[0].state(),
            SubgraphRuntimeState::Unhealthy
        );
        assert!(unhealthy.subgraphs()[0].last_error().is_some());

        endpoint.set(
            "\n type Query { value: String! version: Int! }\n",
            "v4",
            200,
        );
        assert_eq!(
            handle.refresh().await.unwrap(),
            SchemaRefreshOutcome::Unchanged
        );
        assert_eq!(
            handle.status().await.subgraphs()[0].state(),
            SubgraphRuntimeState::Active
        );

        assert!(handle.remove_subgraph("values").await.is_err());
        assert_eq!(
            handle.status().await.active_graph(),
            activated.active_graph()
        );
    }

    #[ntex::test]
    async fn refresh_retries_are_bounded_and_preserve_the_executable_graph() {
        let endpoint = MutableEndpoint::new("type Query { value: String! }", "v1");
        let config = StaticSubgraph::new("values", "http://values.test/graphql", endpoint.url());
        let lifecycle = lifecycle(&[config], 3).await;
        let handle = RouterHandle::new(lifecycle);
        let initial = handle.status().await.active_graph().clone();
        let before = endpoint.requests();

        endpoint.set("", "unavailable", 503);
        assert_eq!(
            handle.refresh().await.unwrap(),
            SchemaRefreshOutcome::Unchanged
        );
        assert_eq!(endpoint.requests() - before, 3);
        let status = handle.status().await;
        assert_eq!(status.active_graph(), &initial);
        assert_eq!(
            status.subgraphs()[0].state(),
            SubgraphRuntimeState::Unhealthy
        );
    }

    #[ntex::test]
    async fn incompatible_candidate_and_explicit_removal_are_atomic() {
        let products = MutableEndpoint::new(PRODUCT, "products-v1");
        let reviews = MutableEndpoint::new(REVIEWS, "reviews-v1");
        let configs = [
            StaticSubgraph::new("products", "http://products.test/graphql", products.url()),
            StaticSubgraph::new("reviews", "http://reviews.test/graphql", reviews.url()),
        ];
        let lifecycle = lifecycle(&configs, 1).await;
        let handle = RouterHandle::new(lifecycle);
        let initial = handle.status().await.active_graph().clone();

        reviews.set(CONFLICTING_REVIEWS, "reviews-v2", 200);
        assert_eq!(
            handle.refresh().await.unwrap(),
            SchemaRefreshOutcome::Rejected
        );
        let rejected = handle.status().await;
        assert_eq!(rejected.active_graph(), &initial);
        let review_status = rejected
            .subgraphs()
            .iter()
            .find(|status| status.name() == "reviews")
            .unwrap();
        assert_eq!(review_status.state(), SubgraphRuntimeState::Rejected);
        assert!(review_status.is_active());
        assert!(review_status.observed_fingerprint().is_some());

        let rejected_identity = rejected.active_graph().clone();
        assert_eq!(
            handle.refresh().await.unwrap(),
            SchemaRefreshOutcome::Unchanged,
            "an unchanged rejected ETag must not be recomposed"
        );
        let still_rejected = handle.status().await;
        assert_eq!(still_rejected.active_graph(), &rejected_identity);
        assert_eq!(
            still_rejected
                .subgraphs()
                .iter()
                .find(|status| status.name() == "reviews")
                .unwrap()
                .state(),
            SubgraphRuntimeState::Rejected
        );

        reviews.set(REVIEWS, "reviews-v3", 200);
        assert_eq!(
            handle.refresh().await.unwrap(),
            SchemaRefreshOutcome::Unchanged
        );
        let removed = handle.remove_subgraph("reviews").await.unwrap();
        assert_eq!(removed.version(), 2);
        let status = handle.status().await;
        let review_status = status
            .subgraphs()
            .iter()
            .find(|status| status.name() == "reviews")
            .unwrap();
        assert_eq!(review_status.state(), SubgraphRuntimeState::Disabled);
        assert!(!review_status.is_active());

        assert!(handle.remove_subgraph("products").await.is_err());
        assert_eq!(handle.status().await.active_graph(), &removed);
    }

    #[ntex::test]
    async fn bounded_repeated_reload_campaign_retains_health_and_monotonic_identity() {
        const RELOADS: u64 = 24;
        let endpoint = MutableEndpoint::new("type Query { value: String! }", "generation-1");
        let config = StaticSubgraph::new("values", "http://values.test/graphql", endpoint.url());
        let lifecycle = lifecycle(&[config], 2).await;
        let handle = RouterHandle::new(lifecycle);

        for generation in 2..=RELOADS + 1 {
            endpoint.set(
                &format!("type Query {{ value: String!, generation{generation}: String }}"),
                &format!("generation-{generation}"),
                200,
            );
            assert_eq!(
                handle.refresh().await.unwrap(),
                SchemaRefreshOutcome::Activated
            );
            assert_eq!(handle.status().await.active_graph().version(), generation);
        }

        let active = handle.status().await.active_graph().clone();
        endpoint.set("", "bounded-outage", 503);
        assert_eq!(
            handle.refresh().await.unwrap(),
            SchemaRefreshOutcome::Unchanged
        );
        assert_eq!(handle.status().await.active_graph(), &active);
        assert_eq!(
            handle.status().await.subgraphs()[0].state(),
            SubgraphRuntimeState::Unhealthy
        );

        endpoint.set(
            "type Query { value: String!, recovered: String }",
            "recovered",
            200,
        );
        assert_eq!(
            handle.refresh().await.unwrap(),
            SchemaRefreshOutcome::Activated
        );
        let recovered = handle.status().await;
        assert_eq!(recovered.active_graph().version(), RELOADS + 2);
        assert_eq!(
            recovered.subgraphs()[0].state(),
            SubgraphRuntimeState::Active
        );
        let metrics = handle.metrics();
        assert_eq!(metrics.active_graph_version(), RELOADS + 2);
        assert_eq!(metrics.schema_refresh_total(), RELOADS + 2);
        assert_eq!(metrics.composition_failure_total(), 0);
    }

    #[ntex::test]
    async fn cancelled_and_serialized_refreshes_cannot_publish_an_older_candidate() {
        let endpoint = MutableEndpoint::new("type Query { one: String }", "v1");
        let config = StaticSubgraph::new("values", "http://values.test/graphql", endpoint.url());
        let lifecycle = lifecycle(&[config], 1).await;
        let handle = RouterHandle::new(lifecycle.clone());

        endpoint.set_delay(Duration::from_millis(200));
        endpoint.set("type Query { one: String, cancelled: String }", "v2", 200);
        let cancelled =
            hive_router::tokio::time::timeout(Duration::from_millis(30), handle.refresh()).await;
        assert!(cancelled.is_err());
        assert_eq!(handle.status().await.active_graph().version(), 1);
        hive_router::tokio::time::sleep(Duration::from_millis(220)).await;

        endpoint.set("type Query { one: String, two: String }", "v3", 200);
        endpoint.set_delay(Duration::from_millis(150));
        let request_target = endpoint.requests() + 1;
        let (tx, rx) = futures::channel::oneshot::channel();
        let first = handle.clone();
        hive_router::ntex::rt::spawn(async move {
            let _ = tx.send(first.refresh().await);
        });
        endpoint.wait_for_requests(request_target).await;
        endpoint.set("type Query { one: String, three: String }", "v4", 200);
        endpoint.set_delay(Duration::ZERO);
        let second = handle.refresh().await.unwrap();
        assert_eq!(rx.await.unwrap().unwrap(), SchemaRefreshOutcome::Activated);
        assert_eq!(second, SchemaRefreshOutcome::Activated);
        let active = lifecycle.load();
        assert_eq!(active.graph.version, 3);
        assert!(active.graph.supergraph_sdl.contains("three"));
        assert!(!active.graph.supergraph_sdl.contains("two"));
    }

    async fn lifecycle(configs: &[StaticSubgraph], attempts: usize) -> Arc<GraphLifecycle> {
        let client = Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let fetched = futures::future::try_join_all(
            configs
                .iter()
                .map(|config| fetch_initial(&client, config, 1024 * 1024)),
        )
        .await
        .unwrap();
        GraphLifecycle::new(
            configs,
            fetched,
            client,
            1024 * 1024,
            attempts,
            Duration::ZERO,
            Duration::from_secs(60),
            false,
            false,
        )
        .unwrap()
    }

    struct MutableEndpoint {
        address: SocketAddr,
        state: Arc<StdMutex<EndpointState>>,
        requests: Arc<AtomicUsize>,
        conditional_requests: Arc<AtomicUsize>,
        stopping: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    #[derive(Clone)]
    struct EndpointState {
        body: String,
        etag: String,
        status: u16,
        delay: Duration,
    }

    impl MutableEndpoint {
        fn new(body: &str, etag: &str) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let state = Arc::new(StdMutex::new(EndpointState {
                body: body.to_owned(),
                etag: etag.to_owned(),
                status: 200,
                delay: Duration::ZERO,
            }));
            let requests = Arc::new(AtomicUsize::new(0));
            let conditional_requests = Arc::new(AtomicUsize::new(0));
            let stopping = Arc::new(AtomicBool::new(false));
            let thread_state = state.clone();
            let thread_requests = requests.clone();
            let thread_conditional = conditional_requests.clone();
            let thread_stopping = stopping.clone();
            let thread = thread::spawn(move || {
                while !thread_stopping.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let state = thread_state.clone();
                            let requests = thread_requests.clone();
                            let conditional = thread_conditional.clone();
                            thread::spawn(move || {
                                let _ = serve_endpoint(stream, state, requests, conditional);
                            });
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => return,
                    }
                }
            });
            Self {
                address,
                state,
                requests,
                conditional_requests,
                stopping,
                thread: Some(thread),
            }
        }

        fn url(&self) -> String {
            format!("http://{}/sdl", self.address)
        }

        fn set(&self, body: &str, etag: &str, status: u16) {
            let mut state = self.state.lock().unwrap();
            state.body = body.to_owned();
            state.etag = etag.to_owned();
            state.status = status;
        }

        fn set_delay(&self, delay: Duration) {
            self.state.lock().unwrap().delay = delay;
        }

        fn requests(&self) -> usize {
            self.requests.load(Ordering::Acquire)
        }

        fn conditional_requests(&self) -> usize {
            self.conditional_requests.load(Ordering::Acquire)
        }

        async fn wait_for_requests(&self, target: usize) {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while self.requests() < target {
                assert!(std::time::Instant::now() < deadline);
                hive_router::tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
    }

    impl Drop for MutableEndpoint {
        fn drop(&mut self) {
            self.stopping.store(true, Ordering::Release);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn serve_endpoint(
        mut stream: std::net::TcpStream,
        state: Arc<StdMutex<EndpointState>>,
        requests: Arc<AtomicUsize>,
        conditional_requests: Arc<AtomicUsize>,
    ) -> std::io::Result<()> {
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut request = Vec::new();
        let mut buffer = [0_u8; 512];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer)?;
            if count == 0 {
                return Ok(());
            }
            request.extend_from_slice(&buffer[..count]);
        }
        let request = String::from_utf8_lossy(&request);
        let snapshot = state.lock().unwrap().clone();
        requests.fetch_add(1, Ordering::AcqRel);
        if !snapshot.delay.is_zero() {
            thread::sleep(snapshot.delay);
        }
        let conditional = request.lines().any(|line| {
            line.split_once(':').is_some_and(|(name, value)| {
                name.eq_ignore_ascii_case("if-none-match")
                    && value.trim() == format!("\"{}\"", snapshot.etag)
            })
        });
        if conditional {
            conditional_requests.fetch_add(1, Ordering::AcqRel);
            stream.write_all(b"HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n")?;
        } else if snapshot.status == 200 {
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/graphql\r\nETag: \"{}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                snapshot.etag,
                snapshot.body.len(),
                snapshot.body
            )?;
        } else {
            write!(
                stream,
                "HTTP/1.1 {} Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                snapshot.status
            )?;
        }
        stream.flush()
    }
}
