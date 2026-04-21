use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::Response,
    routing::any,
    Router,
};
use bytes::Bytes;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedForwardProxyRequest {
    pub method: String,
    pub uri: String,
    pub body: Vec<u8>,
}

#[derive(Clone, Default)]
pub struct CapturedForwardProxyRequests {
    requests: Arc<Mutex<Vec<CapturedForwardProxyRequest>>>,
}

impl CapturedForwardProxyRequests {
    fn push(&self, request: CapturedForwardProxyRequest) {
        self.requests.lock().unwrap().push(request);
    }

    pub fn snapshot(&self) -> Vec<CapturedForwardProxyRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub async fn wait_for_count(
        &self,
        minimum: usize,
        timeout: Duration,
    ) -> Vec<CapturedForwardProxyRequest> {
        let deadline = Instant::now() + timeout;
        loop {
            let snapshot = self.snapshot();
            if snapshot.len() >= minimum {
                return snapshot;
            }
            if Instant::now() >= deadline {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

#[derive(Clone)]
struct ForwardProxyState {
    captured: CapturedForwardProxyRequests,
    client: reqwest::Client,
}

pub async fn spawn_http_forward_proxy() -> (
    String,
    tokio::task::JoinHandle<()>,
    CapturedForwardProxyRequests,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let captured = CapturedForwardProxyRequests::default();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let state = ForwardProxyState {
        captured: captured.clone(),
        client,
    };
    let app = Router::new()
        .route("/", any(forward_proxy_handler))
        .route("/*path", any(forward_proxy_handler))
        .with_state(state);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle, captured)
}

async fn forward_proxy_handler(
    State(state): State<ForwardProxyState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let body_bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from(format!("failed to read proxy body: {error}")))
                .unwrap();
        }
    };

    let Some(target_url) = proxy_target_url(&uri, &headers) else {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from("proxy request missing absolute target URL"))
            .unwrap();
    };

    state.captured.push(CapturedForwardProxyRequest {
        method: method.to_string(),
        uri: target_url.clone(),
        body: body_bytes.clone().to_vec(),
    });

    let mut upstream = state
        .client
        .request(method.clone(), &target_url)
        .body(body_bytes);
    for (name, value) in &headers {
        if name.as_str().eq_ignore_ascii_case("host")
            || name.as_str().eq_ignore_ascii_case("proxy-connection")
        {
            continue;
        }
        upstream = upstream.header(name, value);
    }

    let response = match upstream.send().await {
        Ok(response) => response,
        Err(error) => {
            return Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Body::from(format!("forward proxy upstream error: {error}")))
                .unwrap();
        }
    };

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
    let body = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => Bytes::from(format!("failed to read proxied response body: {error}")),
    };
    let mut builder = Response::builder().status(status);
    for (name, value) in &headers {
        builder = builder.header(name, value);
    }
    builder.body(Body::from(body)).unwrap()
}
