use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode},
    middleware::Next,
    Extension,
};
use serde::Serialize;
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use uuid::Uuid;

use crate::config::{
    AdminCredentialSourceView, Config, DataAuthConfig as StaticDataAuthConfig, DataAuthMode,
    SecretSourceRef, UpstreamConfig, UpstreamProviderKeySourceRef,
};
use crate::formats::UpstreamFormat;

use super::errors::error_response;
use super::state::{AppState, RuntimeState};

pub(super) const AUTH_MODE_ENV: &str = "LLM_UNIVERSAL_PROXY_AUTH_MODE";
pub(super) const PROXY_KEY_ENV: &str = "LLM_UNIVERSAL_PROXY_KEY";
pub(super) const LEGACY_DATA_TOKEN_HEADER: &str = "x-llmup-data-token";
pub(super) const CORS_ALLOWED_ORIGINS_ENV: &str = "LLM_UNIVERSAL_PROXY_CORS_ALLOWED_ORIGINS";

#[derive(Clone)]
pub(super) enum DataAccess {
    Unconfigured,
    ClientProviderKey,
    ProxyKey { key: String },
    Misconfigured(String),
}

impl std::fmt::Debug for DataAccess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unconfigured => formatter.write_str("Unconfigured"),
            Self::ClientProviderKey => formatter.write_str("ClientProviderKey"),
            Self::ProxyKey { .. } => formatter
                .debug_struct("ProxyKey")
                .field("key", &"<redacted>")
                .finish(),
            Self::Misconfigured(message) => formatter
                .debug_tuple("Misconfigured")
                .field(message)
                .finish(),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeConfigValidationPolicy {
    manager: DataAuthManager,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct AdminDataAuthConfigView {
    pub(super) configured: bool,
    pub(super) mode: DataAuthMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) proxy_key: Option<AdminCredentialSourceView>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct AdminDataAuthSnapshot {
    pub(super) revision: String,
    pub(super) config: AdminDataAuthConfigView,
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeDataAuthState {
    revision: String,
    access: DataAccess,
    config: AdminDataAuthConfigView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RequestAuthorization {
    ClientProviderKey { provider_key: String },
    ProxyKey,
}

#[derive(Clone)]
pub(super) struct RequestAuthContext {
    generation: String,
    mode: DataAuthMode,
    access: DataAccess,
    authorization: RequestAuthorization,
    runtime: Arc<RuntimeState>,
}

#[derive(Debug, Clone)]
pub(super) struct DataAuthManager {
    inner: Arc<RwLock<RuntimeDataAuthState>>,
}

impl std::fmt::Debug for RequestAuthContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RequestAuthContext")
            .field("generation", &self.generation)
            .field("mode", &self.mode)
            .field("access", &self.access)
            .field("authorization", &self.authorization.redacted_debug())
            .field("namespace_count", &self.runtime.namespaces.len())
            .finish()
    }
}

impl RequestAuthorization {
    fn redacted_debug(&self) -> &'static str {
        match self {
            Self::ClientProviderKey { .. } => "ClientProviderKey(<redacted>)",
            Self::ProxyKey => "ProxyKey",
        }
    }
}

impl RequestAuthContext {
    fn new(
        generation: String,
        mode: DataAuthMode,
        access: DataAccess,
        authorization: RequestAuthorization,
        runtime: Arc<RuntimeState>,
    ) -> Self {
        Self {
            generation,
            mode,
            access,
            authorization,
            runtime,
        }
    }

    #[cfg(test)]
    pub(super) fn for_test(
        generation: impl Into<String>,
        mode: DataAuthMode,
        access: DataAccess,
        authorization: RequestAuthorization,
        runtime: RuntimeState,
    ) -> Self {
        Self::new(
            generation.into(),
            mode,
            access,
            authorization,
            Arc::new(runtime),
        )
    }

    pub(super) fn generation(&self) -> &str {
        &self.generation
    }

    pub(super) fn mode(&self) -> DataAuthMode {
        self.mode
    }

    pub(super) fn access(&self) -> &DataAccess {
        &self.access
    }

    pub(super) fn authorization(&self) -> &RequestAuthorization {
        &self.authorization
    }

    pub(super) fn runtime(&self) -> &RuntimeState {
        &self.runtime
    }

    pub(super) fn client_provider_key(&self) -> Option<&str> {
        match &self.authorization {
            RequestAuthorization::ClientProviderKey { provider_key } => Some(provider_key),
            RequestAuthorization::ProxyKey => None,
        }
    }
}

impl RuntimeConfigValidationPolicy {
    #[cfg(test)]
    pub(super) fn new(_listener_addr: SocketAddr, access: DataAccess) -> Self {
        Self {
            manager: DataAuthManager::new(RuntimeDataAuthState::from_access(access)),
        }
    }

    pub(super) fn from_manager(_listener_addr: SocketAddr, manager: DataAuthManager) -> Self {
        Self { manager }
    }

    pub(super) fn manager(&self) -> DataAuthManager {
        self.manager.clone()
    }

    pub(super) async fn current_access(&self) -> DataAccess {
        self.manager.current_access().await
    }
}

impl DataAuthManager {
    pub(super) fn new(state: RuntimeDataAuthState) -> Self {
        Self {
            inner: Arc::new(RwLock::new(state)),
        }
    }

    pub(super) async fn current_access(&self) -> DataAccess {
        self.inner.read().await.access.clone()
    }

    pub(super) async fn snapshot(&self) -> AdminDataAuthSnapshot {
        self.inner.read().await.admin_snapshot()
    }

    pub(super) async fn read(&self) -> RwLockReadGuard<'_, RuntimeDataAuthState> {
        self.inner.read().await
    }

    pub(super) async fn write(&self) -> RwLockWriteGuard<'_, RuntimeDataAuthState> {
        self.inner.write().await
    }
}

impl RuntimeDataAuthState {
    pub(super) fn from_static_config(config: Option<&StaticDataAuthConfig>) -> Self {
        match config {
            Some(config) => Self::from_config(config, "data_auth"),
            None => Self::from_env(),
        }
    }

    pub(super) fn from_admin_config(config: StaticDataAuthConfig) -> Result<Self, String> {
        config.validate_admin_payload()?;
        let (access, view) = data_access_from_config(&config, "data_auth config")?;
        Ok(Self {
            revision: generate_data_auth_revision(),
            access,
            config: view,
        })
    }

    fn from_config(config: &StaticDataAuthConfig, owner: &str) -> Self {
        match data_access_from_config(config, owner) {
            Ok((access, view)) => Self {
                revision: generate_data_auth_revision(),
                access,
                config: view,
            },
            Err(message) => Self::misconfigured(message),
        }
    }

    fn from_env() -> Self {
        match data_access_from_env_results(
            std::env::var(AUTH_MODE_ENV),
            std::env::var(PROXY_KEY_ENV),
        ) {
            Ok((access, view)) => Self {
                revision: generate_data_auth_revision(),
                access,
                config: view,
            },
            Err(message) => Self::misconfigured(message),
        }
    }

    pub(super) fn from_access(access: DataAccess) -> Self {
        let config = admin_view_for_access(&access);
        Self {
            revision: generate_data_auth_revision(),
            access,
            config,
        }
    }

    fn misconfigured(message: String) -> Self {
        Self {
            revision: generate_data_auth_revision(),
            access: DataAccess::Misconfigured(message),
            config: AdminDataAuthConfigView {
                configured: false,
                mode: DataAuthMode::ClientProviderKey,
                proxy_key: None,
            },
        }
    }

    pub(super) fn access(&self) -> &DataAccess {
        &self.access
    }

    pub(super) fn revision(&self) -> &str {
        &self.revision
    }

    pub(super) fn into_admin_response(self) -> AdminDataAuthSnapshot {
        self.admin_snapshot()
    }

    fn admin_snapshot(&self) -> AdminDataAuthSnapshot {
        AdminDataAuthSnapshot {
            revision: self.revision.clone(),
            config: self.config.clone(),
        }
    }
}

impl DataAccess {
    pub(super) fn provider_key_for_upstream(
        &self,
        upstream: &UpstreamConfig,
    ) -> Result<Option<String>, String> {
        match self {
            Self::Unconfigured => Err("data auth is not configured".to_string()),
            Self::ClientProviderKey => Ok(None),
            Self::ProxyKey { .. } => resolve_provider_key(upstream).map(Some),
            Self::Misconfigured(message) => Err(format!("data auth misconfigured: {message}")),
        }
    }
}

fn generate_data_auth_revision() -> String {
    Uuid::new_v4().to_string()
}

fn admin_view_for_access(access: &DataAccess) -> AdminDataAuthConfigView {
    match access {
        DataAccess::Unconfigured => AdminDataAuthConfigView {
            configured: false,
            mode: DataAuthMode::ClientProviderKey,
            proxy_key: None,
        },
        DataAccess::ClientProviderKey => AdminDataAuthConfigView {
            configured: true,
            mode: DataAuthMode::ClientProviderKey,
            proxy_key: None,
        },
        DataAccess::ProxyKey { .. } => AdminDataAuthConfigView {
            configured: true,
            mode: DataAuthMode::ProxyKey,
            proxy_key: Some(AdminCredentialSourceView {
                source: "inline",
                configured: true,
                redacted: true,
                env_name: None,
            }),
        },
        DataAccess::Misconfigured(_) => AdminDataAuthConfigView {
            configured: false,
            mode: DataAuthMode::ClientProviderKey,
            proxy_key: None,
        },
    }
}

fn data_access_from_config(
    config: &StaticDataAuthConfig,
    owner: &str,
) -> Result<(DataAccess, AdminDataAuthConfigView), String> {
    config.validate_admin_payload()?;
    match config.mode {
        DataAuthMode::ClientProviderKey => Ok((
            DataAccess::ClientProviderKey,
            AdminDataAuthConfigView {
                configured: true,
                mode: DataAuthMode::ClientProviderKey,
                proxy_key: None,
            },
        )),
        DataAuthMode::ProxyKey => {
            let proxy_key = config
                .proxy_key
                .as_ref()
                .ok_or_else(|| format!("{owner}.proxy_key is required when mode=proxy_key"))?;
            let (key, view) = resolve_proxy_key_source(proxy_key, &format!("{owner}.proxy_key"))?;
            Ok((
                DataAccess::ProxyKey { key },
                AdminDataAuthConfigView {
                    configured: true,
                    mode: DataAuthMode::ProxyKey,
                    proxy_key: Some(view),
                },
            ))
        }
    }
}

fn resolve_proxy_key_source(
    source: &crate::config::SecretSourceConfig,
    owner: &str,
) -> Result<(String, AdminCredentialSourceView), String> {
    match source.source(owner)? {
        SecretSourceRef::Inline(value) => Ok((
            value.to_string(),
            AdminCredentialSourceView {
                source: "inline",
                configured: true,
                redacted: true,
                env_name: None,
            },
        )),
        SecretSourceRef::Env(env_name) => {
            let key = resolve_secret_env(owner, env_name)?;
            Ok((
                key,
                AdminCredentialSourceView {
                    source: "env",
                    configured: true,
                    redacted: true,
                    env_name: Some(env_name.to_string()),
                },
            ))
        }
    }
}

fn resolve_secret_env(owner: &str, env_name: &str) -> Result<String, String> {
    match std::env::var(env_name) {
        Ok(value) if value.trim().is_empty() => {
            Err(format!("{owner}.env `{env_name}` must not be empty"))
        }
        Ok(value) => Ok(value),
        Err(std::env::VarError::NotPresent) => Err(format!("{owner}.env `{env_name}` is not set")),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(format!("{owner}.env `{env_name}` must be valid UTF-8"))
        }
    }
}

fn data_access_from_env_results(
    mode: Result<String, std::env::VarError>,
    proxy_key: Result<String, std::env::VarError>,
) -> Result<(DataAccess, AdminDataAuthConfigView), String> {
    match mode {
        Ok(mode) => match mode.trim().to_ascii_lowercase().as_str() {
            "client_provider_key" => Ok((
                DataAccess::ClientProviderKey,
                AdminDataAuthConfigView {
                    configured: true,
                    mode: DataAuthMode::ClientProviderKey,
                    proxy_key: None,
                },
            )),
            "proxy_key" => {
                let key = proxy_key_from_env_result(proxy_key)?;
                Ok((
                    DataAccess::ProxyKey { key },
                    AdminDataAuthConfigView {
                        configured: true,
                        mode: DataAuthMode::ProxyKey,
                        proxy_key: Some(AdminCredentialSourceView {
                            source: "env",
                            configured: true,
                            redacted: true,
                            env_name: Some(PROXY_KEY_ENV.to_string()),
                        }),
                    },
                ))
            }
            "" => Err(format!("{AUTH_MODE_ENV} must not be empty")),
            value => Err(format!("{AUTH_MODE_ENV} has unsupported value `{value}`")),
        },
        Err(std::env::VarError::NotPresent) => Ok((
            DataAccess::Unconfigured,
            AdminDataAuthConfigView {
                configured: false,
                mode: DataAuthMode::ClientProviderKey,
                proxy_key: None,
            },
        )),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(format!("{AUTH_MODE_ENV} must be valid UTF-8"))
        }
    }
}

fn proxy_key_from_env_result(
    proxy_key: Result<String, std::env::VarError>,
) -> Result<String, String> {
    match proxy_key {
        Ok(key) if key.trim().is_empty() => Err(format!("{PROXY_KEY_ENV} must not be empty")),
        Ok(key) => Ok(key),
        Err(std::env::VarError::NotPresent) => Err(format!(
            "{PROXY_KEY_ENV} is required when {AUTH_MODE_ENV}=proxy_key"
        )),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(format!("{PROXY_KEY_ENV} must be valid UTF-8"))
        }
    }
}

pub(super) fn validate_startup(
    config: &Config,
    _listener_addr: SocketAddr,
    access: &DataAccess,
) -> Result<(), String> {
    validate_runtime_config(config, access)
}

pub(super) fn validate_runtime_config(config: &Config, access: &DataAccess) -> Result<(), String> {
    if matches!(access, DataAccess::Unconfigured) {
        if config.upstreams.is_empty() {
            return Ok(());
        }
        return Err("data auth is not configured".to_string());
    }

    if let DataAccess::Misconfigured(message) = access {
        return Err(format!("data auth misconfigured: {message}"));
    }

    match access {
        DataAccess::Unconfigured => unreachable!("unconfigured access returned above"),
        DataAccess::ClientProviderKey => {
            for upstream in &config.upstreams {
                if matches!(
                    upstream.provider_key_source()?,
                    Some(UpstreamProviderKeySourceRef::Inline(_))
                ) {
                    return Err(format!(
                        "upstream `{}` provider_key.inline is not allowed when {AUTH_MODE_ENV}=client_provider_key",
                        upstream.name
                    ));
                }
            }
        }
        DataAccess::ProxyKey { .. } => {
            for upstream in &config.upstreams {
                resolve_provider_key(upstream)?;
            }
        }
        DataAccess::Misconfigured(_) => unreachable!("misconfigured access returned above"),
    }

    Ok(())
}

fn resolve_provider_key(upstream: &UpstreamConfig) -> Result<String, String> {
    match upstream.provider_key_source()? {
        Some(UpstreamProviderKeySourceRef::Inline(value)) => Ok(value.to_string()),
        Some(UpstreamProviderKeySourceRef::Env { name, legacy }) => match std::env::var(name) {
            Ok(value) if value.trim().is_empty() => Err(format!(
                "upstream `{}` {} `{name}` must not be empty",
                upstream.name,
                provider_env_label(legacy)
            )),
            Ok(value) => Ok(value),
            Err(std::env::VarError::NotPresent) => Err(format!(
                "upstream `{}` {} `{name}` is not set",
                upstream.name,
                provider_env_label(legacy)
            )),
            Err(std::env::VarError::NotUnicode(_)) => Err(format!(
                "upstream `{}` {} `{name}` must be valid UTF-8",
                upstream.name,
                provider_env_label(legacy)
            )),
        },
        None => Err(format!(
            "upstream `{}` provider_key or provider_key_env is required when {AUTH_MODE_ENV}=proxy_key",
            upstream.name
        )),
    }
}

fn provider_env_label(legacy: bool) -> &'static str {
    if legacy {
        "provider_key_env"
    } else {
        "provider_key.env"
    }
}

pub(super) async fn request_auth_context_for_headers(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<RequestAuthContext, (StatusCode, &'static str)> {
    let runtime = state.runtime.read().await;
    let manager = state.data_auth_policy.manager();
    let data_auth = manager.read().await;
    let access = data_auth.access.clone();
    let authorization = authorize_data_request(&access, headers)?.authorization;
    Ok(RequestAuthContext::new(
        data_auth.revision.clone(),
        data_auth.config.mode,
        access,
        authorization,
        Arc::new(runtime.clone()),
    ))
}

pub(super) fn request_auth_context_from_request<B>(
    request: &Request<B>,
) -> Option<RequestAuthContext> {
    request.extensions().get::<RequestAuthContext>().cloned()
}

pub(super) fn request_auth_context_from_extension(
    auth_context: Option<Extension<RequestAuthContext>>,
) -> Option<RequestAuthContext> {
    auth_context.map(|Extension(context)| context)
}

pub(super) fn missing_request_auth_context_response(
    client_format: UpstreamFormat,
) -> Response<Body> {
    error_response(
        client_format,
        StatusCode::UNAUTHORIZED,
        "request authentication context required",
    )
}

pub(super) async fn require_data_access(
    State(state): State<Arc<AppState>>,
    mut request: Request<Body>,
    next: Next,
) -> Response<Body> {
    match request_auth_context_for_headers(&state, request.headers()).await {
        Ok(auth_context) => {
            strip_client_credential_headers(request.headers_mut());
            request.extensions_mut().insert(auth_context);
            next.run(request).await
        }
        Err((status, message)) => error_response(UpstreamFormat::OpenAiCompletion, status, message),
    }
}

struct DataAuthorization {
    authorization: RequestAuthorization,
}

fn authorize_data_request(
    access: &DataAccess,
    headers: &HeaderMap,
) -> Result<DataAuthorization, (StatusCode, &'static str)> {
    if headers.contains_key(LEGACY_DATA_TOKEN_HEADER) {
        return Err((
            StatusCode::BAD_REQUEST,
            "x-llmup-data-token is no longer supported",
        ));
    }

    let credential = match extract_standard_credential(headers) {
        Ok(value) => value,
        Err(error) => return Err((StatusCode::BAD_REQUEST, error.message())),
    };

    match access {
        DataAccess::Unconfigured => {
            Err((StatusCode::SERVICE_UNAVAILABLE, "data auth unconfigured"))
        }
        DataAccess::ClientProviderKey => {
            let Some(provider_key) = credential else {
                return Err((StatusCode::UNAUTHORIZED, "provider credential required"));
            };
            Ok(DataAuthorization {
                authorization: RequestAuthorization::ClientProviderKey { provider_key },
            })
        }
        DataAccess::ProxyKey { key } => {
            let Some(proxy_key) = credential else {
                return Err((StatusCode::UNAUTHORIZED, "proxy key required"));
            };
            if proxy_key == *key {
                Ok(DataAuthorization {
                    authorization: RequestAuthorization::ProxyKey,
                })
            } else {
                Err((StatusCode::FORBIDDEN, "proxy key invalid"))
            }
        }
        DataAccess::Misconfigured(_) => {
            Err((StatusCode::SERVICE_UNAVAILABLE, "data auth misconfigured"))
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CredentialError {
    Empty,
    Multiple,
    NonBearerAuthorization,
    InvalidHeaderValue,
}

impl CredentialError {
    fn message(self) -> &'static str {
        match self {
            Self::Empty => "credential must not be empty",
            Self::Multiple => "multiple credential headers are not supported",
            Self::NonBearerAuthorization => "authorization credential must use Bearer",
            Self::InvalidHeaderValue => "credential header must be valid UTF-8",
        }
    }
}

fn strip_client_credential_headers(headers: &mut HeaderMap) {
    for name in [
        header::AUTHORIZATION,
        HeaderName::from_static("x-api-key"),
        HeaderName::from_static("api-key"),
        HeaderName::from_static("openai-api-key"),
        HeaderName::from_static("anthropic-api-key"),
        HeaderName::from_static(LEGACY_DATA_TOKEN_HEADER),
    ] {
        headers.remove(name);
    }
}

fn extract_standard_credential(headers: &HeaderMap) -> Result<Option<String>, CredentialError> {
    let mut credentials = Vec::new();
    collect_authorization_credentials(headers, &mut credentials)?;
    for name in [
        "x-api-key",
        "api-key",
        "openai-api-key",
        "anthropic-api-key",
    ] {
        collect_api_key_credentials(headers, name, &mut credentials)?;
    }
    match credentials.len() {
        0 => Ok(None),
        1 => Ok(credentials.pop()),
        _ => Err(CredentialError::Multiple),
    }
}

fn collect_authorization_credentials(
    headers: &HeaderMap,
    credentials: &mut Vec<String>,
) -> Result<(), CredentialError> {
    for value in headers.get_all(header::AUTHORIZATION).iter() {
        let value = value
            .to_str()
            .map_err(|_| CredentialError::InvalidHeaderValue)?;
        let Some(token) = value
            .get(..7)
            .filter(|prefix| prefix.eq_ignore_ascii_case("Bearer "))
            .map(|_| &value[7..])
        else {
            return Err(CredentialError::NonBearerAuthorization);
        };
        if token.trim().is_empty() {
            return Err(CredentialError::Empty);
        }
        credentials.push(token.to_string());
    }
    Ok(())
}

fn collect_api_key_credentials(
    headers: &HeaderMap,
    name: &'static str,
    credentials: &mut Vec<String>,
) -> Result<(), CredentialError> {
    for value in headers.get_all(HeaderName::from_static(name)).iter() {
        let value = value
            .to_str()
            .map_err(|_| CredentialError::InvalidHeaderValue)?;
        if value.trim().is_empty() {
            return Err(CredentialError::Empty);
        }
        credentials.push(value.to_string());
    }
    Ok(())
}

pub(super) fn cors_allowed_origins_from_env() -> Result<Vec<HeaderValue>, String> {
    let raw = match std::env::var(CORS_ALLOWED_ORIGINS_ENV) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(Vec::new()),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!("{CORS_ALLOWED_ORIGINS_ENV} must be valid UTF-8"));
        }
    };

    raw.split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(parse_allowed_origin)
        .collect()
}

fn parse_allowed_origin(origin: &str) -> Result<HeaderValue, String> {
    if origin == "*" {
        return Err(format!("{CORS_ALLOWED_ORIGINS_ENV} must not include `*`"));
    }
    let parsed = url::Url::parse(origin).map_err(|error| {
        format!("{CORS_ALLOWED_ORIGINS_ENV} origin `{origin}` is invalid: {error}")
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "{CORS_ALLOWED_ORIGINS_ENV} origin `{origin}` must use http or https"
        ));
    }
    if parsed.host_str().is_none() {
        return Err(format!(
            "{CORS_ALLOWED_ORIGINS_ENV} origin `{origin}` must include a host"
        ));
    }
    if parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err(format!(
            "{CORS_ALLOWED_ORIGINS_ENV} origin `{origin}` must be an origin, not a URL with path, userinfo, query, or fragment"
        ));
    }
    HeaderValue::from_str(origin).map_err(|error| {
        format!("{CORS_ALLOWED_ORIGINS_ENV} origin `{origin}` is not a valid header value: {error}")
    })
}
