use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Response, StatusCode},
    response::IntoResponse,
    Extension, Json,
};
use serde_json::Value;

use crate::config::Config;
use crate::formats::UpstreamFormat;

use super::data_auth::{self, RequestAuthContext};
use super::errors::error_response;
use super::secret_redaction::{redactor_for_request, SecretRedactor};
use super::state::{AppState, DEFAULT_NAMESPACE};

const PUBLIC_MODEL_NAMESPACE: &str = "llmup";

pub(super) async fn handle_openai_models(
    State(_state): State<Arc<AppState>>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiCompletion);
    };
    handle_openai_models_inner(&auth_context, DEFAULT_NAMESPACE).await
}

pub(super) async fn handle_openai_models_namespaced(
    State(_state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiCompletion);
    };
    handle_openai_models_inner(&auth_context, &namespace).await
}

pub(super) async fn handle_openai_model(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiCompletion);
    };
    handle_openai_model_inner(&auth_context, DEFAULT_NAMESPACE, &id).await
}

pub(super) async fn handle_openai_model_namespaced(
    State(_state): State<Arc<AppState>>,
    Path((namespace, id)): Path<(String, String)>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiCompletion);
    };
    handle_openai_model_inner(&auth_context, &namespace, &id).await
}

pub(super) async fn handle_anthropic_models(
    State(_state): State<Arc<AppState>>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::Anthropic);
    };
    handle_anthropic_models_inner(&auth_context, DEFAULT_NAMESPACE).await
}

pub(super) async fn handle_anthropic_models_namespaced(
    State(_state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::Anthropic);
    };
    handle_anthropic_models_inner(&auth_context, &namespace).await
}

pub(super) async fn handle_anthropic_model(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::Anthropic);
    };
    handle_anthropic_model_inner(&auth_context, DEFAULT_NAMESPACE, &id).await
}

pub(super) async fn handle_anthropic_model_namespaced(
    State(_state): State<Arc<AppState>>,
    Path((namespace, id)): Path<(String, String)>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::Anthropic);
    };
    handle_anthropic_model_inner(&auth_context, &namespace, &id).await
}

pub(super) async fn handle_google_models(
    State(_state): State<Arc<AppState>>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::Google);
    };
    handle_google_models_inner(&auth_context, DEFAULT_NAMESPACE).await
}

pub(super) async fn handle_google_models_namespaced(
    State(_state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::Google);
    };
    handle_google_models_inner(&auth_context, &namespace).await
}

pub(super) async fn handle_google_model(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::Google);
    };
    handle_google_model_inner(&auth_context, DEFAULT_NAMESPACE, &id).await
}

pub(super) async fn handle_google_model_namespaced(
    State(_state): State<Arc<AppState>>,
    Path((namespace, id)): Path<(String, String)>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::Google);
    };
    handle_google_model_inner(&auth_context, &namespace, &id).await
}

async fn handle_openai_models_inner(
    auth_context: &RequestAuthContext,
    namespace: &str,
) -> Response<Body> {
    let request_redactor = redactor_for_model_request(auth_context);
    match namespace_config(auth_context, namespace) {
        Some(config) => redacted_json_response(
            StatusCode::OK,
            openai_model_list(&config),
            &request_redactor,
        ),
        None => redacted_error_response(
            UpstreamFormat::OpenAiCompletion,
            StatusCode::NOT_FOUND,
            "namespace not found",
            &request_redactor,
        ),
    }
}

async fn handle_openai_model_inner(
    auth_context: &RequestAuthContext,
    namespace: &str,
    id: &str,
) -> Response<Body> {
    let request_redactor = redactor_for_model_request(auth_context);
    let Some(config) = namespace_config(auth_context, namespace) else {
        return redacted_error_response(
            UpstreamFormat::OpenAiCompletion,
            StatusCode::NOT_FOUND,
            "namespace not found",
            &request_redactor,
        );
    };
    match openai_model_object(&config, id) {
        Some(model) => redacted_json_response(StatusCode::OK, model, &request_redactor),
        None => redacted_error_response(
            UpstreamFormat::OpenAiCompletion,
            StatusCode::NOT_FOUND,
            &format!("model `{id}` not found"),
            &request_redactor,
        ),
    }
}

async fn handle_anthropic_models_inner(
    auth_context: &RequestAuthContext,
    namespace: &str,
) -> Response<Body> {
    let request_redactor = redactor_for_model_request(auth_context);
    match namespace_config(auth_context, namespace) {
        Some(config) => redacted_json_response(
            StatusCode::OK,
            anthropic_model_list(&config),
            &request_redactor,
        ),
        None => redacted_error_response(
            UpstreamFormat::Anthropic,
            StatusCode::NOT_FOUND,
            "namespace not found",
            &request_redactor,
        ),
    }
}

async fn handle_anthropic_model_inner(
    auth_context: &RequestAuthContext,
    namespace: &str,
    id: &str,
) -> Response<Body> {
    let request_redactor = redactor_for_model_request(auth_context);
    let Some(config) = namespace_config(auth_context, namespace) else {
        return redacted_error_response(
            UpstreamFormat::Anthropic,
            StatusCode::NOT_FOUND,
            "namespace not found",
            &request_redactor,
        );
    };
    match anthropic_model_object(&config, id) {
        Some(model) => redacted_json_response(StatusCode::OK, model, &request_redactor),
        None => redacted_error_response(
            UpstreamFormat::Anthropic,
            StatusCode::NOT_FOUND,
            &format!("model `{id}` not found"),
            &request_redactor,
        ),
    }
}

async fn handle_google_models_inner(
    auth_context: &RequestAuthContext,
    namespace: &str,
) -> Response<Body> {
    let request_redactor = redactor_for_model_request(auth_context);
    match namespace_config(auth_context, namespace) {
        Some(config) => redacted_json_response(
            StatusCode::OK,
            google_model_list(&config),
            &request_redactor,
        ),
        None => redacted_error_response(
            UpstreamFormat::Google,
            StatusCode::NOT_FOUND,
            "namespace not found",
            &request_redactor,
        ),
    }
}

async fn handle_google_model_inner(
    auth_context: &RequestAuthContext,
    namespace: &str,
    id: &str,
) -> Response<Body> {
    let request_redactor = redactor_for_model_request(auth_context);
    let Some(config) = namespace_config(auth_context, namespace) else {
        return redacted_error_response(
            UpstreamFormat::Google,
            StatusCode::NOT_FOUND,
            "namespace not found",
            &request_redactor,
        );
    };
    match google_model_object(&config, id) {
        Some(model) => redacted_json_response(StatusCode::OK, model, &request_redactor),
        None => redacted_error_response(
            UpstreamFormat::Google,
            StatusCode::NOT_FOUND,
            &format!("model `{id}` not found"),
            &request_redactor,
        ),
    }
}

fn redactor_for_model_request(auth_context: &RequestAuthContext) -> SecretRedactor {
    redactor_for_request(auth_context, &HeaderMap::new())
}

fn redacted_json_response(
    status: StatusCode,
    body: Value,
    redactor: &SecretRedactor,
) -> Response<Body> {
    (status, Json(redactor.redact_value(&body))).into_response()
}

fn redacted_error_response(
    format: UpstreamFormat,
    status: StatusCode,
    message: &str,
    redactor: &SecretRedactor,
) -> Response<Body> {
    error_response(format, status, &redactor.redact_text(message))
}

fn namespace_config(auth_context: &RequestAuthContext, namespace: &str) -> Option<Config> {
    auth_context
        .runtime()
        .namespaces
        .get(namespace)
        .map(|item| item.config.clone())
}

fn configured_aliases(config: &Config) -> Vec<(&String, &crate::config::ModelAlias)> {
    config.model_aliases.iter().collect()
}

fn synthetic_model_alias(config: &Config, id: &str) -> Option<(String, crate::config::ModelAlias)> {
    if let Some(target) = config.model_aliases.get(id) {
        return Some((id.to_string(), target.clone()));
    }

    let resolved = config.resolve_model(id).ok()?;
    Some((
        id.to_string(),
        crate::config::ModelAlias {
            upstream_name: resolved.upstream_name,
            upstream_model: resolved.upstream_model,
            limits: None,
            surface: None,
        },
    ))
}

fn effective_limits(
    config: &Config,
    target: &crate::config::ModelAlias,
) -> Option<crate::config::ModelLimits> {
    config.effective_model_limits(target)
}

fn effective_surface(
    config: &Config,
    target: &crate::config::ModelAlias,
) -> crate::config::ModelSurface {
    config.effective_model_surface(target)
}

fn public_model_metadata(
    config: &Config,
    target: &crate::config::ModelAlias,
) -> (
    Option<crate::config::ModelLimits>,
    crate::config::ModelSurface,
    Value,
) {
    let limits = effective_limits(config, target);
    let surface = effective_surface(config, target);
    let metadata = serde_json::json!({
        "upstream_name": target.upstream_name,
        "upstream_model": target.upstream_model,
        "limits": limits,
        "surface": surface,
    });
    (limits, surface, metadata)
}

fn google_model_route_kind(config: &Config, id: &str) -> &'static str {
    if config.model_aliases.contains_key(id) {
        "alias"
    } else {
        "route"
    }
}

fn google_model_description(
    config: &Config,
    id: &str,
    target: &crate::config::ModelAlias,
) -> String {
    format!(
        "{PUBLIC_MODEL_NAMESPACE} {} -> {}:{}",
        google_model_route_kind(config, id),
        target.upstream_name,
        target.upstream_model
    )
}

fn openai_model_list(config: &Config) -> Value {
    serde_json::json!({
        "object": "list",
        "data": configured_aliases(config)
            .into_iter()
            .map(|(alias, target)| openai_model_value(config, alias, target))
            .collect::<Vec<_>>()
    })
}

fn openai_model_object(config: &Config, id: &str) -> Option<Value> {
    let (model_id, target) = synthetic_model_alias(config, id)?;
    Some(openai_model_value(config, &model_id, &target))
}

fn openai_model_value(config: &Config, id: &str, target: &crate::config::ModelAlias) -> Value {
    let (_limits, _surface, metadata) = public_model_metadata(config, target);
    serde_json::json!({
        "id": id,
        "object": "model",
        "created": 0,
        "owned_by": PUBLIC_MODEL_NAMESPACE,
        "llmup": metadata
    })
}

fn anthropic_model_list(config: &Config) -> Value {
    let data = configured_aliases(config)
        .into_iter()
        .map(|(alias, target)| anthropic_model_value(config, alias, target))
        .collect::<Vec<_>>();
    let first_id = data
        .first()
        .and_then(|model| model.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let last_id = data
        .last()
        .and_then(|model| model.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    serde_json::json!({
        "data": data,
        "has_more": false,
        "first_id": first_id,
        "last_id": last_id
    })
}

fn anthropic_model_object(config: &Config, id: &str) -> Option<Value> {
    let (model_id, target) = synthetic_model_alias(config, id)?;
    Some(anthropic_model_value(config, &model_id, &target))
}

fn anthropic_model_value(config: &Config, id: &str, target: &crate::config::ModelAlias) -> Value {
    let (_limits, _surface, metadata) = public_model_metadata(config, target);
    serde_json::json!({
        "id": id,
        "type": "model",
        "display_name": id,
        "created_at": "1970-01-01T00:00:00Z",
        "llmup": metadata
    })
}

fn google_model_list(config: &Config) -> Value {
    serde_json::json!({
        "models": configured_aliases(config)
            .into_iter()
            .map(|(alias, target)| google_model_value(config, alias, target))
            .collect::<Vec<_>>()
    })
}

fn google_model_object(config: &Config, id: &str) -> Option<Value> {
    let (model_id, target) = synthetic_model_alias(config, id)?;
    Some(google_model_value(config, &model_id, &target))
}

fn google_model_value(config: &Config, id: &str, target: &crate::config::ModelAlias) -> Value {
    let (limits, _surface, metadata) = public_model_metadata(config, target);
    serde_json::json!({
        "name": format!("models/{}", id),
        "baseModelId": id,
        "version": PUBLIC_MODEL_NAMESPACE,
        "displayName": id,
        "description": google_model_description(config, id, target),
        "inputTokenLimit": limits.as_ref().and_then(|item| item.context_window).unwrap_or(0),
        "outputTokenLimit": limits.as_ref().and_then(|item| item.max_output_tokens).unwrap_or(0),
        "supportedGenerationMethods": ["generateContent"],
        "thinking": false,
        "llmup": metadata
    })
}
