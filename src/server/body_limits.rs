use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{HeaderMap, Response, StatusCode},
};
use serde_json::Value;

use crate::formats::UpstreamFormat;

use super::data_auth::RequestAuthContext;
use super::errors::error_response;
use super::state::AppState;

pub(super) async fn read_limited_json_request(
    _state: &Arc<AppState>,
    namespace: &str,
    client_format: UpstreamFormat,
    auth_context: &RequestAuthContext,
    request: Request,
) -> Result<(HeaderMap, Value), Response<Body>> {
    let max_request_body_bytes = request_body_limit_for_namespace(namespace, auth_context);
    let headers = request.headers().clone();
    let body = match to_bytes(request.into_body(), max_request_body_bytes).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return Err(error_response(
                client_format,
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body exceeded resource limit",
            ));
        }
    };
    let body = serde_json::from_slice(&body).map_err(|_| {
        error_response(
            client_format,
            StatusCode::BAD_REQUEST,
            "invalid JSON request body",
        )
    })?;
    Ok((headers, body))
}

fn request_body_limit_for_namespace(namespace: &str, auth_context: &RequestAuthContext) -> usize {
    auth_context
        .runtime()
        .namespaces
        .get(namespace)
        .map(|namespace_state| {
            namespace_state
                .config
                .resource_limits
                .max_request_body_bytes
        })
        .unwrap_or_else(|| crate::config::ResourceLimits::default().max_request_body_bytes)
}
