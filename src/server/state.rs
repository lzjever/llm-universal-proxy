use std::collections::BTreeMap;
use std::sync::Arc;

use reqwest::Client;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::config::{Config, UpstreamConfig};
use crate::debug_trace::DebugTraceRecorder;
use crate::discovery::{DiscoveredUpstream, UpstreamAvailability, UpstreamCapability};
use crate::hooks::{HookDispatcher, HookSnapshot};
use crate::telemetry::RuntimeMetrics;
use crate::upstream;

pub(super) const DEFAULT_NAMESPACE: &str = "default";
pub(super) const TEST_FORCE_UNAVAILABLE_UPSTREAMS_ENV: &str =
    "LLM_UNIVERSAL_PROXY_TEST_FORCE_UNAVAILABLE_UPSTREAMS";

#[derive(Clone)]
pub(super) struct AppState {
    pub(super) runtime: Arc<RwLock<RuntimeState>>,
    pub(super) metrics: Arc<RuntimeMetrics>,
    pub(super) admin_access: AdminAccess,
}

#[derive(Clone)]
pub(super) enum AdminAccess {
    BearerToken(String),
    LoopbackOnly,
    Misconfigured,
}

impl AdminAccess {
    pub(super) fn from_env() -> Self {
        Self::from_env_var_result(std::env::var("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN"))
    }

    pub(super) fn from_env_var_result(var_result: Result<String, std::env::VarError>) -> Self {
        match var_result {
            Ok(token) if token.trim().is_empty() => Self::Misconfigured,
            Ok(token) => Self::BearerToken(token),
            Err(std::env::VarError::NotPresent) => Self::LoopbackOnly,
            Err(std::env::VarError::NotUnicode(_)) => Self::Misconfigured,
        }
    }
}

#[derive(Clone)]
pub(super) struct UpstreamState {
    pub(super) config: UpstreamConfig,
    pub(super) capability: Option<UpstreamCapability>,
    pub(super) availability: UpstreamAvailability,
}

#[derive(Clone)]
pub(super) struct RuntimeNamespaceState {
    pub(super) revision: String,
    pub(super) config: Config,
    pub(super) upstreams: BTreeMap<String, UpstreamState>,
    pub(super) client: Client,
    pub(super) hooks: Option<HookDispatcher>,
    pub(super) debug_trace: Option<DebugTraceRecorder>,
}

#[derive(Default)]
pub(super) struct RuntimeState {
    pub(super) namespaces: BTreeMap<String, RuntimeNamespaceState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DashboardUpstreamStatus {
    pub name: String,
    pub availability_status: String,
    pub availability_reason: Option<String>,
}

#[derive(Clone)]
pub(crate) struct DashboardRuntimeHandle {
    pub(super) runtime: Arc<RwLock<RuntimeState>>,
}

#[derive(Clone)]
pub(crate) struct DashboardNamespaceSnapshot {
    pub config: Config,
    pub upstreams: Vec<DashboardUpstreamStatus>,
    pub hooks: Option<HookSnapshot>,
}

impl DashboardRuntimeHandle {
    pub(super) fn new(runtime: Arc<RwLock<RuntimeState>>) -> Self {
        Self { runtime }
    }

    pub(crate) fn snapshot(&self) -> DashboardNamespaceSnapshot {
        let Ok(runtime) = self.runtime.try_read() else {
            return DashboardNamespaceSnapshot::empty();
        };
        runtime
            .namespaces
            .get(DEFAULT_NAMESPACE)
            .map(|namespace| DashboardNamespaceSnapshot {
                config: namespace.config.clone(),
                upstreams: namespace
                    .upstreams
                    .values()
                    .map(|upstream| DashboardUpstreamStatus {
                        name: upstream.config.name.clone(),
                        availability_status: upstream.availability.status_label().to_string(),
                        availability_reason: upstream
                            .availability
                            .reason()
                            .map(ToString::to_string),
                    })
                    .collect(),
                hooks: namespace
                    .hooks
                    .as_ref()
                    .map(|dispatcher| dispatcher.snapshot()),
            })
            .unwrap_or_else(DashboardNamespaceSnapshot::empty)
    }
}

impl DashboardNamespaceSnapshot {
    fn empty() -> Self {
        Self {
            config: Config::default(),
            upstreams: Vec::new(),
            hooks: None,
        }
    }
}

pub(super) async fn build_runtime_namespace_state(
    revision: String,
    config: Config,
) -> Result<RuntimeNamespaceState, String> {
    if !config.upstreams.is_empty() {
        config.validate()?;
    }
    let upstreams = resolve_upstreams(&config).await;
    let client = upstream::build_client(&config);
    let hooks = HookDispatcher::new(&config.hooks);
    let debug_trace = DebugTraceRecorder::new(&config.debug_trace);
    Ok(RuntimeNamespaceState {
        revision,
        config,
        upstreams,
        client,
        hooks,
        debug_trace,
    })
}

pub(super) async fn build_runtime_state(config: Config) -> Result<RuntimeState, String> {
    let mut state = RuntimeState::default();
    if !config.upstreams.is_empty() {
        state.namespaces.insert(
            DEFAULT_NAMESPACE.to_string(),
            build_runtime_namespace_state(generate_admin_revision(), config).await?,
        );
    }
    Ok(state)
}

pub(super) fn generate_admin_revision() -> String {
    Uuid::new_v4().to_string()
}

pub(super) async fn resolve_upstreams(config: &Config) -> BTreeMap<String, UpstreamState> {
    let mut upstreams = BTreeMap::new();
    for upstream in &config.upstreams {
        let mut discovered = if let Some(f) = upstream.fixed_upstream_format {
            DiscoveredUpstream::fixed(f)
        } else {
            let supported = crate::discovery::discover_supported_formats(
                &upstream.api_root,
                config.upstream_timeout,
                upstream.fallback_api_key.as_deref(),
                &upstream.upstream_headers,
            )
            .await;
            DiscoveredUpstream::from_supported(supported)
        };
        if test_forced_upstream_unavailable(&upstream.name) {
            discovered.availability =
                UpstreamAvailability::unavailable("forced unavailable by test override");
        }
        upstreams.insert(
            upstream.name.clone(),
            UpstreamState {
                config: upstream.clone(),
                capability: discovered.capability,
                availability: discovered.availability,
            },
        );
    }
    upstreams
}

fn test_forced_upstream_unavailable(name: &str) -> bool {
    std::env::var(TEST_FORCE_UNAVAILABLE_UPSTREAMS_ENV)
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .any(|candidate| !candidate.is_empty() && candidate == name)
        })
        .unwrap_or(false)
}
