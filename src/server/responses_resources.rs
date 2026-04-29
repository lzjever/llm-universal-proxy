use std::pin::Pin;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{OriginalUri, Path, Request, State},
    http::{header, HeaderMap, Response, StatusCode},
    response::IntoResponse,
    Extension,
};
use bytes::Bytes;
use serde_json::Value;
use url::form_urlencoded;

use crate::config::ResourceLimits;
use crate::downstream::DownstreamCancellation;
use crate::formats::UpstreamFormat;
use crate::streaming::GuardedSseStream;
use crate::upstream;

use super::body_limits::read_limited_json_request;
use super::data_auth::{self, RequestAuthContext};
use super::errors::{client_closed_response, error_response, streaming_error_response};
use super::headers::{
    append_upstream_protocol_response_headers, apply_upstream_headers, build_auth_headers,
};
use super::public_boundary::{
    validate_openai_responses_resource_request_body,
    validate_openai_responses_resource_response_body,
};
use super::secret_redaction::{redactor_for_request, RedactingSseStream, SecretRedactor};
use super::state::{AppState, RuntimeNamespaceState, UpstreamState, DEFAULT_NAMESPACE};
use super::tracked_body::TrackedBodyStream;

struct OpenAiResponsesResourceRequest {
    method: reqwest::Method,
    resource_path: String,
    body: Option<Value>,
    query: Option<String>,
}

struct ResponsesResourceRequestAuth {
    headers: HeaderMap,
    auth_context: RequestAuthContext,
}

impl OpenAiResponsesResourceRequest {
    fn new(
        method: reqwest::Method,
        resource_path: String,
        body: Option<Value>,
        query: Option<String>,
    ) -> Self {
        Self {
            method,
            resource_path,
            body,
            query,
        }
    }
}

impl ResponsesResourceRequestAuth {
    fn new(headers: HeaderMap, auth_context: RequestAuthContext) -> Self {
        Self {
            headers,
            auth_context,
        }
    }
}

struct RedactedResponsesResourceRequestMetadata {
    path: String,
    namespace: String,
}

struct RedactedResponsesResourceUpstreamMetadata {
    name: String,
    model: String,
}

impl RedactedResponsesResourceRequestMetadata {
    fn new(redactor: &SecretRedactor, namespace: &str, resource_path: &str) -> Self {
        let resource_path = redactor.redact_text(resource_path);
        Self {
            path: format!("/openai/v1/{resource_path}"),
            namespace: redactor.redact_text(namespace),
        }
    }
}

impl RedactedResponsesResourceUpstreamMetadata {
    fn new(redactor: &SecretRedactor, upstream_name: &str) -> Self {
        Self {
            name: redactor.redact_text(upstream_name),
            model: String::new(),
        }
    }
}

fn redacted_error_response(
    format: UpstreamFormat,
    status: StatusCode,
    message: &str,
    redactor: &SecretRedactor,
) -> Response<Body> {
    error_response(format, status, &redactor.redact_text(message))
}

fn redacted_streaming_error_response(
    format: UpstreamFormat,
    status: StatusCode,
    message: &str,
    redactor: &SecretRedactor,
) -> Response<Body> {
    streaming_error_response(format, status, &redactor.redact_text(message))
}

fn redacted_upstream_unavailable_message(
    redactor: &SecretRedactor,
    upstream: &RedactedResponsesResourceUpstreamMetadata,
    availability: &crate::discovery::UpstreamAvailability,
) -> String {
    match availability {
        crate::discovery::UpstreamAvailability::Available => {
            format!("resolved upstream `{}` is unavailable", upstream.name)
        }
        crate::discovery::UpstreamAvailability::Unavailable { reason } => {
            let reason = redactor.redact_text(reason);
            format!(
                "resolved upstream `{}` is unavailable: {reason}",
                upstream.name
            )
        }
    }
}

fn downstream_cancellation_or_disabled(
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
) -> DownstreamCancellation {
    downstream_cancellation
        .map(|Extension(cancellation)| cancellation)
        .unwrap_or_else(DownstreamCancellation::disabled)
}

pub(super) async fn handle_openai_responses_compact(
    State(state): State<Arc<AppState>>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        DEFAULT_NAMESPACE,
        UpstreamFormat::OpenAiResponses,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_responses_compact_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_responses_compact_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        &namespace,
        UpstreamFormat::OpenAiResponses,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_responses_compact_inner(
        state,
        namespace,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_response_get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(response_id): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_response_get_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        uri,
        response_id,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_response_get_namespaced(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((namespace, response_id)): Path<(String, String)>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_response_get_inner(
        state,
        namespace,
        uri,
        response_id,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_response_input_items(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(response_id): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_response_input_items_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        uri,
        response_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        headers,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_response_input_items_namespaced(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((namespace, response_id)): Path<(String, String)>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_response_input_items_inner(
        state,
        namespace,
        uri,
        response_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        headers,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_responses_input_tokens(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        DEFAULT_NAMESPACE,
        UpstreamFormat::OpenAiResponses,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_responses_input_tokens_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        uri,
        downstream_cancellation_or_disabled(downstream_cancellation),
        headers,
        body,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_responses_input_tokens_namespaced(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(namespace): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        &namespace,
        UpstreamFormat::OpenAiResponses,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_responses_input_tokens_inner(
        state,
        namespace,
        uri,
        downstream_cancellation_or_disabled(downstream_cancellation),
        headers,
        body,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_response_delete(
    State(state): State<Arc<AppState>>,
    Path(response_id): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_response_delete_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        response_id,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_response_delete_namespaced(
    State(state): State<Arc<AppState>>,
    Path((namespace, response_id)): Path<(String, String)>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_response_delete_inner(
        state,
        namespace,
        response_id,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_response_cancel(
    State(state): State<Arc<AppState>>,
    Path(response_id): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_response_cancel_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        response_id,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_response_cancel_namespaced(
    State(state): State<Arc<AppState>>,
    Path((namespace, response_id)): Path<(String, String)>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_response_cancel_inner(
        state,
        namespace,
        response_id,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_conversations_create(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        DEFAULT_NAMESPACE,
        UpstreamFormat::OpenAiResponses,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_conversations_create_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        uri,
        downstream_cancellation_or_disabled(downstream_cancellation),
        headers,
        body,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_conversations_create_namespaced(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(namespace): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        &namespace,
        UpstreamFormat::OpenAiResponses,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_conversations_create_inner(
        state,
        namespace,
        uri,
        downstream_cancellation_or_disabled(downstream_cancellation),
        headers,
        body,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_conversation_get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(conversation_id): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_conversation_get_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        uri,
        conversation_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        headers,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_conversation_get_namespaced(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((namespace, conversation_id)): Path<(String, String)>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_conversation_get_inner(
        state,
        namespace,
        uri,
        conversation_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        headers,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_conversation_update(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(conversation_id): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        DEFAULT_NAMESPACE,
        UpstreamFormat::OpenAiResponses,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_conversation_update_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        uri,
        conversation_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        ResponsesResourceRequestAuth::new(headers, auth_context),
        body,
    )
    .await
}

pub(super) async fn handle_openai_conversation_update_namespaced(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((namespace, conversation_id)): Path<(String, String)>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        &namespace,
        UpstreamFormat::OpenAiResponses,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_conversation_update_inner(
        state,
        namespace,
        uri,
        conversation_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        ResponsesResourceRequestAuth::new(headers, auth_context),
        body,
    )
    .await
}

pub(super) async fn handle_openai_conversation_delete(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(conversation_id): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_conversation_delete_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        uri,
        conversation_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        headers,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_conversation_delete_namespaced(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((namespace, conversation_id)): Path<(String, String)>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_conversation_delete_inner(
        state,
        namespace,
        uri,
        conversation_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        headers,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_conversation_items(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(conversation_id): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_conversation_items_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        uri,
        conversation_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        headers,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_conversation_items_namespaced(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((namespace, conversation_id)): Path<(String, String)>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_conversation_items_inner(
        state,
        namespace,
        uri,
        conversation_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        headers,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_conversation_item_create(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(conversation_id): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        DEFAULT_NAMESPACE,
        UpstreamFormat::OpenAiResponses,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_conversation_item_create_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        uri,
        conversation_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        ResponsesResourceRequestAuth::new(headers, auth_context),
        body,
    )
    .await
}

pub(super) async fn handle_openai_conversation_item_create_namespaced(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((namespace, conversation_id)): Path<(String, String)>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        &namespace,
        UpstreamFormat::OpenAiResponses,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_conversation_item_create_inner(
        state,
        namespace,
        uri,
        conversation_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        ResponsesResourceRequestAuth::new(headers, auth_context),
        body,
    )
    .await
}

pub(super) async fn handle_openai_conversation_item_get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((conversation_id, item_id)): Path<(String, String)>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_conversation_item_get_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        uri,
        conversation_id,
        item_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        ResponsesResourceRequestAuth::new(headers, auth_context),
    )
    .await
}

pub(super) async fn handle_openai_conversation_item_get_namespaced(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((namespace, conversation_id, item_id)): Path<(String, String, String)>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_conversation_item_get_inner(
        state,
        namespace,
        uri,
        conversation_id,
        item_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        ResponsesResourceRequestAuth::new(headers, auth_context),
    )
    .await
}

pub(super) async fn handle_openai_conversation_item_delete(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((conversation_id, item_id)): Path<(String, String)>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_conversation_item_delete_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        uri,
        conversation_id,
        item_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        ResponsesResourceRequestAuth::new(headers, auth_context),
    )
    .await
}

pub(super) async fn handle_openai_conversation_item_delete_namespaced(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((namespace, conversation_id, item_id)): Path<(String, String, String)>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    handle_openai_conversation_item_delete_inner(
        state,
        namespace,
        uri,
        conversation_id,
        item_id,
        downstream_cancellation_or_disabled(downstream_cancellation),
        ResponsesResourceRequestAuth::new(headers, auth_context),
    )
    .await
}

async fn handle_openai_responses_compact_inner(
    state: Arc<AppState>,
    namespace: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    body: Value,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        OpenAiResponsesResourceRequest::new(
            reqwest::Method::POST,
            "responses/compact".to_string(),
            Some(body),
            None,
        ),
        auth_context,
    )
    .await
}

async fn handle_openai_response_get_inner(
    state: Arc<AppState>,
    namespace: String,
    uri: axum::http::Uri,
    response_id: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        OpenAiResponsesResourceRequest::new(
            reqwest::Method::GET,
            format!("responses/{}", encode_resource_path_segment(&response_id)),
            None,
            uri.query().map(ToString::to_string),
        ),
        auth_context,
    )
    .await
}

async fn handle_openai_response_input_items_inner(
    state: Arc<AppState>,
    namespace: String,
    uri: axum::http::Uri,
    response_id: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        OpenAiResponsesResourceRequest::new(
            reqwest::Method::GET,
            format!(
                "responses/{}/input_items",
                encode_resource_path_segment(&response_id)
            ),
            None,
            uri.query().map(ToString::to_string),
        ),
        auth_context,
    )
    .await
}

async fn handle_openai_responses_input_tokens_inner(
    state: Arc<AppState>,
    namespace: String,
    uri: axum::http::Uri,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    body: Value,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        OpenAiResponsesResourceRequest::new(
            reqwest::Method::POST,
            "responses/input_tokens".to_string(),
            Some(body),
            uri.query().map(ToString::to_string),
        ),
        auth_context,
    )
    .await
}

async fn handle_openai_response_delete_inner(
    state: Arc<AppState>,
    namespace: String,
    response_id: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        OpenAiResponsesResourceRequest::new(
            reqwest::Method::DELETE,
            format!("responses/{}", encode_resource_path_segment(&response_id)),
            None,
            None,
        ),
        auth_context,
    )
    .await
}

async fn handle_openai_response_cancel_inner(
    state: Arc<AppState>,
    namespace: String,
    response_id: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        OpenAiResponsesResourceRequest::new(
            reqwest::Method::POST,
            format!(
                "responses/{}/cancel",
                encode_resource_path_segment(&response_id)
            ),
            None,
            None,
        ),
        auth_context,
    )
    .await
}

async fn handle_openai_conversations_create_inner(
    state: Arc<AppState>,
    namespace: String,
    uri: axum::http::Uri,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    body: Value,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        OpenAiResponsesResourceRequest::new(
            reqwest::Method::POST,
            "conversations".to_string(),
            Some(body),
            uri.query().map(ToString::to_string),
        ),
        auth_context,
    )
    .await
}

async fn handle_openai_conversation_get_inner(
    state: Arc<AppState>,
    namespace: String,
    uri: axum::http::Uri,
    conversation_id: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        OpenAiResponsesResourceRequest::new(
            reqwest::Method::GET,
            format!(
                "conversations/{}",
                encode_resource_path_segment(&conversation_id)
            ),
            None,
            uri.query().map(ToString::to_string),
        ),
        auth_context,
    )
    .await
}

async fn handle_openai_conversation_update_inner(
    state: Arc<AppState>,
    namespace: String,
    uri: axum::http::Uri,
    conversation_id: String,
    downstream_cancellation: DownstreamCancellation,
    request_auth: ResponsesResourceRequestAuth,
    body: Value,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        request_auth.headers,
        OpenAiResponsesResourceRequest::new(
            reqwest::Method::POST,
            format!(
                "conversations/{}",
                encode_resource_path_segment(&conversation_id)
            ),
            Some(body),
            uri.query().map(ToString::to_string),
        ),
        request_auth.auth_context,
    )
    .await
}

async fn handle_openai_conversation_delete_inner(
    state: Arc<AppState>,
    namespace: String,
    uri: axum::http::Uri,
    conversation_id: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        OpenAiResponsesResourceRequest::new(
            reqwest::Method::DELETE,
            format!(
                "conversations/{}",
                encode_resource_path_segment(&conversation_id)
            ),
            None,
            uri.query().map(ToString::to_string),
        ),
        auth_context,
    )
    .await
}

async fn handle_openai_conversation_items_inner(
    state: Arc<AppState>,
    namespace: String,
    uri: axum::http::Uri,
    conversation_id: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        OpenAiResponsesResourceRequest::new(
            reqwest::Method::GET,
            format!(
                "conversations/{}/items",
                encode_resource_path_segment(&conversation_id)
            ),
            None,
            uri.query().map(ToString::to_string),
        ),
        auth_context,
    )
    .await
}

async fn handle_openai_conversation_item_create_inner(
    state: Arc<AppState>,
    namespace: String,
    uri: axum::http::Uri,
    conversation_id: String,
    downstream_cancellation: DownstreamCancellation,
    request_auth: ResponsesResourceRequestAuth,
    body: Value,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        request_auth.headers,
        OpenAiResponsesResourceRequest::new(
            reqwest::Method::POST,
            format!(
                "conversations/{}/items",
                encode_resource_path_segment(&conversation_id)
            ),
            Some(body),
            uri.query().map(ToString::to_string),
        ),
        request_auth.auth_context,
    )
    .await
}

async fn handle_openai_conversation_item_get_inner(
    state: Arc<AppState>,
    namespace: String,
    uri: axum::http::Uri,
    conversation_id: String,
    item_id: String,
    downstream_cancellation: DownstreamCancellation,
    request_auth: ResponsesResourceRequestAuth,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        request_auth.headers,
        OpenAiResponsesResourceRequest::new(
            reqwest::Method::GET,
            format!(
                "conversations/{}/items/{}",
                encode_resource_path_segment(&conversation_id),
                encode_resource_path_segment(&item_id)
            ),
            None,
            uri.query().map(ToString::to_string),
        ),
        request_auth.auth_context,
    )
    .await
}

async fn handle_openai_conversation_item_delete_inner(
    state: Arc<AppState>,
    namespace: String,
    uri: axum::http::Uri,
    conversation_id: String,
    item_id: String,
    downstream_cancellation: DownstreamCancellation,
    request_auth: ResponsesResourceRequestAuth,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        request_auth.headers,
        OpenAiResponsesResourceRequest::new(
            reqwest::Method::DELETE,
            format!(
                "conversations/{}/items/{}",
                encode_resource_path_segment(&conversation_id),
                encode_resource_path_segment(&item_id)
            ),
            None,
            uri.query().map(ToString::to_string),
        ),
        request_auth.auth_context,
    )
    .await
}

#[cfg(test)]
pub(super) async fn handle_openai_responses_resource(
    state: Arc<AppState>,
    namespace: String,
    headers: HeaderMap,
    method: reqwest::Method,
    resource_path: String,
    body: Option<Value>,
    query: Option<String>,
) -> Response<Body> {
    let auth_context = trusted_test_request_auth_context(&state, &headers).await;
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        DownstreamCancellation::disabled(),
        headers,
        OpenAiResponsesResourceRequest::new(method, resource_path, body, query),
        auth_context,
    )
    .await
}

#[cfg(test)]
pub(super) struct TestOpenAiResponsesResourceRequest {
    pub(super) method: reqwest::Method,
    pub(super) resource_path: String,
    pub(super) body: Option<Value>,
    pub(super) query: Option<String>,
}

#[cfg(test)]
pub(super) async fn handle_openai_responses_resource_with_auth_context(
    state: Arc<AppState>,
    namespace: String,
    headers: HeaderMap,
    request: TestOpenAiResponsesResourceRequest,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    handle_openai_responses_resource_with_downstream_cancellation(
        state,
        namespace,
        DownstreamCancellation::disabled(),
        headers,
        OpenAiResponsesResourceRequest::new(
            request.method,
            request.resource_path,
            request.body,
            request.query,
        ),
        auth_context,
    )
    .await
}

#[cfg(test)]
async fn trusted_test_request_auth_context(
    state: &Arc<AppState>,
    headers: &HeaderMap,
) -> RequestAuthContext {
    let runtime = state.runtime.read().await.clone();
    let access = state.data_auth_policy.current_access().await;
    let (mode, authorization) = match &access {
        data_auth::DataAccess::ClientProviderKey => (
            crate::config::DataAuthMode::ClientProviderKey,
            data_auth::RequestAuthorization::ClientProviderKey {
                provider_key: test_client_provider_key_from_headers(headers)
                    .unwrap_or_else(|| "test-client-provider-key".to_string()),
            },
        ),
        data_auth::DataAccess::ProxyKey { .. } => (
            crate::config::DataAuthMode::ProxyKey,
            data_auth::RequestAuthorization::ProxyKey,
        ),
        data_auth::DataAccess::Unconfigured => (
            crate::config::DataAuthMode::ClientProviderKey,
            data_auth::RequestAuthorization::ClientProviderKey {
                provider_key: "test-client-provider-key".to_string(),
            },
        ),
        data_auth::DataAccess::Misconfigured(_) => (
            crate::config::DataAuthMode::ClientProviderKey,
            data_auth::RequestAuthorization::ClientProviderKey {
                provider_key: "test-client-provider-key".to_string(),
            },
        ),
    };
    RequestAuthContext::for_test("test-generation", mode, access, authorization, runtime)
}

#[cfg(test)]
fn test_client_provider_key_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .get(..7)
                .filter(|prefix| prefix.eq_ignore_ascii_case("Bearer "))
                .map(|_| value[7..].to_string())
        })
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            [
                "x-api-key",
                "api-key",
                "openai-api-key",
                "x-goog-api-key",
                "anthropic-api-key",
            ]
            .into_iter()
            .find_map(|name| {
                headers
                    .get(axum::http::HeaderName::from_static(name))
                    .and_then(|value| value.to_str().ok())
                    .filter(|value| !value.trim().is_empty())
                    .map(ToString::to_string)
            })
        })
}

async fn handle_openai_responses_resource_with_downstream_cancellation(
    state: Arc<AppState>,
    namespace: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    request: OpenAiResponsesResourceRequest,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    let OpenAiResponsesResourceRequest {
        method,
        resource_path,
        body,
        query,
    } = request;
    let stream_resource = is_streamed_responses_retrieve(&method, &resource_path, query.as_deref());
    let request_redactor = redactor_for_request(&auth_context, &headers);
    let redacted_request_metadata = RedactedResponsesResourceRequestMetadata::new(
        &request_redactor,
        &namespace,
        &resource_path,
    );
    let mut tracker = state.metrics.start_request(
        redacted_request_metadata.path.clone(),
        String::new(),
        stream_resource,
    );
    if let Some(body) = body.as_ref() {
        if let Err(message) = validate_openai_responses_resource_request_body(body) {
            tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
            return redacted_error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_REQUEST,
                &message,
                &request_redactor,
            );
        }
    }
    let namespace_state = {
        match auth_context.runtime().namespaces.get(&namespace) {
            Some(item) => item.clone(),
            None => {
                tracker.finish_error(StatusCode::NOT_FOUND.as_u16());
                return redacted_error_response(
                    UpstreamFormat::OpenAiResponses,
                    StatusCode::NOT_FOUND,
                    &format!(
                        "namespace `{}` is not configured",
                        redacted_request_metadata.namespace
                    ),
                    &request_redactor,
                );
            }
        }
    };

    if responses_owner_provenance_is_ambiguous(&namespace_state) {
        tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
        return redacted_error_response(
            UpstreamFormat::OpenAiResponses,
            StatusCode::BAD_REQUEST,
            &responses_auto_discovery_ambiguity_message("Responses lifecycle endpoints"),
            &request_redactor,
        );
    }

    let matching = provenance_free_native_responses_upstreams(&namespace_state);

    let upstream_state = match matching.as_slice() {
        [upstream] => *upstream,
        [] => {
            tracker.finish_error(StatusCode::SERVICE_UNAVAILABLE.as_u16());
            return redacted_error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::SERVICE_UNAVAILABLE,
                "Responses lifecycle endpoints require an available upstream that natively supports OpenAI Responses",
                &request_redactor,
            );
        }
        _ => {
            tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
            return redacted_error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_REQUEST,
                "Responses lifecycle endpoint is ambiguous across multiple Responses-capable upstreams in this namespace",
                &request_redactor,
            );
        }
    };

    let redacted_upstream_metadata = RedactedResponsesResourceUpstreamMetadata::new(
        &request_redactor,
        &upstream_state.config.name,
    );
    tracker.set_upstream(
        redacted_upstream_metadata.name.clone(),
        redacted_upstream_metadata.model.clone(),
    );
    if !upstream_state.availability.is_available() {
        tracker.finish_error(StatusCode::SERVICE_UNAVAILABLE.as_u16());
        return redacted_error_response(
            UpstreamFormat::OpenAiResponses,
            StatusCode::SERVICE_UNAVAILABLE,
            &redacted_upstream_unavailable_message(
                &request_redactor,
                &redacted_upstream_metadata,
                &upstream_state.availability,
            ),
            &request_redactor,
        );
    }
    let (mut auth_headers, _effective_credential) = match build_auth_headers(
        &headers,
        &auth_context,
        upstream_state,
        UpstreamFormat::OpenAiResponses,
    ) {
        Ok(value) => value,
        Err(message) => {
            tracker.finish_error(StatusCode::SERVICE_UNAVAILABLE.as_u16());
            return redacted_error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::SERVICE_UNAVAILABLE,
                &message,
                &request_redactor,
            );
        }
    };
    apply_upstream_headers(
        &mut auth_headers,
        &upstream_state.config.upstream_headers,
        UpstreamFormat::OpenAiResponses,
    );

    let target = match upstream::build_upstream_resource_target(
        &upstream_state.config.api_root,
        &resource_path,
        query.as_deref(),
    ) {
        Ok(target) => target,
        Err(message) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return redacted_error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_GATEWAY,
                &message,
                &request_redactor,
            );
        }
    };

    let upstream_client = if stream_resource {
        &upstream_state.streaming_client
    } else {
        &upstream_state.no_auto_decompression_client
    };
    let response =
        match upstream::call_upstream_resource_target_with_streaming_accept_and_cancellation(
            upstream_client,
            upstream::UpstreamResourceRequest {
                method,
                target: &target,
                body: body.as_ref(),
                headers: &auth_headers,
                accept_event_stream: stream_resource,
                resolved_proxy: &upstream_state.resolved_proxy,
            },
            &downstream_cancellation,
        )
        .await
        {
            Ok(response) => response,
            Err(upstream::DownstreamAwareError::Inner(error)) => {
                tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
                return redacted_error_response(
                    UpstreamFormat::OpenAiResponses,
                    StatusCode::BAD_GATEWAY,
                    &error.to_string(),
                    &request_redactor,
                );
            }
            Err(upstream::DownstreamAwareError::DownstreamCancelled) => {
                tracker.finish_cancelled();
                return client_closed_response(UpstreamFormat::OpenAiResponses);
            }
        };

    let status = response.status();
    let upstream_response_headers = response.headers().clone();
    if stream_resource {
        return handle_openai_responses_resource_stream_response(
            response,
            status,
            upstream_response_headers,
            namespace_state.config.resource_limits.clone(),
            request_redactor,
            tracker,
            downstream_cancellation,
        )
        .await;
    }
    if status_allows_empty_success_body(status)
        && no_content_response_framing_is_invalid(&upstream_response_headers)
    {
        tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
        return redacted_error_response(
            UpstreamFormat::OpenAiResponses,
            StatusCode::BAD_GATEWAY,
            "upstream returned invalid no-content response framing",
            &request_redactor,
        );
    }
    let response_body_limit = if status.is_success() {
        namespace_state
            .config
            .resource_limits
            .max_non_stream_response_bytes
    } else {
        namespace_state
            .config
            .resource_limits
            .max_upstream_error_body_bytes
    };
    let bytes = match upstream::read_resource_response_bytes_limited_with_cancellation(
        response,
        response_body_limit,
        &downstream_cancellation,
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(upstream::DownstreamAwareError::Inner(
            upstream::ResponseBodyLimitError::LimitExceeded { limit },
        )) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            let message = if status.is_success() {
                format!("upstream response body exceeded resource limit of {limit} bytes")
            } else {
                format!("upstream error body exceeded resource limit of {limit} bytes")
            };
            return redacted_error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_GATEWAY,
                &message,
                &request_redactor,
            );
        }
        Err(upstream::DownstreamAwareError::Inner(upstream::ResponseBodyLimitError::Inner(
            error,
        ))) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return redacted_error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
                &request_redactor,
            );
        }
        Err(upstream::DownstreamAwareError::DownstreamCancelled) => {
            tracker.finish_cancelled();
            return client_closed_response(UpstreamFormat::OpenAiResponses);
        }
    };

    let response_body_bytes;
    if status.is_success() {
        if bytes.is_empty() {
            if status_allows_empty_success_body(status) {
                tracker.finish_success(status.as_u16());
                let mut response = Response::builder()
                    .status(status)
                    .body(Body::empty())
                    .unwrap_or_else(|_| {
                        redacted_error_response(
                            UpstreamFormat::OpenAiResponses,
                            StatusCode::BAD_GATEWAY,
                            "failed to build upstream resource response",
                            &request_redactor,
                        )
                    });
                append_upstream_protocol_response_headers(
                    &mut response,
                    &upstream_response_headers,
                    &request_redactor,
                );
                return response;
            }

            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return redacted_error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_GATEWAY,
                "upstream returned empty response body",
                &request_redactor,
            );
        }

        if status_allows_empty_success_body(status) {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return redacted_error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_GATEWAY,
                "upstream returned unexpected body for no-content response",
                &request_redactor,
            );
        }

        let upstream_body = match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => value,
            Err(_) => {
                tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
                return redacted_error_response(
                    UpstreamFormat::OpenAiResponses,
                    StatusCode::BAD_GATEWAY,
                    "upstream returned invalid JSON",
                    &request_redactor,
                );
            }
        };
        if let Err(message) = validate_openai_responses_resource_response_body(&upstream_body) {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return redacted_error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_GATEWAY,
                &message,
                &request_redactor,
            );
        }
        let public_body = request_redactor.redact_value(&upstream_body);
        response_body_bytes = serde_json::to_vec(&public_body).unwrap_or_else(|_| b"{}".to_vec());
        tracker.finish_success(status.as_u16());
    } else {
        tracker.finish_error(status.as_u16());
        let upstream_error_body = String::from_utf8_lossy(&bytes);
        let public_error_body = if serde_json::from_str::<Value>(&upstream_error_body).is_ok() {
            upstream_error_body.to_string()
        } else {
            format!("upstream resource error body: {upstream_error_body}")
        };
        let public_error_body = request_redactor.redact_text(&public_error_body);
        return redacted_error_response(
            UpstreamFormat::OpenAiResponses,
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            &public_error_body,
            &request_redactor,
        );
    }

    let mut response = Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::from(response_body_bytes))
        .unwrap_or_else(|_| {
            redacted_error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_GATEWAY,
                "failed to build upstream resource response",
                &request_redactor,
            )
        });
    append_upstream_protocol_response_headers(
        &mut response,
        &upstream_response_headers,
        &request_redactor,
    );
    response
}

async fn handle_openai_responses_resource_stream_response(
    response: upstream::UpstreamResourceResponse,
    status: StatusCode,
    upstream_response_headers: reqwest::header::HeaderMap,
    resource_limits: ResourceLimits,
    request_redactor: SecretRedactor,
    mut tracker: crate::telemetry::RequestTracker,
    downstream_cancellation: DownstreamCancellation,
) -> Response<Body> {
    if !status.is_success() {
        let error_body = match upstream::read_resource_response_text_limited_with_cancellation(
            response,
            resource_limits.max_upstream_error_body_bytes,
            &downstream_cancellation,
        )
        .await
        {
            Ok(body) => body,
            Err(upstream::DownstreamAwareError::Inner(
                upstream::ResponseBodyLimitError::LimitExceeded { limit },
            )) => {
                tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
                let mut response = redacted_streaming_error_response(
                    UpstreamFormat::OpenAiResponses,
                    StatusCode::BAD_GATEWAY,
                    &format!("upstream error body exceeded resource limit of {limit} bytes"),
                    &request_redactor,
                );
                append_upstream_protocol_response_headers(
                    &mut response,
                    &upstream_response_headers,
                    &request_redactor,
                );
                return response;
            }
            Err(upstream::DownstreamAwareError::Inner(
                upstream::ResponseBodyLimitError::Inner(_),
            )) => "Unknown error".to_string(),
            Err(upstream::DownstreamAwareError::DownstreamCancelled) => {
                tracker.finish_cancelled();
                return client_closed_response(UpstreamFormat::OpenAiResponses);
            }
        };
        tracker.finish_error(status.as_u16());
        let public_error_body = if serde_json::from_str::<Value>(&error_body).is_ok() {
            error_body
        } else {
            format!("upstream streaming resource error body: {error_body}")
        };
        let public_error_body = request_redactor.redact_text(&public_error_body);
        let mut response = redacted_streaming_error_response(
            UpstreamFormat::OpenAiResponses,
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            &public_error_body,
            &request_redactor,
        );
        append_upstream_protocol_response_headers(
            &mut response,
            &upstream_response_headers,
            &request_redactor,
        );
        return response;
    }

    if !response_is_event_stream(&upstream_response_headers) {
        tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
        return redacted_streaming_error_response(
            UpstreamFormat::OpenAiResponses,
            StatusCode::BAD_GATEWAY,
            "upstream returned non-SSE response for streamed Responses resource",
            &request_redactor,
        );
    }

    let guarded = GuardedSseStream::new(
        response.into_bytes_stream(),
        UpstreamFormat::OpenAiResponses,
    )
    .with_resource_limits(resource_limits);
    let body_stream: Pin<
        Box<dyn futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send>,
    > = Box::pin(RedactingSseStream::new(
        Box::pin(guarded),
        request_redactor.clone(),
    ));
    let body = Body::from_stream(TrackedBodyStream::new(
        body_stream,
        tracker,
        status.as_u16(),
    ));
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(body)
        .unwrap_or_else(|_| {
            redacted_error_response(
                UpstreamFormat::OpenAiResponses,
                StatusCode::BAD_GATEWAY,
                "failed to build upstream resource stream response",
                &request_redactor,
            )
        });
    append_upstream_protocol_response_headers(
        &mut response,
        &upstream_response_headers,
        &request_redactor,
    );
    response
}

fn status_allows_empty_success_body(status: StatusCode) -> bool {
    matches!(status, StatusCode::NO_CONTENT | StatusCode::RESET_CONTENT)
}

fn is_streamed_responses_retrieve(
    method: &reqwest::Method,
    resource_path: &str,
    query: Option<&str>,
) -> bool {
    if *method != reqwest::Method::GET || !resource_path_is_response_retrieve(resource_path) {
        return false;
    }
    query
        .map(|query| {
            form_urlencoded::parse(query.as_bytes()).any(|(name, value)| {
                name.eq_ignore_ascii_case("stream") && value.eq_ignore_ascii_case("true")
            })
        })
        .unwrap_or(false)
}

fn resource_path_is_response_retrieve(resource_path: &str) -> bool {
    let Some(response_id) = resource_path.strip_prefix("responses/") else {
        return false;
    };
    !response_id.is_empty() && !response_id.contains('/')
}

fn encode_resource_path_segment(segment: &str) -> String {
    if segment == "." {
        return "%2E".to_string();
    }
    if segment == ".." {
        return "%2E%2E".to_string();
    }

    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'='
            | b':'
            | b'@' => {
                encoded.push(*byte as char);
            }
            _ => {
                let _ = std::fmt::Write::write_fmt(&mut encoded, format_args!("%{byte:02X}"));
            }
        }
    }
    encoded
}

fn response_is_event_stream(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|media_type| media_type.trim().eq_ignore_ascii_case("text/event-stream"))
        .unwrap_or(false)
}

fn no_content_response_framing_is_invalid(headers: &reqwest::header::HeaderMap) -> bool {
    headers.contains_key(reqwest::header::TRANSFER_ENCODING)
        || !content_length_allows_no_content(headers)
}

fn content_length_allows_no_content(headers: &reqwest::header::HeaderMap) -> bool {
    for value in headers.get_all(reqwest::header::CONTENT_LENGTH).iter() {
        let Ok(value) = value.to_str() else {
            return false;
        };
        for part in value.split(',') {
            let part = part.trim_matches(|char| char == ' ' || char == '\t');
            if part.is_empty()
                || !part.as_bytes().iter().all(|byte| byte.is_ascii_digit())
                || part.as_bytes().iter().any(|byte| *byte != b'0')
            {
                return false;
            }
        }
    }
    true
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
