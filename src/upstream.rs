//! Upstream HTTP client: build request URLs and call upstream resources.

use std::error::Error;
use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::rt::TokioExecutor;
use reqwest::{Client, Proxy};
use serde_json::Value;

use crate::config::{Config, ProxyConfig, UpstreamConfig};
use crate::downstream::DownstreamCancellation;
use crate::formats::UpstreamFormat;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedProxySource {
    Upstream,
    Namespace,
    Environment,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedProxyTarget {
    Inherited,
    Direct,
    Proxy { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedProxyMetadata {
    pub source: ResolvedProxySource,
    pub target: ResolvedProxyTarget,
}

pub(crate) fn resolve_upstream_proxy(
    upstream_proxy: Option<&ProxyConfig>,
    namespace_proxy: Option<&ProxyConfig>,
) -> ResolvedProxyMetadata {
    if let Some(proxy) = upstream_proxy {
        return ResolvedProxyMetadata {
            source: ResolvedProxySource::Upstream,
            target: resolved_proxy_target(proxy),
        };
    }
    if let Some(proxy) = namespace_proxy {
        return ResolvedProxyMetadata {
            source: ResolvedProxySource::Namespace,
            target: resolved_proxy_target(proxy),
        };
    }
    if has_environment_proxy_configuration() {
        return ResolvedProxyMetadata {
            source: ResolvedProxySource::Environment,
            target: ResolvedProxyTarget::Inherited,
        };
    }
    ResolvedProxyMetadata {
        source: ResolvedProxySource::None,
        target: ResolvedProxyTarget::Inherited,
    }
}

fn resolved_proxy_target(proxy: &ProxyConfig) -> ResolvedProxyTarget {
    match proxy {
        ProxyConfig::Direct => ResolvedProxyTarget::Direct,
        ProxyConfig::Proxy { url } => ResolvedProxyTarget::Proxy { url: url.clone() },
    }
}

fn has_environment_proxy_configuration() -> bool {
    const CANDIDATES: [&str; 6] = [
        "ALL_PROXY",
        "all_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ];
    CANDIDATES.into_iter().any(|key| {
        std::env::var(key)
            .ok()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    })
}

fn build_client_with_proxy(
    timeout: Duration,
    resolved_proxy: &ResolvedProxyMetadata,
    streaming: bool,
    auto_decompression: bool,
) -> Result<Client, String> {
    let mut builder = Client::builder();
    builder = if streaming {
        builder.connect_timeout(timeout)
    } else {
        builder.timeout(timeout)
    };
    if !auto_decompression {
        builder = builder.no_gzip().no_brotli().no_zstd().no_deflate();
    }
    match resolved_proxy.target {
        ResolvedProxyTarget::Inherited => {}
        ResolvedProxyTarget::Direct => {
            builder = builder.no_proxy();
        }
        ResolvedProxyTarget::Proxy { ref url } => {
            builder = builder.no_proxy();
            let proxy = Proxy::all(url)
                .map_err(|error| format!("invalid explicit upstream proxy `{url}`: {error}"))?;
            builder = builder.proxy(proxy);
        }
    }
    builder
        .build()
        .map_err(|error| format!("failed to build upstream HTTP client: {error}"))
}

pub(crate) fn build_upstream_clients(
    config: &Config,
    upstream_proxy: Option<&ProxyConfig>,
    namespace_proxy: Option<&ProxyConfig>,
) -> Result<(Client, Client, ResolvedProxyMetadata), String> {
    let resolved_proxy = resolve_upstream_proxy(upstream_proxy, namespace_proxy);
    let client = build_client_with_proxy(config.upstream_timeout, &resolved_proxy, false, true)?;
    let streaming_client =
        build_client_with_proxy(config.upstream_timeout, &resolved_proxy, true, true)?;
    Ok((client, streaming_client, resolved_proxy))
}

pub(crate) fn build_no_auto_decompression_client(
    timeout: Duration,
    resolved_proxy: &ResolvedProxyMetadata,
) -> Result<Client, String> {
    build_client_with_proxy(timeout, resolved_proxy, false, false)
}

/// Build a reqwest client with timeout from config.
pub fn build_client(config: &Config) -> Client {
    build_upstream_clients(config, None, config.proxy.as_ref())
        .map(|(client, _, _)| client)
        .unwrap_or_else(|_| Client::new())
}

/// Build a reqwest client for streaming requests.
///
/// Keep the connect/setup timeout, but avoid a total request timeout so long-lived
/// SSE streams are not cut off mid-body by the unary timeout budget.
pub fn build_streaming_client(config: &Config) -> Client {
    build_upstream_clients(config, None, config.proxy.as_ref())
        .map(|(_, client, _)| client)
        .unwrap_or_else(|_| Client::new())
}

/// Call upstream with JSON body; for non-streaming, read full body and return (status, body bytes).
/// For streaming, returns the response so caller can forward the stream.
pub async fn call_upstream(
    client: &Client,
    url: &str,
    body: &Value,
    stream: bool,
    headers: &[(String, String)],
) -> Result<reqwest::Response, reqwest::Error> {
    let mut req = client.post(url).json(body);
    if stream {
        req = req.header("Accept", "text/event-stream");
    }
    // Forward auth headers
    for (name, value) in headers {
        req = req.header(name, value);
    }
    req.send().await
}

#[derive(Debug)]
pub(crate) enum DownstreamAwareError<E> {
    Inner(E),
    DownstreamCancelled,
}

type BoxError = Box<dyn Error + Send + Sync>;

pub(crate) struct UpstreamResourceTarget {
    reqwest_url: String,
    raw_uri: hyper::Uri,
    requires_raw_path_fidelity: bool,
}

pub(crate) struct UpstreamResourceRequest<'a> {
    pub(crate) method: reqwest::Method,
    pub(crate) target: &'a UpstreamResourceTarget,
    pub(crate) body: Option<&'a Value>,
    pub(crate) headers: &'a [(String, String)],
    pub(crate) accept_event_stream: bool,
    pub(crate) resolved_proxy: &'a ResolvedProxyMetadata,
}

pub(crate) enum UpstreamResourceResponse {
    Reqwest(reqwest::Response),
    Hyper(hyper::Response<hyper::body::Incoming>),
}

impl UpstreamResourceResponse {
    pub(crate) fn status(&self) -> reqwest::StatusCode {
        match self {
            Self::Reqwest(response) => response.status(),
            Self::Hyper(response) => response.status(),
        }
    }

    pub(crate) fn headers(&self) -> &reqwest::header::HeaderMap {
        match self {
            Self::Reqwest(response) => response.headers(),
            Self::Hyper(response) => response.headers(),
        }
    }

    pub(crate) fn into_bytes_stream(
        self,
    ) -> Pin<Box<dyn Stream<Item = Result<Bytes, BoxError>> + Send>> {
        match self {
            Self::Reqwest(response) => Box::pin(
                response
                    .bytes_stream()
                    .map(|result| result.map_err(|error| Box::new(error) as BoxError)),
            ),
            Self::Hyper(response) => Box::pin(
                response
                    .into_body()
                    .into_data_stream()
                    .map(|result| result.map_err(|error| Box::new(error) as BoxError)),
            ),
        }
    }
}

async fn await_with_downstream_cancellation<F, T, E>(
    future: F,
    downstream_cancellation: &DownstreamCancellation,
) -> Result<T, DownstreamAwareError<E>>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    tokio::select! {
        result = future => result.map_err(DownstreamAwareError::Inner),
        _ = downstream_cancellation.cancelled() => Err(DownstreamAwareError::DownstreamCancelled),
    }
}

pub(crate) async fn call_upstream_with_cancellation(
    client: &Client,
    url: &str,
    body: &Value,
    stream: bool,
    headers: &[(String, String)],
    downstream_cancellation: &DownstreamCancellation,
) -> Result<reqwest::Response, DownstreamAwareError<reqwest::Error>> {
    let mut req = client.post(url).json(body);
    if stream {
        req = req.header("Accept", "text/event-stream");
    }
    for (name, value) in headers {
        req = req.header(name, value);
    }
    await_with_downstream_cancellation(req.send(), downstream_cancellation).await
}

pub(crate) fn build_upstream_resource_target(
    api_root: &str,
    resource_path: &str,
    query: Option<&str>,
) -> Result<UpstreamResourceTarget, String> {
    let mut reqwest_url = crate::config::build_upstream_resource_url(api_root, resource_path);
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        reqwest_url.push('?');
        reqwest_url.push_str(query);
    }

    let parsed = url::Url::parse(api_root)
        .map_err(|error| format!("upstream api_root is not a valid URL: {error}"))?;
    let origin = parsed.origin().ascii_serialization();
    let base_path = parsed.path().trim_end_matches('/');
    let resource_path = resource_path.trim_start_matches('/');
    let mut path_and_query = if base_path.is_empty() || base_path == "/" {
        format!("/{resource_path}")
    } else {
        format!("{base_path}/{resource_path}")
    };
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        path_and_query.push('?');
        path_and_query.push_str(query);
    }
    let raw_uri = format!("{origin}{path_and_query}")
        .parse::<hyper::Uri>()
        .map_err(|error| format!("upstream resource URI is invalid: {error}"))?;

    Ok(UpstreamResourceTarget {
        reqwest_url,
        raw_uri,
        requires_raw_path_fidelity: resource_path_requires_raw_path_fidelity(resource_path),
    })
}

fn resource_path_requires_raw_path_fidelity(resource_path: &str) -> bool {
    resource_path
        .trim_matches('/')
        .split('/')
        .any(url_stack_normalizes_dot_segment)
}

fn url_stack_normalizes_dot_segment(segment: &str) -> bool {
    let normalized = segment.to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "." | ".." | "%2e" | "%2e%2e" | "%2e." | ".%2e"
    )
}

/// Call an arbitrary upstream HTTP resource.
pub async fn call_upstream_resource(
    client: &Client,
    method: reqwest::Method,
    url: &str,
    body: Option<&Value>,
    headers: &[(String, String)],
) -> Result<reqwest::Response, reqwest::Error> {
    send_upstream_resource_request(client, method, url, body, headers, false).await
}

async fn send_upstream_resource_request(
    client: &Client,
    method: reqwest::Method,
    url: &str,
    body: Option<&Value>,
    headers: &[(String, String)],
    accept_event_stream: bool,
) -> Result<reqwest::Response, reqwest::Error> {
    let mut req = client.request(method, url);
    if accept_event_stream {
        req = req.header("Accept", "text/event-stream");
    }
    if let Some(body) = body {
        req = req.json(body);
    }
    for (name, value) in headers {
        req = req.header(name, value);
    }
    req.send().await
}

async fn send_upstream_resource_request_preserving_path(
    client: &Client,
    method: reqwest::Method,
    target: &UpstreamResourceTarget,
    body: Option<&Value>,
    headers: &[(String, String)],
    accept_event_stream: bool,
    resolved_proxy: &ResolvedProxyMetadata,
) -> Result<UpstreamResourceResponse, BoxError> {
    if target.requires_raw_path_fidelity {
        if !raw_path_fidelity_sender_can_use_direct_connection(resolved_proxy) {
            return Err("Responses resource path contains a dot segment that requires raw request-target fidelity, but the configured upstream proxy would route this request through the URL-normalizing client".into());
        }
        return send_raw_path_upstream_resource_request(
            method,
            target.raw_uri.clone(),
            body,
            headers,
            accept_event_stream,
        )
        .await;
    }

    send_upstream_resource_request(
        client,
        method,
        &target.reqwest_url,
        body,
        headers,
        accept_event_stream,
    )
    .await
    .map(UpstreamResourceResponse::Reqwest)
    .map_err(|error| Box::new(error) as BoxError)
}

fn raw_path_fidelity_sender_can_use_direct_connection(
    resolved_proxy: &ResolvedProxyMetadata,
) -> bool {
    matches!(
        (&resolved_proxy.source, &resolved_proxy.target),
        (ResolvedProxySource::None, ResolvedProxyTarget::Inherited)
            | (_, ResolvedProxyTarget::Direct)
    )
}

async fn send_raw_path_upstream_resource_request(
    method: reqwest::Method,
    uri: hyper::Uri,
    body: Option<&Value>,
    headers: &[(String, String)],
    accept_event_stream: bool,
) -> Result<UpstreamResourceResponse, BoxError> {
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();
    let client: HyperClient<_, Full<Bytes>> =
        HyperClient::builder(TokioExecutor::new()).build(https);

    let body_bytes = match body {
        Some(body) => {
            Bytes::from(serde_json::to_vec(body).map_err(|error| Box::new(error) as BoxError)?)
        }
        None => Bytes::new(),
    };
    let mut request = hyper::Request::builder().method(method.as_str()).uri(uri);
    if accept_event_stream {
        request = request.header(reqwest::header::ACCEPT, "text/event-stream");
    }
    if body.is_some() {
        request = request
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(
                reqwest::header::CONTENT_LENGTH,
                body_bytes.len().to_string(),
            );
    }
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }

    let request = request
        .body(Full::new(body_bytes))
        .map_err(|error| Box::new(error) as BoxError)?;
    let response = client
        .request(request)
        .await
        .map_err(|error| Box::new(error) as BoxError)?;
    Ok(UpstreamResourceResponse::Hyper(response))
}

pub(crate) async fn call_upstream_resource_target_with_streaming_accept_and_cancellation(
    client: &Client,
    request: UpstreamResourceRequest<'_>,
    downstream_cancellation: &DownstreamCancellation,
) -> Result<UpstreamResourceResponse, DownstreamAwareError<BoxError>> {
    await_with_downstream_cancellation(
        send_upstream_resource_request_preserving_path(
            client,
            request.method,
            request.target,
            request.body,
            request.headers,
            request.accept_event_stream,
            request.resolved_proxy,
        ),
        downstream_cancellation,
    )
    .await
}

#[derive(Debug)]
pub(crate) enum ResponseBodyLimitError<E> {
    Inner(E),
    LimitExceeded { limit: usize },
}

async fn read_response_bytes_limited(
    response: reqwest::Response,
    limit: usize,
) -> Result<bytes::Bytes, ResponseBodyLimitError<reqwest::Error>> {
    let mut stream = response.bytes_stream();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(ResponseBodyLimitError::Inner)?;
        if out.len().saturating_add(chunk.len()) > limit {
            return Err(ResponseBodyLimitError::LimitExceeded { limit });
        }
        out.extend_from_slice(&chunk);
    }
    Ok(bytes::Bytes::from(out))
}

pub(crate) async fn read_response_bytes_limited_with_cancellation(
    response: reqwest::Response,
    limit: usize,
    downstream_cancellation: &DownstreamCancellation,
) -> Result<bytes::Bytes, DownstreamAwareError<ResponseBodyLimitError<reqwest::Error>>> {
    await_with_downstream_cancellation(
        read_response_bytes_limited(response, limit),
        downstream_cancellation,
    )
    .await
}

pub(crate) async fn read_response_text_limited_with_cancellation(
    response: reqwest::Response,
    limit: usize,
    downstream_cancellation: &DownstreamCancellation,
) -> Result<String, DownstreamAwareError<ResponseBodyLimitError<reqwest::Error>>> {
    read_response_bytes_limited_with_cancellation(response, limit, downstream_cancellation)
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
}

async fn read_resource_response_bytes_limited(
    response: UpstreamResourceResponse,
    limit: usize,
) -> Result<bytes::Bytes, ResponseBodyLimitError<BoxError>> {
    let mut stream = response.into_bytes_stream();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(ResponseBodyLimitError::Inner)?;
        if out.len().saturating_add(chunk.len()) > limit {
            return Err(ResponseBodyLimitError::LimitExceeded { limit });
        }
        out.extend_from_slice(&chunk);
    }
    Ok(bytes::Bytes::from(out))
}

pub(crate) async fn read_resource_response_bytes_limited_with_cancellation(
    response: UpstreamResourceResponse,
    limit: usize,
    downstream_cancellation: &DownstreamCancellation,
) -> Result<bytes::Bytes, DownstreamAwareError<ResponseBodyLimitError<BoxError>>> {
    await_with_downstream_cancellation(
        read_resource_response_bytes_limited(response, limit),
        downstream_cancellation,
    )
    .await
}

pub(crate) async fn read_resource_response_text_limited_with_cancellation(
    response: UpstreamResourceResponse,
    limit: usize,
    downstream_cancellation: &DownstreamCancellation,
) -> Result<String, DownstreamAwareError<ResponseBodyLimitError<BoxError>>> {
    read_resource_response_bytes_limited_with_cancellation(response, limit, downstream_cancellation)
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
}

/// Resolve upstream URL for the given format using config base URL.
/// For Google (Gemini), pass the model so the path is .../models/{model}:generateContent.
pub fn upstream_url(
    config: &Config,
    upstream: &UpstreamConfig,
    format: UpstreamFormat,
    model: Option<&str>,
    stream: bool,
) -> String {
    config.upstream_url_for_format(upstream, format, model, stream)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, LazyLock, Mutex};

    use axum::{
        body::Body,
        extract::{Request, State},
        http::{HeaderMap, Method, StatusCode, Uri},
        response::Response,
        routing::any,
        Router,
    };
    use bytes::Bytes;
    use tokio::net::TcpListener;

    use super::*;

    static UPSTREAM_PROXY_ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> =
        LazyLock::new(|| tokio::sync::Mutex::new(()));

    struct ScopedEnvVar {
        key: &'static str,
        previous: Option<String>,
    }

    impl ScopedEnvVar {
        fn set(key: &'static str, value: impl AsRef<str>) -> Self {
            let previous = std::env::var(key).ok();
            std::env::set_var(key, value.as_ref());
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            std::env::remove_var(key);
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

    #[derive(Clone, Default)]
    struct CapturedProxyRequests {
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl CapturedProxyRequests {
        fn push(&self, uri: String) {
            self.requests.lock().expect("proxy request lock").push(uri);
        }

        fn snapshot(&self) -> Vec<String> {
            self.requests.lock().expect("proxy request lock").clone()
        }
    }

    #[derive(Clone)]
    struct ProxyState {
        captured: CapturedProxyRequests,
        client: Client,
    }

    async fn proxy_handler(
        State(state): State<ProxyState>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        request: Request,
    ) -> Response {
        let body_bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
            .await
            .expect("read proxy body");
        let target_url = proxy_target_url(&uri, &headers).expect("proxy target URL");
        state.captured.push(target_url.clone());
        let mut upstream = state.client.request(method, &target_url).body(body_bytes);
        for (name, value) in &headers {
            if name.as_str().eq_ignore_ascii_case("host")
                || name.as_str().eq_ignore_ascii_case("proxy-connection")
            {
                continue;
            }
            upstream = upstream.header(name, value);
        }
        let response = upstream.send().await.expect("proxy upstream response");
        build_proxy_response(response).await
    }

    fn proxy_target_url(uri: &Uri, headers: &HeaderMap) -> Option<String> {
        if uri.scheme_str().is_some() && uri.authority().is_some() {
            return Some(uri.to_string());
        }
        let host = headers.get("host")?.to_str().ok()?;
        let path = uri
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or("/");
        Some(format!("http://{host}{path}"))
    }

    async fn build_proxy_response(response: reqwest::Response) -> Response {
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes().await.unwrap_or_else(|error| {
            Bytes::from(format!("failed to read proxied response body: {error}"))
        });
        let mut builder = Response::builder().status(status);
        for (name, value) in &headers {
            builder = builder.header(name, value);
        }
        builder.body(Body::from(body)).expect("proxy response")
    }

    async fn spawn_forward_proxy() -> (String, CapturedProxyRequests, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
        let addr = listener.local_addr().expect("proxy addr");
        let captured = CapturedProxyRequests::default();
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("proxy client");
        let app = Router::new()
            .route("/", any(proxy_handler))
            .route("/*path", any(proxy_handler))
            .with_state(ProxyState {
                captured: captured.clone(),
                client,
            });
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("proxy server");
        });
        (format!("http://{addr}"), captured, handle)
    }

    async fn spawn_direct_upstream(
    ) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
        #[derive(Clone)]
        struct DirectState {
            requests: Arc<Mutex<Vec<String>>>,
        }

        async fn direct_handler(
            uri: Uri,
            State(state): State<DirectState>,
            request: Request,
        ) -> Response {
            let _body = axum::body::to_bytes(request.into_body(), usize::MAX)
                .await
                .expect("read direct body");
            state
                .requests
                .lock()
                .expect("direct request lock")
                .push(uri.path().to_string());
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from("ok"))
                .expect("direct response")
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind direct upstream");
        let addr = listener.local_addr().expect("direct upstream addr");
        let app = Router::new()
            .route("/", any(direct_handler))
            .route("/*path", any(direct_handler))
            .with_state(DirectState {
                requests: requests.clone(),
            });
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("direct upstream server");
        });
        (format!("http://{addr}"), requests, handle)
    }

    fn test_config(timeout: Duration) -> Config {
        Config {
            listen: "127.0.0.1:0".to_string(),
            upstream_timeout: timeout,
            compatibility_mode: crate::config::CompatibilityMode::Balanced,
            proxy: Some(ProxyConfig::Direct),
            upstreams: Vec::new(),
            model_aliases: Default::default(),
            hooks: Default::default(),
            debug_trace: crate::config::DebugTraceConfig::default(),
            resource_limits: Default::default(),
        }
    }

    #[test]
    fn resolve_upstream_proxy_prefers_upstream_then_namespace_then_environment() {
        let _guard = UPSTREAM_PROXY_ENV_LOCK.blocking_lock();
        let _http_proxy = ScopedEnvVar::set("HTTP_PROXY", "http://env-proxy.example:8080");
        let _http_proxy_lower = ScopedEnvVar::remove("http_proxy");
        let _https_proxy = ScopedEnvVar::remove("HTTPS_PROXY");
        let _https_proxy_lower = ScopedEnvVar::remove("https_proxy");
        let _all_proxy = ScopedEnvVar::remove("ALL_PROXY");
        let _all_proxy_lower = ScopedEnvVar::remove("all_proxy");
        let _no_proxy = ScopedEnvVar::remove("NO_PROXY");
        let _no_proxy_lower = ScopedEnvVar::remove("no_proxy");
        let _request_method = ScopedEnvVar::remove("REQUEST_METHOD");

        assert_eq!(
            resolve_upstream_proxy(
                Some(&ProxyConfig::Proxy {
                    url: "http://upstream-proxy.example:8080".to_string(),
                }),
                Some(&ProxyConfig::Direct),
            ),
            ResolvedProxyMetadata {
                source: ResolvedProxySource::Upstream,
                target: ResolvedProxyTarget::Proxy {
                    url: "http://upstream-proxy.example:8080".to_string(),
                },
            }
        );
        assert_eq!(
            resolve_upstream_proxy(None, Some(&ProxyConfig::Direct)),
            ResolvedProxyMetadata {
                source: ResolvedProxySource::Namespace,
                target: ResolvedProxyTarget::Direct,
            }
        );
        assert_eq!(
            resolve_upstream_proxy(None, None),
            ResolvedProxyMetadata {
                source: ResolvedProxySource::Environment,
                target: ResolvedProxyTarget::Inherited,
            }
        );
    }

    #[test]
    fn resolve_upstream_proxy_without_any_configured_sources_returns_none() {
        let _guard = UPSTREAM_PROXY_ENV_LOCK.blocking_lock();
        let _http_proxy = ScopedEnvVar::remove("HTTP_PROXY");
        let _http_proxy_lower = ScopedEnvVar::remove("http_proxy");
        let _https_proxy = ScopedEnvVar::remove("HTTPS_PROXY");
        let _https_proxy_lower = ScopedEnvVar::remove("https_proxy");
        let _all_proxy = ScopedEnvVar::remove("ALL_PROXY");
        let _all_proxy_lower = ScopedEnvVar::remove("all_proxy");
        let _no_proxy = ScopedEnvVar::remove("NO_PROXY");
        let _no_proxy_lower = ScopedEnvVar::remove("no_proxy");
        let _request_method = ScopedEnvVar::remove("REQUEST_METHOD");

        assert_eq!(
            resolve_upstream_proxy(None, None),
            ResolvedProxyMetadata {
                source: ResolvedProxySource::None,
                target: ResolvedProxyTarget::Inherited,
            }
        );
    }

    #[test]
    fn resolve_upstream_proxy_with_only_no_proxy_returns_none() {
        let _guard = UPSTREAM_PROXY_ENV_LOCK.blocking_lock();
        let _http_proxy = ScopedEnvVar::remove("HTTP_PROXY");
        let _http_proxy_lower = ScopedEnvVar::remove("http_proxy");
        let _https_proxy = ScopedEnvVar::remove("HTTPS_PROXY");
        let _https_proxy_lower = ScopedEnvVar::remove("https_proxy");
        let _all_proxy = ScopedEnvVar::remove("ALL_PROXY");
        let _all_proxy_lower = ScopedEnvVar::remove("all_proxy");
        let _no_proxy = ScopedEnvVar::set("NO_PROXY", "localhost,127.0.0.1");
        let _no_proxy_lower = ScopedEnvVar::set("no_proxy", "localhost,127.0.0.1");
        let _request_method = ScopedEnvVar::remove("REQUEST_METHOD");

        assert_eq!(
            resolve_upstream_proxy(None, None),
            ResolvedProxyMetadata {
                source: ResolvedProxySource::None,
                target: ResolvedProxyTarget::Inherited,
            }
        );
    }

    #[tokio::test]
    async fn build_upstream_clients_without_explicit_proxy_inherits_environment_proxy() {
        let _guard = UPSTREAM_PROXY_ENV_LOCK.lock().await;
        let (target_base, direct_requests, direct_server) = spawn_direct_upstream().await;
        let (env_proxy_base, env_captured, env_proxy_server) = spawn_forward_proxy().await;
        let _http_proxy = ScopedEnvVar::set("HTTP_PROXY", &env_proxy_base);
        let _http_proxy_lower = ScopedEnvVar::set("http_proxy", &env_proxy_base);
        let _https_proxy = ScopedEnvVar::remove("HTTPS_PROXY");
        let _https_proxy_lower = ScopedEnvVar::remove("https_proxy");
        let _all_proxy = ScopedEnvVar::remove("ALL_PROXY");
        let _all_proxy_lower = ScopedEnvVar::remove("all_proxy");
        let _no_proxy = ScopedEnvVar::remove("NO_PROXY");
        let _no_proxy_lower = ScopedEnvVar::remove("no_proxy");
        let _request_method = ScopedEnvVar::remove("REQUEST_METHOD");
        let config = test_config(Duration::from_secs(5));

        let (client, _, resolved_proxy) =
            build_upstream_clients(&config, None, None).expect("environment inherited client");

        let response = call_upstream_resource(
            &client,
            reqwest::Method::POST,
            &format!("{target_base}/resource"),
            None,
            &[],
        )
        .await
        .expect("environment proxied request");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            resolved_proxy,
            ResolvedProxyMetadata {
                source: ResolvedProxySource::Environment,
                target: ResolvedProxyTarget::Inherited,
            }
        );
        assert_eq!(env_captured.snapshot().len(), 1);
        assert_eq!(
            direct_requests.lock().expect("direct request lock").len(),
            1
        );

        direct_server.abort();
        env_proxy_server.abort();
    }

    #[tokio::test]
    async fn build_upstream_clients_explicit_proxy_beats_environment_proxy() {
        let _guard = UPSTREAM_PROXY_ENV_LOCK.lock().await;
        let (target_base, direct_requests, direct_server) = spawn_direct_upstream().await;
        let (env_proxy_base, env_captured, env_proxy_server) = spawn_forward_proxy().await;
        let (explicit_proxy_base, explicit_captured, explicit_proxy_server) =
            spawn_forward_proxy().await;
        let _http_proxy = ScopedEnvVar::set("HTTP_PROXY", &env_proxy_base);
        let _http_proxy_lower = ScopedEnvVar::set("http_proxy", &env_proxy_base);
        let _https_proxy = ScopedEnvVar::remove("HTTPS_PROXY");
        let _https_proxy_lower = ScopedEnvVar::remove("https_proxy");
        let _all_proxy = ScopedEnvVar::remove("ALL_PROXY");
        let _all_proxy_lower = ScopedEnvVar::remove("all_proxy");
        let _no_proxy = ScopedEnvVar::remove("NO_PROXY");
        let _no_proxy_lower = ScopedEnvVar::remove("no_proxy");
        let _request_method = ScopedEnvVar::remove("REQUEST_METHOD");
        let config = test_config(Duration::from_secs(5));

        let (client, _, resolved_proxy) = build_upstream_clients(
            &config,
            Some(&ProxyConfig::Proxy {
                url: explicit_proxy_base.clone(),
            }),
            None,
        )
        .expect("explicit proxy client");

        let response = call_upstream_resource(
            &client,
            reqwest::Method::POST,
            &format!("{target_base}/resource"),
            None,
            &[],
        )
        .await
        .expect("proxied request");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            resolved_proxy,
            ResolvedProxyMetadata {
                source: ResolvedProxySource::Upstream,
                target: ResolvedProxyTarget::Proxy {
                    url: explicit_proxy_base.clone(),
                },
            }
        );
        assert_eq!(env_captured.snapshot(), Vec::<String>::new());
        assert_eq!(explicit_captured.snapshot().len(), 1);
        assert_eq!(
            direct_requests.lock().expect("direct request lock").len(),
            1
        );

        direct_server.abort();
        env_proxy_server.abort();
        explicit_proxy_server.abort();
    }

    #[tokio::test]
    async fn build_upstream_clients_direct_cuts_environment_proxy_inheritance() {
        let _guard = UPSTREAM_PROXY_ENV_LOCK.lock().await;
        let (target_base, direct_requests, direct_server) = spawn_direct_upstream().await;
        let (env_proxy_base, env_captured, env_proxy_server) = spawn_forward_proxy().await;
        let _http_proxy = ScopedEnvVar::set("HTTP_PROXY", &env_proxy_base);
        let _http_proxy_lower = ScopedEnvVar::set("http_proxy", &env_proxy_base);
        let _https_proxy = ScopedEnvVar::remove("HTTPS_PROXY");
        let _https_proxy_lower = ScopedEnvVar::remove("https_proxy");
        let _all_proxy = ScopedEnvVar::remove("ALL_PROXY");
        let _all_proxy_lower = ScopedEnvVar::remove("all_proxy");
        let _no_proxy = ScopedEnvVar::remove("NO_PROXY");
        let _no_proxy_lower = ScopedEnvVar::remove("no_proxy");
        let _request_method = ScopedEnvVar::remove("REQUEST_METHOD");
        let config = test_config(Duration::from_secs(5));

        let (client, _, resolved_proxy) =
            build_upstream_clients(&config, Some(&ProxyConfig::Direct), None)
                .expect("direct upstream client");

        let response = call_upstream_resource(
            &client,
            reqwest::Method::GET,
            &format!("{target_base}/resource"),
            None,
            &[],
        )
        .await
        .expect("direct request");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            resolved_proxy,
            ResolvedProxyMetadata {
                source: ResolvedProxySource::Upstream,
                target: ResolvedProxyTarget::Direct,
            }
        );
        assert_eq!(env_captured.snapshot(), Vec::<String>::new());
        assert_eq!(
            direct_requests.lock().expect("direct request lock").len(),
            1
        );

        direct_server.abort();
        env_proxy_server.abort();
    }
}
