pub(super) use std::collections::BTreeMap;
pub(super) use std::sync::Arc;

pub(super) use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, Response, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
pub(super) use reqwest::Client;
pub(super) use serde_json::Value;
pub(super) use tokio::sync::{Mutex, RwLock};

pub(super) use super::*;

mod admin;
mod errors;
mod headers;
mod proxy;
mod responses_resources;
mod state;

pub(super) struct ScopedEnvVar {
    key: &'static str,
    previous: Option<String>,
}

impl ScopedEnvVar {
    pub(super) fn set(key: &'static str, value: impl AsRef<str>) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value.as_ref());
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        if let Some(value) = &self.previous {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

pub(super) fn test_upstream_config(
    name: &str,
    format: crate::formats::UpstreamFormat,
) -> crate::config::UpstreamConfig {
    test_upstream_config_with_fixed_format(name, Some(format))
}

pub(super) fn test_upstream_config_with_fixed_format(
    name: &str,
    fixed_upstream_format: Option<crate::formats::UpstreamFormat>,
) -> crate::config::UpstreamConfig {
    crate::config::UpstreamConfig {
        name: name.to_string(),
        api_root: format!("https://{name}.example/v1"),
        fixed_upstream_format,
        fallback_credential_env: None,
        fallback_credential_actual: None,
        fallback_api_key: None,
        auth_policy: crate::config::AuthPolicy::ClientOrFallback,
        upstream_headers: Vec::new(),
    }
}

pub(super) fn runtime_namespace_state_for_tests(
    upstreams: &[(&str, crate::formats::UpstreamFormat, bool)],
) -> RuntimeNamespaceState {
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        upstreams: upstreams
            .iter()
            .map(|(name, format, _)| test_upstream_config(name, *format))
            .collect(),
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
    };
    let upstream_states = upstreams
        .iter()
        .map(|(name, format, available)| {
            (
                (*name).to_string(),
                UpstreamState {
                    config: test_upstream_config(name, *format),
                    capability: Some(crate::discovery::UpstreamCapability::fixed(*format)),
                    availability: if *available {
                        crate::discovery::UpstreamAvailability::available()
                    } else {
                        crate::discovery::UpstreamAvailability::unavailable("test outage")
                    },
                },
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    RuntimeNamespaceState {
        revision: "test-revision".to_string(),
        config,
        upstreams: upstream_states,
        client: Client::new(),
        hooks: None,
        debug_trace: None,
    }
}

pub(super) async fn spawn_openai_completion_mock(
    response_body: Value,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Value>>>,
        response_body: Value,
    }

    async fn handle_chat_completions(
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state.requests.lock().await.push(body);
        Json(state.response_body)
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/chat/completions", post(handle_chat_completions))
        .with_state(MockState {
            requests: requests.clone(),
            response_body,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock upstream");
    let addr = listener.local_addr().expect("mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock server");
    });

    (format!("http://{addr}"), requests, server)
}

pub(super) fn app_state_for_single_upstream(
    api_root: String,
    upstream_format: crate::formats::UpstreamFormat,
) -> Arc<AppState> {
    let upstream = crate::config::UpstreamConfig {
        name: "primary".to_string(),
        api_root,
        fixed_upstream_format: Some(upstream_format),
        fallback_credential_env: None,
        fallback_credential_actual: None,
        fallback_api_key: None,
        auth_policy: crate::config::AuthPolicy::ClientOrFallback,
        upstream_headers: Vec::new(),
    };
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        upstreams: vec![upstream.clone()],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
    };
    let runtime = RuntimeState {
        namespaces: BTreeMap::from([(
            DEFAULT_NAMESPACE.to_string(),
            RuntimeNamespaceState {
                revision: "test-revision".to_string(),
                config: config.clone(),
                upstreams: BTreeMap::from([(
                    upstream.name.clone(),
                    UpstreamState {
                        config: upstream,
                        capability: Some(crate::discovery::UpstreamCapability::fixed(
                            upstream_format,
                        )),
                        availability: crate::discovery::UpstreamAvailability::available(),
                    },
                )]),
                client: crate::upstream::build_client(&config),
                hooks: None,
                debug_trace: None,
            },
        )]),
    };

    Arc::new(AppState {
        runtime: Arc::new(RwLock::new(runtime)),
        metrics: crate::telemetry::RuntimeMetrics::new(&crate::config::Config::default()),
        admin_access: AdminAccess::LoopbackOnly,
    })
}
