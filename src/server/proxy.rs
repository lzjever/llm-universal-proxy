use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, Response, StatusCode},
    response::IntoResponse,
    Extension, Json,
};
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::Value;
use tracing::{debug, error, warn};

use crate::debug_trace::DebugTraceContext;
use crate::downstream::DownstreamCancellation;
use crate::formats::UpstreamFormat;
use crate::hooks::{
    capture_headers, json_response_headers, new_request_id, now_timestamp_ms, sse_response_headers,
    HookRequestContext,
};
use crate::streaming::{needs_stream_translation, GuardedSseStream, TranslateSseStream};
use crate::translate::{
    assess_request_translation_with_surface, translate_request_with_policy,
    translate_response_with_context, RequestTranslationPolicy, ResponseTranslationContext,
    TranslationDecision,
};
use crate::upstream;

use super::body_limits::read_limited_json_request;
use super::errors::{
    append_compatibility_warning_headers, classify_post_translation_non_stream_status,
    client_closed_response, error_response, format_upstream_unavailable_message,
    normalized_non_stream_upstream_error, streaming_error_response,
};
use super::headers::{
    append_upstream_protocol_response_headers, apply_upstream_headers, build_auth_headers,
};
use super::public_boundary::{
    reject_internal_request_scoped_tool_bridge_context, REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD,
};
use super::responses_resources::{
    resolve_native_responses_stateful_route_or_error, responses_stateful_request_controls,
};
use super::state::{AppState, RuntimeNamespaceState, DEFAULT_NAMESPACE};
use super::tracked_body::TrackedBodyStream;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrustedToolBridgeContextEntry {
    stable_name: String,
    source_kind: String,
    transport_kind: String,
    wrapper_field: String,
    expected_canonical_shape: String,
}

impl TrustedToolBridgeContextEntry {
    fn from_value(stable_name: &str, value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let declared_stable_name = object.get("stable_name").and_then(Value::as_str)?;
        if declared_stable_name.is_empty() || declared_stable_name != stable_name {
            return None;
        }
        let source_kind = object.get("source_kind")?.as_str()?;
        let transport_kind = object.get("transport_kind")?.as_str()?;
        let wrapper_field = object.get("wrapper_field")?.as_str()?;
        let expected_canonical_shape = object.get("expected_canonical_shape")?.as_str()?;
        if !matches!(source_kind, "custom_text" | "custom_grammar")
            || transport_kind != "function_object_wrapper"
            || wrapper_field != "input"
            || expected_canonical_shape != "single_required_string"
        {
            return None;
        }
        Some(Self {
            stable_name: stable_name.to_string(),
            source_kind: source_kind.to_string(),
            transport_kind: transport_kind.to_string(),
            wrapper_field: wrapper_field.to_string(),
            expected_canonical_shape: expected_canonical_shape.to_string(),
        })
    }

    fn to_value(&self) -> Value {
        serde_json::json!({
            "stable_name": self.stable_name,
            "source_kind": self.source_kind,
            "transport_kind": self.transport_kind,
            "wrapper_field": self.wrapper_field,
            "expected_canonical_shape": self.expected_canonical_shape
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrustedToolBridgeContext {
    version: u64,
    compatibility_mode: String,
    entries: BTreeMap<String, TrustedToolBridgeContextEntry>,
}

impl TrustedToolBridgeContext {
    fn from_value(value: Value) -> Option<Self> {
        let object = value.as_object()?;
        let version = object.get("version").and_then(Value::as_u64)?;
        if version != 1 {
            return None;
        }
        let compatibility_mode = object.get("compatibility_mode").and_then(Value::as_str)?;
        if !matches!(compatibility_mode, "strict" | "balanced" | "max_compat") {
            return None;
        }
        let entries_object = object.get("entries")?.as_object()?;
        let mut entries = BTreeMap::new();
        for (stable_name, entry_value) in entries_object {
            let entry = TrustedToolBridgeContextEntry::from_value(stable_name, entry_value)?;
            entries.insert(stable_name.clone(), entry);
        }
        if entries.is_empty() {
            return None;
        }
        Some(Self {
            version,
            compatibility_mode: compatibility_mode.to_string(),
            entries,
        })
    }

    fn take_from_body(body: &mut Value) -> Option<Self> {
        let value = body
            .as_object_mut()?
            .remove(REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD)?;
        Self::from_value(value)
    }

    fn to_value(&self) -> Value {
        let entries = self
            .entries
            .iter()
            .map(|(stable_name, entry)| (stable_name.clone(), entry.to_value()))
            .collect::<serde_json::Map<String, Value>>();
        serde_json::json!({
            "version": self.version,
            "compatibility_mode": self.compatibility_mode,
            "entries": entries
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RequestBoundaryDecision {
    Allow,
    AllowWithWarnings(Vec<String>),
    Reject(String),
}

pub(super) async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" })))
}

pub(super) async fn handle_openai_chat_completions(
    State(state): State<Arc<AppState>>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let (headers, body) = match read_limited_json_request(
        &state,
        DEFAULT_NAMESPACE,
        UpstreamFormat::OpenAiCompletion,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_chat_completions_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
    )
    .await
}

pub(super) async fn handle_openai_chat_completions_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let (headers, body) = match read_limited_json_request(
        &state,
        &namespace,
        UpstreamFormat::OpenAiCompletion,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_chat_completions_inner(
        state,
        namespace,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
    )
    .await
}

pub(super) async fn handle_openai_responses(
    State(state): State<Arc<AppState>>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let (headers, body) = match read_limited_json_request(
        &state,
        DEFAULT_NAMESPACE,
        UpstreamFormat::OpenAiResponses,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_responses_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
    )
    .await
}

pub(super) async fn handle_openai_responses_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let (headers, body) = match read_limited_json_request(
        &state,
        &namespace,
        UpstreamFormat::OpenAiResponses,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_responses_inner(
        state,
        namespace,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
    )
    .await
}

pub(super) async fn handle_anthropic_messages(
    State(state): State<Arc<AppState>>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let (headers, body) = match read_limited_json_request(
        &state,
        DEFAULT_NAMESPACE,
        UpstreamFormat::Anthropic,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_anthropic_messages_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
    )
    .await
}

pub(super) async fn handle_anthropic_messages_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let (headers, body) =
        match read_limited_json_request(&state, &namespace, UpstreamFormat::Anthropic, request)
            .await
        {
            Ok(value) => value,
            Err(response) => return response,
        };
    handle_anthropic_messages_inner(
        state,
        namespace,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
    )
    .await
}

pub(super) async fn handle_google_model_action(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let (headers, body) =
        match read_limited_json_request(&state, DEFAULT_NAMESPACE, UpstreamFormat::Google, request)
            .await
        {
            Ok(value) => value,
            Err(response) => return response,
        };
    handle_google_model_action_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        id,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
    )
    .await
}

pub(super) async fn handle_google_model_action_namespaced(
    State(state): State<Arc<AppState>>,
    Path((namespace, id)): Path<(String, String)>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let (headers, body) = match read_limited_json_request(
        &state,
        &namespace,
        UpstreamFormat::Google,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_google_model_action_inner(
        state,
        namespace,
        id,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
    )
    .await
}

async fn handle_openai_chat_completions_inner(
    state: Arc<AppState>,
    namespace: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    body: Value,
) -> Response<Body> {
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    handle_request_core_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        "/openai/v1/chat/completions".to_string(),
        body,
        requested_model,
        UpstreamFormat::OpenAiCompletion,
        None,
    )
    .await
}

async fn handle_openai_responses_inner(
    state: Arc<AppState>,
    namespace: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    body: Value,
) -> Response<Body> {
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    handle_request_core_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        "/openai/v1/responses".to_string(),
        body,
        requested_model,
        UpstreamFormat::OpenAiResponses,
        None,
    )
    .await
}

async fn handle_anthropic_messages_inner(
    state: Arc<AppState>,
    namespace: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    body: Value,
) -> Response<Body> {
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    handle_request_core_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        "/anthropic/v1/messages".to_string(),
        body,
        requested_model,
        UpstreamFormat::Anthropic,
        None,
    )
    .await
}

async fn handle_google_model_action_inner(
    state: Arc<AppState>,
    namespace: String,
    id: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    body: Value,
) -> Response<Body> {
    let Some((requested_model, action)) = id.split_once(':') else {
        return error_response(
            UpstreamFormat::Google,
            StatusCode::BAD_REQUEST,
            "google model action path must end with :generateContent or :streamGenerateContent",
        );
    };
    let forced_stream = match action {
        "generateContent" => false,
        "streamGenerateContent" => true,
        _ => {
            return error_response(
                UpstreamFormat::Google,
                StatusCode::BAD_REQUEST,
                "unsupported google model action",
            );
        }
    };
    handle_request_core_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        format!("/google/v1beta/models/{id}"),
        body,
        requested_model.to_string(),
        UpstreamFormat::Google,
        Some(forced_stream),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(super) async fn handle_request_core(
    state: Arc<AppState>,
    namespace: String,
    headers: HeaderMap,
    path: String,
    body: Value,
    requested_model: String,
    client_format: UpstreamFormat,
    forced_stream: Option<bool>,
) -> Response<Body> {
    handle_request_core_with_downstream_cancellation(
        state,
        namespace,
        DownstreamCancellation::disabled(),
        headers,
        path,
        body,
        requested_model,
        client_format,
        forced_stream,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn handle_request_core_with_downstream_cancellation(
    state: Arc<AppState>,
    namespace: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    path: String,
    mut body: Value,
    requested_model: String,
    client_format: UpstreamFormat,
    forced_stream: Option<bool>,
) -> Response<Body> {
    let request_id = new_request_id();
    let request_timestamp = now_timestamp_ms();
    let original_body = body.clone();
    let stateful_responses_controls = responses_stateful_request_controls(&original_body);
    let original_headers = capture_headers(&headers);
    let stream = forced_stream
        .unwrap_or_else(|| body.get("stream").and_then(Value::as_bool).unwrap_or(false));

    debug!("Request path: {}", path);
    debug!(
        "Request body: {}",
        serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string())
    );

    let namespace_state = {
        let runtime = state.runtime.read().await;
        match runtime.namespaces.get(&namespace) {
            Some(item) => item.clone(),
            None => {
                return error_response(
                    client_format,
                    StatusCode::NOT_FOUND,
                    &format!("namespace `{namespace}` is not configured"),
                );
            }
        }
    };

    let mut tracker = state
        .metrics
        .start_request(path.as_str(), requested_model.clone(), stream);
    if let Some(message) = reject_internal_request_scoped_tool_bridge_context(&original_body) {
        tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
        return error_response(client_format, StatusCode::BAD_REQUEST, &message);
    }
    let resolved_model = match resolve_request_model_or_error(
        &namespace_state,
        &requested_model,
        client_format,
        &original_body,
    ) {
        Ok(v) => v,
        Err(e) => {
            tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
            return error_response(client_format, StatusCode::BAD_REQUEST, &e);
        }
    };
    let upstream_state = match namespace_state.upstreams.get(&resolved_model.upstream_name) {
        Some(v) => v,
        None => {
            tracker.finish_error(StatusCode::INTERNAL_SERVER_ERROR.as_u16());
            return error_response(
                client_format,
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!(
                    "resolved upstream `{}` is not configured",
                    resolved_model.upstream_name
                ),
            );
        }
    };
    tracker.set_upstream(
        resolved_model.upstream_name.clone(),
        resolved_model.upstream_model.clone(),
    );
    let request_translation_policy =
        request_translation_policy(&namespace_state.config, &requested_model, &resolved_model);
    if !upstream_state.availability.is_available() {
        tracker.finish_error(StatusCode::SERVICE_UNAVAILABLE.as_u16());
        return error_response(
            client_format,
            StatusCode::SERVICE_UNAVAILABLE,
            &format_upstream_unavailable_message(
                &resolved_model.upstream_name,
                &upstream_state.availability,
            ),
        );
    }

    let Some(capability) = upstream_state.capability.as_ref() else {
        tracker.finish_error(StatusCode::SERVICE_UNAVAILABLE.as_u16());
        return error_response(
            client_format,
            StatusCode::SERVICE_UNAVAILABLE,
            &format_upstream_unavailable_message(
                &resolved_model.upstream_name,
                &upstream_state.availability,
            ),
        );
    };
    let upstream_format = capability.upstream_format_for_request(client_format);
    if let Some(obj) = body.as_object_mut() {
        if let Some(forced_stream) = forced_stream {
            if !(client_format == UpstreamFormat::Google
                && upstream_format == UpstreamFormat::Google)
            {
                obj.insert("stream".to_string(), Value::Bool(forced_stream));
            }
        }
    }

    let compatibility_warnings = match classify_request_boundary_with_policy(
        client_format,
        upstream_format,
        &original_body,
        &request_translation_policy,
    ) {
        RequestBoundaryDecision::Allow => Vec::new(),
        RequestBoundaryDecision::AllowWithWarnings(warnings) => warnings,
        RequestBoundaryDecision::Reject(message) => {
            tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
            return error_response(client_format, StatusCode::BAD_REQUEST, &message);
        }
    };
    for warning in &compatibility_warnings {
        warn!(
            "compatibility downgrade: client_format={} upstream_format={} warning={}",
            client_format, upstream_format, warning
        );
    }

    if let Err(e) = translate_request_with_policy(
        client_format,
        upstream_format,
        &resolved_model.upstream_model,
        &mut body,
        request_translation_policy,
        stream,
    ) {
        error!("Translation failed: {}", e);
        tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
        return error_response(client_format, StatusCode::BAD_REQUEST, &e);
    }

    if let Some(obj) = body.as_object_mut() {
        match upstream_format {
            UpstreamFormat::Google => {
                obj.remove("model");
            }
            _ if client_format == UpstreamFormat::OpenAiResponses
                && upstream_format == UpstreamFormat::OpenAiResponses
                && requested_model.trim().is_empty()
                && !stateful_responses_controls.is_empty()
                && resolved_model.upstream_model.trim().is_empty() =>
            {
                obj.remove("model");
            }
            _ => {
                obj.insert(
                    "model".to_string(),
                    Value::String(resolved_model.upstream_model.clone()),
                );
            }
        }
    }

    let request_scoped_tool_bridge_context = TrustedToolBridgeContext::take_from_body(&mut body);
    let upstream_request_body = body.clone();

    debug!(
        "Translated body for upstream: {}",
        serde_json::to_string_pretty(&upstream_request_body)
            .unwrap_or_else(|_| upstream_request_body.to_string())
    );

    let (mut auth_headers, effective_credential) =
        build_auth_headers(&headers, upstream_state, upstream_format);
    apply_upstream_headers(
        &mut auth_headers,
        &upstream_state.config.upstream_headers,
        upstream_format,
    );
    let hook_ctx = namespace_state.hooks.as_ref().map(|_| HookRequestContext {
        request_id: request_id.clone(),
        timestamp_ms: request_timestamp,
        path: path.clone(),
        method: "POST".to_string(),
        stream,
        client_model: requested_model.clone(),
        upstream_name: resolved_model.upstream_name.clone(),
        upstream_model: resolved_model.upstream_model.clone(),
        client_format,
        upstream_format,
        credential_source: effective_credential.source,
        credential_fingerprint: effective_credential.fingerprint.clone(),
        client_request_headers: original_headers,
        client_request_body: original_body.clone(),
    });
    let debug_ctx = namespace_state
        .debug_trace
        .as_ref()
        .map(|_| DebugTraceContext {
            request_id: request_id.clone(),
            timestamp_ms: request_timestamp,
            path: path.clone(),
            stream,
            client_model: requested_model.clone(),
            upstream_name: resolved_model.upstream_name.clone(),
            upstream_model: resolved_model.upstream_model.clone(),
            client_format,
            upstream_format,
        });
    if let (Some(recorder), Some(ctx)) = (namespace_state.debug_trace.as_ref(), debug_ctx.as_ref())
    {
        recorder.record_request_with_upstream(ctx, &original_body, &upstream_request_body);
    }

    let url = upstream::upstream_url(
        &namespace_state.config,
        &upstream_state.config,
        upstream_format,
        if upstream_format == UpstreamFormat::Google {
            Some(resolved_model.upstream_model.as_str())
        } else {
            None
        },
        stream,
    );
    debug!("Calling upstream URL: {}", url);
    let upstream_client = if stream {
        upstream_state.streaming_client.clone()
    } else {
        upstream_state.client.clone()
    };
    let res = match upstream::call_upstream_with_cancellation(
        &upstream_client,
        &url,
        &upstream_request_body,
        stream,
        &auth_headers,
        &downstream_cancellation,
    )
    .await
    {
        Ok(r) => r,
        Err(upstream::DownstreamAwareError::Inner(e)) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return if stream {
                streaming_error_response(client_format, StatusCode::BAD_GATEWAY, &e.to_string())
            } else {
                error_response(client_format, StatusCode::BAD_GATEWAY, &e.to_string())
            };
        }
        Err(upstream::DownstreamAwareError::DownstreamCancelled) => {
            tracker.finish_cancelled();
            return client_closed_response(client_format);
        }
    };
    let preserve_native_upstream_protocol_headers = upstream_format == client_format;

    if stream {
        let status = res.status();
        let upstream_response_headers = res.headers().clone();
        debug!("Upstream streaming response status: {}", status);
        if !status.is_success() {
            let error_body = match upstream::read_response_text_limited_with_cancellation(
                res,
                namespace_state
                    .config
                    .resource_limits
                    .max_upstream_error_body_bytes,
                &downstream_cancellation,
            )
            .await
            {
                Ok(body) => body,
                Err(upstream::DownstreamAwareError::Inner(
                    upstream::ResponseBodyLimitError::LimitExceeded { limit },
                )) => {
                    tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
                    return streaming_error_response(
                        client_format,
                        StatusCode::BAD_GATEWAY,
                        &format!("upstream error body exceeded resource limit of {limit} bytes"),
                    );
                }
                Err(upstream::DownstreamAwareError::Inner(
                    upstream::ResponseBodyLimitError::Inner(_),
                )) => "Unknown error".to_string(),
                Err(upstream::DownstreamAwareError::DownstreamCancelled) => {
                    tracker.finish_cancelled();
                    return client_closed_response(client_format);
                }
            };
            error!(
                "Upstream returned error for streaming request: {} - {}",
                status, error_body
            );
            tracker.finish_error(status.as_u16());
            let public_error_body = if serde_json::from_str::<Value>(&error_body).is_ok() {
                error_body
            } else {
                format!("upstream streaming error body: {error_body}")
            };
            let mut response = streaming_error_response(
                client_format,
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                &public_error_body,
            );
            if preserve_native_upstream_protocol_headers {
                append_upstream_protocol_response_headers(
                    &mut response,
                    &upstream_response_headers,
                );
            }
            return response;
        }
        let upstream_stream = res.bytes_stream();
        let mut body_stream: Pin<
            Box<dyn futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send>,
        > = if needs_stream_translation(upstream_format, client_format) {
            let translated =
                TranslateSseStream::new(upstream_stream, upstream_format, client_format)
                    .with_resource_limits(namespace_state.config.resource_limits.clone())
                    .with_request_scoped_tool_bridge_context(
                        request_scoped_tool_bridge_context
                            .as_ref()
                            .map(TrustedToolBridgeContext::to_value),
                    );
            Box::pin(translated.map(|r| r.map_err(std::io::Error::other)))
        } else {
            let guarded = GuardedSseStream::new(upstream_stream, client_format)
                .with_resource_limits(namespace_state.config.resource_limits.clone());
            Box::pin(guarded.map(|r| r.map_err(std::io::Error::other)))
        };
        if let (Some(dispatcher), Some(ctx)) = (namespace_state.hooks.clone(), hook_ctx.clone()) {
            body_stream = Box::pin(dispatcher.wrap_stream(
                body_stream,
                ctx,
                status.as_u16(),
                sse_response_headers(),
            ));
        }
        if let (Some(recorder), Some(ctx)) =
            (namespace_state.debug_trace.as_ref(), debug_ctx.clone())
        {
            body_stream = Box::pin(recorder.wrap_stream(body_stream, ctx, status.as_u16()));
        }
        let body = Body::from_stream(TrackedBodyStream::new(
            body_stream,
            tracker,
            status.as_u16(),
        ));
        let mut response = Response::builder()
            .status(status)
            .header("Content-Type", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .header("Connection", "keep-alive")
            .body(body)
            .unwrap();
        append_upstream_protocol_response_headers(&mut response, &upstream_response_headers);
        append_compatibility_warning_headers(&mut response, &compatibility_warnings);
        return response;
    }

    let status = res.status();
    let upstream_response_headers = res.headers().clone();
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
    let bytes = match upstream::read_response_bytes_limited_with_cancellation(
        res,
        response_body_limit,
        &downstream_cancellation,
    )
    .await
    {
        Ok(b) => b,
        Err(upstream::DownstreamAwareError::Inner(
            upstream::ResponseBodyLimitError::LimitExceeded { limit },
        )) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            let message = if status.is_success() {
                format!("upstream response body exceeded resource limit of {limit} bytes")
            } else {
                format!("upstream error body exceeded resource limit of {limit} bytes")
            };
            return error_response(client_format, StatusCode::BAD_GATEWAY, &message);
        }
        Err(upstream::DownstreamAwareError::Inner(upstream::ResponseBodyLimitError::Inner(e))) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return error_response(client_format, StatusCode::BAD_GATEWAY, &e.to_string());
        }
        Err(upstream::DownstreamAwareError::DownstreamCancelled) => {
            tracker.finish_cancelled();
            return client_closed_response(client_format);
        }
    };
    if !status.is_success() {
        error!("Upstream returned non-success status: {}", status);
        error!(
            "Upstream response body: {}",
            String::from_utf8_lossy(&bytes)
        );
        tracker.finish_error(status.as_u16());
        let upstream_error_body = String::from_utf8_lossy(&bytes);
        let public_error_body = if serde_json::from_str::<Value>(&upstream_error_body).is_ok() {
            upstream_error_body.to_string()
        } else {
            format!("upstream error body: {upstream_error_body}")
        };
        let mut response = error_response(
            client_format,
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            &public_error_body,
        );
        if preserve_native_upstream_protocol_headers {
            append_upstream_protocol_response_headers(&mut response, &upstream_response_headers);
        }
        return response;
    }
    let upstream_body: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            error!(
                "Upstream returned invalid JSON body: {}",
                String::from_utf8_lossy(&bytes)
            );
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return error_response(
                client_format,
                StatusCode::BAD_GATEWAY,
                "upstream returned invalid JSON",
            );
        }
    };
    if let Some((status, message)) =
        normalized_non_stream_upstream_error(upstream_format, client_format, &upstream_body)
    {
        tracker.finish_error(status.as_u16());
        let mut response = error_response(client_format, status, &message);
        if preserve_native_upstream_protocol_headers {
            append_upstream_protocol_response_headers(&mut response, &upstream_response_headers);
        }
        return response;
    }
    let response_translation_context = ResponseTranslationContext::default()
        .with_request_scoped_tool_bridge_context_value(
            request_scoped_tool_bridge_context
                .as_ref()
                .map(TrustedToolBridgeContext::to_value),
        );
    let out = match translate_response_with_context(
        upstream_format,
        client_format,
        &upstream_body,
        response_translation_context,
    ) {
        Ok(v) => v,
        Err(e) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return error_response(client_format, StatusCode::BAD_GATEWAY, &e);
        }
    };
    let response_status = classify_post_translation_non_stream_status(client_format, &out);
    if let (Some(dispatcher), Some(ctx)) = (namespace_state.hooks.as_ref(), hook_ctx) {
        dispatcher.emit_non_stream(
            ctx,
            response_status.as_u16(),
            json_response_headers(),
            out.clone(),
        );
    }
    if let (Some(recorder), Some(ctx)) = (namespace_state.debug_trace.as_ref(), debug_ctx.as_ref())
    {
        recorder.record_non_stream_response(ctx, response_status.as_u16(), &out);
    }
    if response_status.is_success() {
        tracker.finish_success(response_status.as_u16());
    } else {
        tracker.finish_error(response_status.as_u16());
    }
    let mut response = Response::builder()
        .status(response_status)
        .header("Content-Type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&out).unwrap_or_else(|_| b"{}".to_vec()),
        ))
        .unwrap();
    if preserve_native_upstream_protocol_headers {
        append_upstream_protocol_response_headers(&mut response, &upstream_response_headers);
    }
    append_compatibility_warning_headers(&mut response, &compatibility_warnings);
    response
}

#[cfg(test)]
pub(super) fn classify_request_boundary(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
) -> RequestBoundaryDecision {
    classify_request_boundary_with_policy(
        client_format,
        upstream_format,
        body,
        &RequestTranslationPolicy {
            compatibility_mode: crate::config::CompatibilityMode::Balanced,
            surface: crate::config::ModelSurface::default(),
        },
    )
}

fn classify_request_boundary_with_policy(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
    policy: &RequestTranslationPolicy,
) -> RequestBoundaryDecision {
    match assess_request_translation_with_surface(
        client_format,
        upstream_format,
        body,
        policy.compatibility_mode,
        &policy.surface,
    )
    .decision()
    {
        TranslationDecision::Allow => RequestBoundaryDecision::Allow,
        TranslationDecision::AllowWithWarnings(warnings) => {
            RequestBoundaryDecision::AllowWithWarnings(warnings)
        }
        TranslationDecision::Reject(message) => RequestBoundaryDecision::Reject(message),
    }
}

pub(super) fn resolve_requested_model_or_error(
    namespace_config: &crate::config::Config,
    requested_model: &str,
    client_format: UpstreamFormat,
    body: &Value,
) -> Result<crate::config::ResolvedModel, String> {
    if requested_model.trim().is_empty() && namespace_config.upstreams.len() > 1 {
        if client_format == UpstreamFormat::OpenAiResponses
            && body.get("previous_response_id").is_some()
        {
            return Err(
                "Responses requests with `previous_response_id` must also include a routable `model` when this namespace has multiple upstreams; the proxy does not reconstruct response-to-upstream state"
                    .to_string(),
            );
        }

        return Err(
            "request must include a routable `model` when this namespace has multiple upstreams; use `upstream:model` or configure `model_aliases`"
                .to_string(),
        );
    }

    namespace_config.resolve_model(requested_model)
}

fn resolve_request_model_or_error(
    namespace_state: &RuntimeNamespaceState,
    requested_model: &str,
    client_format: UpstreamFormat,
    body: &Value,
) -> Result<crate::config::ResolvedModel, String> {
    if let Some(resolved) = resolve_native_responses_stateful_route_or_error(
        namespace_state,
        requested_model,
        client_format,
        body,
    )? {
        return Ok(resolved);
    }

    resolve_requested_model_or_error(
        &namespace_state.config,
        requested_model,
        client_format,
        body,
    )
}

fn request_translation_policy(
    namespace_config: &crate::config::Config,
    requested_model: &str,
    resolved_model: &crate::config::ResolvedModel,
) -> RequestTranslationPolicy {
    let surface = namespace_config
        .model_aliases
        .get(requested_model)
        .map(|alias| namespace_config.effective_model_surface(alias))
        .unwrap_or_else(|| {
            namespace_config.effective_model_surface(&crate::config::ModelAlias {
                upstream_name: resolved_model.upstream_name.clone(),
                upstream_model: resolved_model.upstream_model.clone(),
                limits: None,
                surface: None,
            })
        });

    RequestTranslationPolicy {
        compatibility_mode: namespace_config.compatibility_mode,
        surface,
    }
}
