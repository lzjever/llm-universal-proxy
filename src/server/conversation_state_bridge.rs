use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::config::ResolvedModel;

use super::data_auth::RequestAuthContext;

pub(super) const LOCAL_RESPONSE_ID_PREFIX: &str = "resp_llmup_";

#[derive(Debug, Clone)]
pub(super) struct StoredBridgeResponse {
    pub(super) namespace: String,
    pub(super) owner_hash: String,
    pub(super) client_model: String,
    pub(super) resolved_model: ResolvedModel,
    pub(super) transcript_items: Vec<Value>,
    expires_at: Instant,
    size_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BridgeLookupError {
    Unknown,
    Expired,
    OwnerMismatch,
}

impl BridgeLookupError {
    pub(super) fn public_message(&self, response_id: &str) -> String {
        match self {
            Self::Unknown | Self::Expired => format!(
                "Responses `previous_response_id` `{response_id}` is unknown or expired in the local conversation_state_bridge memory store"
            ),
            Self::OwnerMismatch => format!(
                "Responses `previous_response_id` `{response_id}` belongs to a different local conversation_state_bridge owner"
            ),
        }
    }
}

#[derive(Debug, Default)]
struct StoreInner {
    responses: HashMap<String, StoredBridgeResponse>,
    current_bytes: usize,
}

#[derive(Debug, Default)]
pub(super) struct ConversationStateBridgeStore {
    inner: Mutex<StoreInner>,
}

impl ConversationStateBridgeStore {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn owner_hash(namespace: &str, auth_context: &RequestAuthContext) -> Option<String> {
        let provider_key = auth_context.client_provider_key()?;
        let mut hasher = Sha256::new();
        hasher.update(namespace.as_bytes());
        hasher.update([0]);
        hasher.update(format!("{:?}", auth_context.mode()).as_bytes());
        hasher.update([0]);
        hasher.update(b"client_provider_key");
        hasher.update([0]);
        hasher.update(provider_key.as_bytes());
        Some(hex::encode(hasher.finalize()))
    }

    pub(super) async fn get(
        &self,
        response_id: &str,
        namespace: &str,
        owner_hash: &str,
    ) -> Result<StoredBridgeResponse, BridgeLookupError> {
        let now = Instant::now();
        let mut inner = self.inner.lock().await;
        let Some(entry) = inner.responses.get(response_id).cloned() else {
            return Err(BridgeLookupError::Unknown);
        };
        if entry.expires_at <= now {
            if let Some(removed) = inner.responses.remove(response_id) {
                inner.current_bytes = inner.current_bytes.saturating_sub(removed.size_bytes);
            }
            return Err(BridgeLookupError::Expired);
        }
        if entry.namespace != namespace || entry.owner_hash != owner_hash {
            return Err(BridgeLookupError::OwnerMismatch);
        }
        Ok(entry)
    }

    pub(super) async fn put(
        &self,
        mut entry: StoredBridgeResponse,
        ttl: Duration,
        max_bytes: usize,
    ) -> Result<String, String> {
        let now = Instant::now();
        entry.expires_at = now + ttl;
        entry.size_bytes = estimate_entry_size(&entry)?;
        if entry.size_bytes > max_bytes {
            return Err(format!(
                "conversation_state_bridge entry requires {} bytes, exceeding max_bytes {max_bytes}",
                entry.size_bytes
            ));
        }

        let mut inner = self.inner.lock().await;
        prune_expired(&mut inner, now);
        if inner.current_bytes.saturating_add(entry.size_bytes) > max_bytes {
            return Err(format!(
                "conversation_state_bridge memory limit would exceed max_bytes {max_bytes}"
            ));
        }

        let id = format!("{LOCAL_RESPONSE_ID_PREFIX}{}", Uuid::new_v4().simple());
        inner.current_bytes += entry.size_bytes;
        inner.responses.insert(id.clone(), entry);
        Ok(id)
    }
}

impl StoredBridgeResponse {
    pub(super) fn new(
        namespace: String,
        owner_hash: String,
        client_model: String,
        resolved_model: ResolvedModel,
        transcript_items: Vec<Value>,
    ) -> Self {
        Self {
            namespace,
            owner_hash,
            client_model,
            resolved_model,
            transcript_items,
            expires_at: Instant::now(),
            size_bytes: 0,
        }
    }
}

fn prune_expired(inner: &mut StoreInner, now: Instant) {
    let expired = inner
        .responses
        .iter()
        .filter_map(|(id, entry)| (entry.expires_at <= now).then_some(id.clone()))
        .collect::<Vec<_>>();
    for id in expired {
        if let Some(removed) = inner.responses.remove(&id) {
            inner.current_bytes = inner.current_bytes.saturating_sub(removed.size_bytes);
        }
    }
}

fn estimate_entry_size(entry: &StoredBridgeResponse) -> Result<usize, String> {
    let transcript = serde_json::to_vec(&entry.transcript_items)
        .map_err(|error| format!("serialize conversation_state_bridge transcript: {error}"))?;
    Ok(transcript.len()
        + entry.namespace.len()
        + entry.owner_hash.len()
        + entry.client_model.len()
        + entry.resolved_model.upstream_name.len()
        + entry.resolved_model.upstream_model.len())
}
