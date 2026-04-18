use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{Response, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::Value;

use crate::config::Config;
use crate::formats::UpstreamFormat;

use super::errors::error_response;
use super::state::{AppState, DEFAULT_NAMESPACE};

pub(super) async fn handle_openai_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    handle_openai_models_inner(state, DEFAULT_NAMESPACE.to_string()).await
}

pub(super) async fn handle_openai_models_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> impl IntoResponse {
    handle_openai_models_inner(state, namespace).await
}

pub(super) async fn handle_openai_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    handle_openai_model_inner(state, DEFAULT_NAMESPACE.to_string(), id).await
}

pub(super) async fn handle_openai_model_namespaced(
    State(state): State<Arc<AppState>>,
    Path((namespace, id)): Path<(String, String)>,
) -> impl IntoResponse {
    handle_openai_model_inner(state, namespace, id).await
}

pub(super) async fn handle_anthropic_models(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    handle_anthropic_models_inner(state, DEFAULT_NAMESPACE.to_string()).await
}

pub(super) async fn handle_anthropic_models_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> impl IntoResponse {
    handle_anthropic_models_inner(state, namespace).await
}

pub(super) async fn handle_anthropic_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    handle_anthropic_model_inner(state, DEFAULT_NAMESPACE.to_string(), id).await
}

pub(super) async fn handle_anthropic_model_namespaced(
    State(state): State<Arc<AppState>>,
    Path((namespace, id)): Path<(String, String)>,
) -> impl IntoResponse {
    handle_anthropic_model_inner(state, namespace, id).await
}

pub(super) async fn handle_google_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    handle_google_models_inner(state, DEFAULT_NAMESPACE.to_string()).await
}

pub(super) async fn handle_google_models_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> impl IntoResponse {
    handle_google_models_inner(state, namespace).await
}

pub(super) async fn handle_google_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    handle_google_model_inner(state, DEFAULT_NAMESPACE.to_string(), id).await
}

pub(super) async fn handle_google_model_namespaced(
    State(state): State<Arc<AppState>>,
    Path((namespace, id)): Path<(String, String)>,
) -> impl IntoResponse {
    handle_google_model_inner(state, namespace, id).await
}

async fn handle_openai_models_inner(state: Arc<AppState>, namespace: String) -> Response<Body> {
    match namespace_config(&state, &namespace).await {
        Some(config) => (StatusCode::OK, Json(openai_model_list(&config))).into_response(),
        None => error_response(
            UpstreamFormat::OpenAiCompletion,
            StatusCode::NOT_FOUND,
            "namespace not found",
        ),
    }
}

async fn handle_openai_model_inner(
    state: Arc<AppState>,
    namespace: String,
    id: String,
) -> Response<Body> {
    let Some(config) = namespace_config(&state, &namespace).await else {
        return error_response(
            UpstreamFormat::OpenAiCompletion,
            StatusCode::NOT_FOUND,
            "namespace not found",
        );
    };
    match openai_model_object(&config, &id) {
        Some(model) => (StatusCode::OK, Json(model)).into_response(),
        None => error_response(
            UpstreamFormat::OpenAiCompletion,
            StatusCode::NOT_FOUND,
            &format!("model `{id}` not found"),
        ),
    }
}

async fn handle_anthropic_models_inner(state: Arc<AppState>, namespace: String) -> Response<Body> {
    match namespace_config(&state, &namespace).await {
        Some(config) => (StatusCode::OK, Json(anthropic_model_list(&config))).into_response(),
        None => error_response(
            UpstreamFormat::Anthropic,
            StatusCode::NOT_FOUND,
            "namespace not found",
        ),
    }
}

async fn handle_anthropic_model_inner(
    state: Arc<AppState>,
    namespace: String,
    id: String,
) -> Response<Body> {
    let Some(config) = namespace_config(&state, &namespace).await else {
        return error_response(
            UpstreamFormat::Anthropic,
            StatusCode::NOT_FOUND,
            "namespace not found",
        );
    };
    match anthropic_model_object(&config, &id) {
        Some(model) => (StatusCode::OK, Json(model)).into_response(),
        None => error_response(
            UpstreamFormat::Anthropic,
            StatusCode::NOT_FOUND,
            &format!("model `{id}` not found"),
        ),
    }
}

async fn handle_google_models_inner(state: Arc<AppState>, namespace: String) -> Response<Body> {
    match namespace_config(&state, &namespace).await {
        Some(config) => (StatusCode::OK, Json(google_model_list(&config))).into_response(),
        None => error_response(
            UpstreamFormat::Google,
            StatusCode::NOT_FOUND,
            "namespace not found",
        ),
    }
}

async fn handle_google_model_inner(
    state: Arc<AppState>,
    namespace: String,
    id: String,
) -> Response<Body> {
    let Some(config) = namespace_config(&state, &namespace).await else {
        return error_response(
            UpstreamFormat::Google,
            StatusCode::NOT_FOUND,
            "namespace not found",
        );
    };
    match google_model_object(&config, &id) {
        Some(model) => (StatusCode::OK, Json(model)).into_response(),
        None => error_response(
            UpstreamFormat::Google,
            StatusCode::NOT_FOUND,
            &format!("model `{id}` not found"),
        ),
    }
}

async fn namespace_config(state: &Arc<AppState>, namespace: &str) -> Option<Config> {
    let runtime = state.runtime.read().await;
    runtime
        .namespaces
        .get(namespace)
        .map(|item| item.config.clone())
}

fn configured_aliases(config: &Config) -> Vec<(&String, &crate::config::ModelAlias)> {
    config.model_aliases.iter().collect()
}

fn openai_model_list(config: &Config) -> Value {
    serde_json::json!({
        "object": "list",
        "data": configured_aliases(config)
            .into_iter()
            .map(|(alias, target)| serde_json::json!({
                "id": alias,
                "object": "model",
                "created": 0,
                "owned_by": "proxec",
                "proxec": {
                    "upstream_name": target.upstream_name,
                    "upstream_model": target.upstream_model,
                }
            }))
            .collect::<Vec<_>>()
    })
}

fn openai_model_object(config: &Config, id: &str) -> Option<Value> {
    let target = config.model_aliases.get(id)?;
    Some(serde_json::json!({
        "id": id,
        "object": "model",
        "created": 0,
        "owned_by": "proxec",
        "proxec": {
            "upstream_name": target.upstream_name,
            "upstream_model": target.upstream_model,
        }
    }))
}

fn anthropic_model_list(config: &Config) -> Value {
    let data = configured_aliases(config)
        .into_iter()
        .map(|(alias, target)| anthropic_model_value(alias, target))
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
    let target = config.model_aliases.get(id)?;
    Some(anthropic_model_value(id, target))
}

fn anthropic_model_value(id: &str, target: &crate::config::ModelAlias) -> Value {
    serde_json::json!({
        "id": id,
        "type": "model",
        "display_name": id,
        "created_at": "1970-01-01T00:00:00Z",
        "proxec": {
            "upstream_name": target.upstream_name,
            "upstream_model": target.upstream_model,
        }
    })
}

fn google_model_list(config: &Config) -> Value {
    serde_json::json!({
        "models": configured_aliases(config)
            .into_iter()
            .map(|(alias, target)| google_model_value(alias, target))
            .collect::<Vec<_>>()
    })
}

fn google_model_object(config: &Config, id: &str) -> Option<Value> {
    let target = config.model_aliases.get(id)?;
    Some(google_model_value(id, target))
}

fn google_model_value(id: &str, target: &crate::config::ModelAlias) -> Value {
    serde_json::json!({
        "name": format!("models/{}", id),
        "baseModelId": id,
        "version": "proxec",
        "displayName": id,
        "description": format!("proxec alias -> {}:{}", target.upstream_name, target.upstream_model),
        "inputTokenLimit": 0,
        "outputTokenLimit": 0,
        "supportedGenerationMethods": ["generateContent"],
        "thinking": false
    })
}
