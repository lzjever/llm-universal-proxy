use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::config::Config;

const MAX_RECENT_REQUESTS: usize = 12;

#[derive(Debug)]
pub struct RuntimeMetrics {
    started_at: Instant,
    inner: Mutex<MetricsInner>,
}

#[derive(Debug, Default)]
struct MetricsInner {
    total_requests: u64,
    active_requests: u64,
    total_stream_requests: u64,
    active_stream_requests: u64,
    success_responses: u64,
    error_responses: u64,
    cancelled_responses: u64,
    per_upstream: BTreeMap<String, UpstreamMetrics>,
    recent_requests: VecDeque<RecentRequest>,
}

#[derive(Debug, Clone, Default)]
pub struct UpstreamMetrics {
    pub total_requests: u64,
    pub active_requests: u64,
    pub success_responses: u64,
    pub error_responses: u64,
    pub cancelled_responses: u64,
    pub stream_requests: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestOutcome {
    Success,
    Error,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct RecentRequest {
    pub path: String,
    pub client_model: String,
    pub upstream_name: Option<String>,
    pub upstream_model: Option<String>,
    pub stream: bool,
    pub status: u16,
    pub outcome: RequestOutcome,
    pub duration_ms: u128,
}

#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub uptime_secs: u64,
    pub total_requests: u64,
    pub active_requests: u64,
    pub total_stream_requests: u64,
    pub active_stream_requests: u64,
    pub success_responses: u64,
    pub error_responses: u64,
    pub cancelled_responses: u64,
    pub upstreams: Vec<(String, UpstreamMetrics)>,
    pub recent_requests: Vec<RecentRequest>,
    pub configured_aliases: usize,
    pub configured_upstreams: usize,
}

#[derive(Debug)]
pub struct RequestTracker {
    metrics: Arc<RuntimeMetrics>,
    start: Instant,
    path: String,
    client_model: String,
    upstream_name: Option<String>,
    upstream_model: Option<String>,
    stream: bool,
    finished: bool,
}

impl RuntimeMetrics {
    pub fn new(_config: &Config) -> Arc<Self> {
        Arc::new(Self {
            started_at: Instant::now(),
            inner: Mutex::new(MetricsInner::default()),
        })
    }

    pub fn start_request(
        self: &Arc<Self>,
        path: impl Into<String>,
        client_model: impl Into<String>,
        stream: bool,
    ) -> RequestTracker {
        let path = path.into();
        let client_model = client_model.into();
        {
            let mut inner = self.inner.lock().unwrap();
            inner.total_requests = inner.total_requests.saturating_add(1);
            inner.active_requests = inner.active_requests.saturating_add(1);
            if stream {
                inner.total_stream_requests = inner.total_stream_requests.saturating_add(1);
                inner.active_stream_requests = inner.active_stream_requests.saturating_add(1);
            }
        }
        RequestTracker {
            metrics: self.clone(),
            start: Instant::now(),
            path,
            client_model,
            upstream_name: None,
            upstream_model: None,
            stream,
            finished: false,
        }
    }

    pub fn snapshot(&self, config: &Config) -> MetricsSnapshot {
        let inner = self.inner.lock().unwrap();
        MetricsSnapshot {
            uptime_secs: self.started_at.elapsed().as_secs(),
            total_requests: inner.total_requests,
            active_requests: inner.active_requests,
            total_stream_requests: inner.total_stream_requests,
            active_stream_requests: inner.active_stream_requests,
            success_responses: inner.success_responses,
            error_responses: inner.error_responses,
            cancelled_responses: inner.cancelled_responses,
            upstreams: inner
                .per_upstream
                .iter()
                .map(|(name, stats)| (name.clone(), stats.clone()))
                .collect(),
            recent_requests: inner.recent_requests.iter().cloned().collect(),
            configured_aliases: config.model_aliases.len(),
            configured_upstreams: config.upstreams.len(),
        }
    }
}

impl RequestTracker {
    pub fn set_upstream(
        &mut self,
        upstream_name: impl Into<String>,
        upstream_model: impl Into<String>,
    ) {
        let upstream_name = upstream_name.into();
        let upstream_model = upstream_model.into();
        if self.upstream_name.as_deref() == Some(upstream_name.as_str())
            && self.upstream_model.as_deref() == Some(upstream_model.as_str())
        {
            return;
        }

        let mut inner = self.metrics.inner.lock().unwrap();
        if let Some(previous) = self.upstream_name.as_deref() {
            if let Some(entry) = inner.per_upstream.get_mut(previous) {
                entry.active_requests = entry.active_requests.saturating_sub(1);
            }
        }

        let entry = inner.per_upstream.entry(upstream_name.clone()).or_default();
        entry.total_requests = entry.total_requests.saturating_add(1);
        entry.active_requests = entry.active_requests.saturating_add(1);
        if self.stream {
            entry.stream_requests = entry.stream_requests.saturating_add(1);
        }

        self.upstream_name = Some(upstream_name);
        self.upstream_model = Some(upstream_model);
    }

    pub fn finish_success(&mut self, status: u16) {
        self.finish_with(RequestOutcome::Success, status);
    }

    pub fn finish_error(&mut self, status: u16) {
        self.finish_with(RequestOutcome::Error, status);
    }

    pub fn finish_cancelled(&mut self) {
        self.finish_with(RequestOutcome::Cancelled, 499);
    }

    fn finish_with(&mut self, outcome: RequestOutcome, status: u16) {
        if self.finished {
            return;
        }
        self.finished = true;

        let duration_ms = self.start.elapsed().as_millis();
        let mut inner = self.metrics.inner.lock().unwrap();
        inner.active_requests = inner.active_requests.saturating_sub(1);
        if self.stream {
            inner.active_stream_requests = inner.active_stream_requests.saturating_sub(1);
        }
        match outcome {
            RequestOutcome::Success => {
                inner.success_responses = inner.success_responses.saturating_add(1);
            }
            RequestOutcome::Error => {
                inner.error_responses = inner.error_responses.saturating_add(1);
            }
            RequestOutcome::Cancelled => {
                inner.cancelled_responses = inner.cancelled_responses.saturating_add(1);
            }
        }
        if let Some(name) = self.upstream_name.as_deref() {
            if let Some(entry) = inner.per_upstream.get_mut(name) {
                entry.active_requests = entry.active_requests.saturating_sub(1);
                match outcome {
                    RequestOutcome::Success => {
                        entry.success_responses = entry.success_responses.saturating_add(1);
                    }
                    RequestOutcome::Error => {
                        entry.error_responses = entry.error_responses.saturating_add(1);
                    }
                    RequestOutcome::Cancelled => {
                        entry.cancelled_responses = entry.cancelled_responses.saturating_add(1);
                    }
                }
            }
        }

        inner.recent_requests.push_front(RecentRequest {
            path: self.path.clone(),
            client_model: self.client_model.clone(),
            upstream_name: self.upstream_name.clone(),
            upstream_model: self.upstream_model.clone(),
            stream: self.stream,
            status,
            outcome,
            duration_ms,
        });
        while inner.recent_requests.len() > MAX_RECENT_REQUESTS {
            inner.recent_requests.pop_back();
        }
    }
}

impl Drop for RequestTracker {
    fn drop(&mut self) {
        if !self.finished {
            self.finish_cancelled();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RuntimeMetrics;
    use crate::Config;

    #[test]
    fn request_tracker_updates_snapshot() {
        let config = Config::default();
        let metrics = RuntimeMetrics::new(&config);
        let mut tracker = metrics.start_request("/openai/v1/responses", "sonnet", true);
        tracker.set_upstream("GLM", "GLM-5");
        tracker.finish_success(200);

        let snapshot = metrics.snapshot(&config);
        assert_eq!(snapshot.total_requests, 1);
        assert_eq!(snapshot.active_requests, 0);
        assert_eq!(snapshot.total_stream_requests, 1);
        assert_eq!(snapshot.success_responses, 1);
        assert_eq!(snapshot.error_responses, 0);
        assert_eq!(snapshot.cancelled_responses, 0);
        assert_eq!(snapshot.upstreams.len(), 1);
        assert_eq!(snapshot.recent_requests.len(), 1);
    }

    #[test]
    fn request_tracker_drop_records_cancelled() {
        let config = Config::default();
        let metrics = RuntimeMetrics::new(&config);
        let mut tracker = metrics.start_request("/openai/v1/responses", "sonnet", true);
        tracker.set_upstream("GLM", "GLM-5");
        drop(tracker);

        let snapshot = metrics.snapshot(&config);
        assert_eq!(snapshot.success_responses, 0);
        assert_eq!(snapshot.error_responses, 0);
        assert_eq!(snapshot.cancelled_responses, 1);
        assert_eq!(snapshot.upstreams[0].1.cancelled_responses, 1);
        assert_eq!(snapshot.recent_requests[0].status, 499);
        assert_eq!(
            snapshot.recent_requests[0].outcome,
            super::RequestOutcome::Cancelled
        );
    }
}
