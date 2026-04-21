use std::sync::Arc;

use axum::{
    body::Body,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, Response, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::Value;

use crate::formats::UpstreamFormat;
use crate::upstream;

use super::errors::{error_response, format_upstream_unavailable_message};
use super::headers::{
    append_upstream_protocol_response_headers, apply_upstream_headers, build_auth_headers,
};
use super::state::{AppState, RuntimeNamespaceState, UpstreamState, DEFAULT_NAMESPACE};

pub(super) async fn handle_openai_responses_compact(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_openai_responses_compact_inner(state, DEFAULT_NAMESPACE.to_string(), headers, body).await
}

pub(super) async fn handle_openai_responses_compact_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    handle_openai_responses_compact_inner(state, namespace, headers, body).await
}

pub(super) async fn handle_openai_response_get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(response_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    handle_openai_response_get_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        uri,
        response_id,
        headers,
    )
    .await
}

pub(super) async fn handle_openai_response_get_namespaced(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((namespace, response_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    handle_openai_response_get_inner(state, namespace, uri, response_id, headers).await
}

pub(super) async fn handle_openai_response_delete(
    State(state): State<Arc<AppState>>,
    Path(response_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    handle_openai_response_delete_inner(state, DEFAULT_NAMESPACE.to_string(), response_id, headers)
        .await
}

pub(super) async fn handle_openai_response_delete_namespaced(
    State(state): State<Arc<AppState>>,
    Path((namespace, response_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    handle_openai_response_delete_inner(state, namespace, response_id, headers).await
}

pub(super) async fn handle_openai_response_cancel(
    State(state): State<Arc<AppState>>,
    Path(response_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    handle_openai_response_cancel_inner(state, DEFAULT_NAMESPACE.to_string(), response_id, headers)
        .await
}

pub(super) async fn handle_openai_response_cancel_namespaced(
    State(state): State<Arc<AppState>>,
    Path((namespace, response_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    handle_openai_response_cancel_inner(state, namespace, response_id, headers).await
}

async fn handle_openai_responses_compact_inner(
    state: Arc<AppState>,
    namespace: String,
    headers: HeaderMap,
    body: Value,
) -> Response<Body> {
    handle_openai_responses_resource(
        state,
        namespace,
        headers,
        reqwest::Method::POST,
        "responses/compact".to_string(),
        Some(body),
        None,
    )
    .await
}

async fn handle_openai_response_get_inner(
    state: Arc<AppState>,
    namespace: String,
    uri: axum::http::Uri,
    response_id: String,
    headers: HeaderMap,
) -> Response<Body> {
    handle_openai_responses_resource(
        state,
        namespace,
        headers,
        reqwest::Method::GET,
        format!("responses/{response_id}"),
        None,
        uri.query().map(ToString::to_string),
    )
    .await
}

async fn handle_openai_response_delete_inner(
    state: Arc<AppState>,
    namespace: String,
    response_id: String,
    headers: HeaderMap,
) -> Response<Body> {
    handle_openai_responses_resource(
        state,
        namespace,
        headers,
        reqwest::Method::DELETE,
        format!("responses/{response_id}"),
        None,
        None,
    )
    .await
}

async fn handle_openai_response_cancel_inner(
    state: Arc<AppState>,
    namespace: String,
    response_id: String,
    headers: HeaderMap,
) -> Response<Body> {
    handle_openai_responses_resource(
        state,
        namespace,
        headers,
        reqwest::Method::POST,
        format!("responses/{response_id}/cancel"),
        None,
        None,
    )
    .await
}

pub(super) async fn handle_openai_responses_resource(
    state: Arc<AppState>,
    namespace: String,
    headers: HeaderMap,
    method: reqwest::Method,
    resource_path: String,
    body: Option<Value>,
    query: Option<String>,
) -> Response<Body> {
    let request_path = format!("/openai/v1/{resource_path}");
    let mut tracker = state
        .metrics
        .start_request(&request_path, String::new(), false);
    let namespace_state = {
        let runtime = state.runtime.read().await;
        match runtime.namespaces.get(&namespace) {
            Some(item) => item.clone(),
            None => {
                tracker.finish_error(StatusCode::NOT_FOUND.as_u16());
                return error_response(
                    UpstreamFormat::OpenAiResponses,
                    StatusCode::NOT_FOUND,
                    &format!("namespace `{namespace}` is not configured"),
                );
            }
        }
    };

    if responses_owner_provenance_is_ambiguous(&namespace_state) {
        tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
        return error_response(
            UpstreamFormat::OpenAiResponses,
            StatusCode::BAD_REQUEST,
            &responses_auto_discovery_ambiguity_message("Responses lifecycle endpoints"),
        );
    }

    let matching = provenance_free_native_responses_upstreams(&namespace_state);

    let upstream_state = match matching.as_slice() {
        [upstream] => *upstream,
        [] => {
            tracker.finish_error(StatusCode::SERVICE_UNAVAILABLE.as_u16());
            return error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::SERVICE_UNAVAILABLE,
                "Responses lifecycle endpoints require an available upstream that natively supports OpenAI Responses",
            );
        }
        _ => {
            tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
            return error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_REQUEST,
                "Responses lifecycle endpoint is ambiguous across multiple Responses-capable upstreams in this namespace",
            );
        }
    };

    tracker.set_upstream(upstream_state.config.name.clone(), String::new());
    if !upstream_state.availability.is_available() {
        tracker.finish_error(StatusCode::SERVICE_UNAVAILABLE.as_u16());
        return error_response(
            UpstreamFormat::OpenAiResponses,
            StatusCode::SERVICE_UNAVAILABLE,
            &format_upstream_unavailable_message(
                &upstream_state.config.name,
                &upstream_state.availability,
            ),
        );
    }
    let (mut auth_headers, _effective_credential) =
        build_auth_headers(&headers, upstream_state, UpstreamFormat::OpenAiResponses);
    apply_upstream_headers(
        &mut auth_headers,
        &upstream_state.config.upstream_headers,
        UpstreamFormat::OpenAiResponses,
    );

    let mut url =
        crate::config::build_upstream_resource_url(&upstream_state.config.api_root, &resource_path);
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        url.push('?');
        url.push_str(&query);
    }

    let response = match upstream::call_upstream_resource(
        &upstream_state.client,
        method,
        &url,
        body.as_ref(),
        &auth_headers,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            );
        }
    };

    let status = response.status();
    let upstream_response_headers = response.headers().clone();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            );
        }
    };

    if status.is_success() {
        tracker.finish_success(status.as_u16());
    } else {
        tracker.finish_error(status.as_u16());
    }

    let mut response = Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| {
            error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_GATEWAY,
                "failed to build upstream resource response",
            )
        });
    append_upstream_protocol_response_headers(&mut response, &upstream_response_headers);
    response
}

pub(super) fn resolve_native_responses_stateful_route_or_error(
    namespace_state: &RuntimeNamespaceState,
    requested_model: &str,
    client_format: UpstreamFormat,
    body: &Value,
) -> Result<Option<crate::config::ResolvedModel>, String> {
    if client_format != UpstreamFormat::OpenAiResponses || !requested_model.trim().is_empty() {
        return Ok(None);
    }

    let stateful_controls = responses_stateful_request_controls(body);
    if stateful_controls.is_empty() {
        return Ok(None);
    }

    let quoted_controls = quoted_field_list(&stateful_controls);
    if responses_owner_provenance_is_ambiguous(namespace_state) {
        return Err(format!(
            "Responses requests with stateful controls {quoted_controls} must include a routable `model` in namespaces that use auto-discovery; set `fixed_upstream_format` on the owning upstream because provenance-free routing cannot rely on discovery-time capabilities"
        ));
    }

    let matching = provenance_free_native_responses_upstreams(namespace_state);
    match matching.as_slice() {
        [upstream] => Ok(Some(crate::config::ResolvedModel {
            upstream_name: upstream.config.name.clone(),
            upstream_model: String::new(),
        })),
        [] => Err(format!(
            "Responses requests with stateful controls {quoted_controls} require exactly one configured native OpenAI Responses upstream when `model` is omitted; the proxy does not reconstruct provider state"
        )),
        _ => Err(format!(
            "Responses requests with stateful controls {quoted_controls} must include a routable `model` when this namespace has multiple configured native OpenAI Responses upstreams; the proxy does not reconstruct response-to-upstream state"
        )),
    }
}

fn pinned_native_responses_upstreams(
    namespace_state: &RuntimeNamespaceState,
) -> Vec<&UpstreamState> {
    namespace_state
        .upstreams
        .values()
        .filter(|upstream| {
            upstream.config.fixed_upstream_format == Some(UpstreamFormat::OpenAiResponses)
        })
        .collect()
}

pub(super) fn provenance_free_native_responses_upstreams(
    namespace_state: &RuntimeNamespaceState,
) -> Vec<&UpstreamState> {
    if namespace_state.config.upstreams.len() == 1 {
        return namespace_state
            .upstreams
            .values()
            .filter(|upstream| {
                upstream.capability.as_ref().is_some_and(|capability| {
                    capability
                        .supported
                        .contains(&UpstreamFormat::OpenAiResponses)
                })
            })
            .collect();
    }

    pinned_native_responses_upstreams(namespace_state)
}

pub(super) fn responses_owner_provenance_is_ambiguous(
    namespace_state: &RuntimeNamespaceState,
) -> bool {
    namespace_state.config.upstreams.len() > 1
        && namespace_state
            .config
            .upstreams
            .iter()
            .any(|upstream| upstream.fixed_upstream_format.is_none())
}

pub(super) fn responses_auto_discovery_ambiguity_message(request_kind: &str) -> String {
    format!(
        "{request_kind} are ambiguous in multi-upstream namespaces that use auto-discovery; set `fixed_upstream_format` on the owning upstream or route explicitly because provenance-free routing cannot rely on discovery-time capabilities"
    )
}

pub(super) fn responses_stateful_request_controls(body: &Value) -> Vec<&'static str> {
    let mut controls = Vec::new();
    if body.get("previous_response_id").is_some() {
        controls.push("previous_response_id");
    }
    if body.get("conversation").is_some() {
        controls.push("conversation");
    }
    if control_is_enabled(body.get("background")) {
        controls.push("background");
    }
    if control_is_enabled(body.get("store")) {
        controls.push("store");
    }
    if body.get("prompt").is_some() {
        controls.push("prompt");
    }
    controls
}

fn control_is_enabled(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(false)) | None | Some(Value::Null) => false,
        Some(_) => true,
    }
}

pub(super) fn quoted_field_list(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|field| format!("`{field}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
