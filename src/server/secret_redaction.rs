use axum::http::{header, HeaderMap, HeaderName};
use bytes::Bytes;
use futures_util::Stream;
use serde_json::Value;
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::data_auth::{DataAccess, RequestAuthContext};
use super::state::RuntimeState;

pub(super) const REDACTED_SECRET: &str = "[REDACTED]";

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct SecretRedactor {
    secrets: Vec<String>,
}

impl std::fmt::Debug for SecretRedactor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretRedactor")
            .field("secret_count", &self.secrets.len())
            .finish()
    }
}

impl SecretRedactor {
    pub(super) fn new<I, S>(secrets: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut secrets = secrets
            .into_iter()
            .map(Into::into)
            .filter(|secret| should_redact_known_secret(secret))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        secrets.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
        Self { secrets }
    }

    pub(super) fn redact_text(&self, value: &str) -> String {
        let mut redacted = value.to_string();
        for secret in &self.secrets {
            if redacted.contains(secret) {
                redacted = redacted.replace(secret, REDACTED_SECRET);
            }
        }
        redacted
    }

    pub(super) fn redact_value(&self, value: &Value) -> Value {
        match value {
            Value::String(value) => Value::String(self.redact_text(value)),
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .map(|value| self.redact_value(value))
                    .collect(),
            ),
            Value::Object(values) => Value::Object(
                values
                    .iter()
                    .map(|(key, value)| (self.redact_text(key), self.redact_value(value)))
                    .collect(),
            ),
            Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        }
    }
}

pub(super) struct RedactingSseStream<S> {
    inner: S,
    redactor: SecretRedactor,
    buffer: Vec<u8>,
    output_queue: VecDeque<Bytes>,
}

impl<S> RedactingSseStream<S> {
    pub(super) fn new(inner: S, redactor: SecretRedactor) -> Self {
        Self {
            inner,
            redactor,
            buffer: Vec::new(),
            output_queue: VecDeque::new(),
        }
    }

    fn drain_frames(&mut self) {
        while let Some(frame) = take_one_sse_frame(&mut self.buffer) {
            self.output_queue
                .push_back(Bytes::from(redact_sse_frame(&self.redactor, &frame)));
        }
    }
}

impl<S> Stream for RedactingSseStream<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(bytes) = this.output_queue.pop_front() {
                return Poll::Ready(Some(Ok(bytes)));
            }

            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    this.buffer.extend_from_slice(&bytes);
                    this.drain_frames();
                }
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Some(Err(error))),
                Poll::Ready(None) => {
                    if !this.buffer.is_empty() {
                        let tail = std::mem::take(&mut this.buffer);
                        return Poll::Ready(Some(Ok(Bytes::from(redact_raw_frame(
                            &this.redactor,
                            &tail,
                        )))));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

fn redact_sse_frame(redactor: &SecretRedactor, frame: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(frame);
    let lines = split_sse_lines(&text);
    let redacted_json_data = redacted_sse_json_data(redactor, &lines);
    let mut wrote_redacted_json_data = false;
    let mut out = String::with_capacity(text.len());

    for line in lines {
        match split_sse_field(line.content) {
            SseField::Comment { value } => {
                out.push(':');
                out.push_str(&redactor.redact_text(value));
            }
            SseField::Field { name, value } if name == "data" => {
                if let Some(data) = redacted_json_data.as_deref() {
                    if !wrote_redacted_json_data {
                        out.push_str("data:");
                        if value.starts_with(' ') {
                            out.push(' ');
                        }
                        out.push_str(data);
                        wrote_redacted_json_data = true;
                    } else {
                        continue;
                    }
                } else {
                    out.push_str(name);
                    out.push(':');
                    out.push_str(&redactor.redact_text(value));
                }
            }
            SseField::Field { name, value } => {
                out.push_str(name);
                out.push(':');
                out.push_str(&redactor.redact_text(value));
            }
            SseField::FieldNameOnly => {
                out.push_str(&redactor.redact_text(line.content));
            }
        }
        out.push_str(line.ending);
    }

    if out.as_bytes() == frame {
        frame.to_vec()
    } else {
        out.into_bytes()
    }
}

struct SseLine<'a> {
    content: &'a str,
    ending: &'a str,
}

enum SseField<'a> {
    Comment { value: &'a str },
    Field { name: &'a str, value: &'a str },
    FieldNameOnly,
}

fn split_sse_lines(text: &str) -> Vec<SseLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let Some(relative_newline) = text[start..].find('\n') else {
            lines.push(SseLine {
                content: &text[start..],
                ending: "",
            });
            return lines;
        };
        let newline = start + relative_newline;
        let (content, ending_start) = if newline > start && text.as_bytes()[newline - 1] == b'\r' {
            (&text[start..newline - 1], newline - 1)
        } else {
            (&text[start..newline], newline)
        };
        lines.push(SseLine {
            content,
            ending: &text[ending_start..newline + 1],
        });
        start = newline + 1;
    }
    lines
}

fn split_sse_field(line: &str) -> SseField<'_> {
    if let Some(value) = line.strip_prefix(':') {
        return SseField::Comment { value };
    }
    if let Some(colon) = line.find(':') {
        return SseField::Field {
            name: &line[..colon],
            value: &line[colon + 1..],
        };
    }
    SseField::FieldNameOnly
}

fn redacted_sse_json_data(redactor: &SecretRedactor, lines: &[SseLine<'_>]) -> Option<String> {
    let mut data_lines = Vec::new();
    for line in lines {
        if let SseField::Field {
            name: "data",
            value,
        } = split_sse_field(line.content)
        {
            data_lines.push(value.strip_prefix(' ').unwrap_or(value));
        }
    }

    if data_lines.is_empty() {
        return None;
    }
    let data = data_lines.join("\n");
    if data == "[DONE]" || data.trim().is_empty() {
        return None;
    }
    let event = serde_json::from_str::<Value>(&data).ok()?;
    let redacted = redactor.redact_value(&event);
    if redacted == event {
        return None;
    }
    Some(serde_json::to_string(&redacted).unwrap_or_else(|_| "{}".to_string()))
}

fn redact_raw_frame(redactor: &SecretRedactor, frame: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(frame);
    let redacted = redactor.redact_text(&text);
    if redacted == text.as_ref() {
        frame.to_vec()
    } else {
        redacted.into_bytes()
    }
}

fn take_one_sse_frame(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let end = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
        .or_else(|| {
            buffer
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|position| position + 2)
        })?;
    Some(buffer.drain(..end).collect())
}

pub(super) fn redactor_for_request(
    auth_context: &RequestAuthContext,
    headers: &HeaderMap,
) -> SecretRedactor {
    let mut secrets = request_scoped_credential_secrets(headers);
    if let Some(client_provider_key) = auth_context.client_provider_key() {
        secrets.push(client_provider_key.to_string());
    }
    collect_data_access_secrets(auth_context.access(), &mut secrets);
    collect_runtime_secrets(auth_context.runtime(), &mut secrets);
    SecretRedactor::new(secrets)
}

fn collect_runtime_secrets(runtime: &RuntimeState, secrets: &mut Vec<String>) {
    for namespace in runtime.namespaces.values() {
        for upstream in namespace.upstreams.values() {
            if let Some(provider_key) = upstream.provider_key.as_ref() {
                secrets.push(provider_key.clone());
            }
        }
    }
}

fn collect_data_access_secrets(data_access: &DataAccess, secrets: &mut Vec<String>) {
    if let DataAccess::ProxyKey { key } = data_access {
        secrets.push(key.clone());
    }
}

fn request_scoped_credential_secrets(headers: &HeaderMap) -> Vec<String> {
    let mut secrets = Vec::new();
    for value in headers.get_all(header::AUTHORIZATION) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        if let Some(token) = bearer_token(value) {
            secrets.push(token.to_string());
        }
    }
    for name in [
        "x-api-key",
        "api-key",
        "openai-api-key",
        "anthropic-api-key",
    ] {
        for value in headers.get_all(HeaderName::from_static(name)) {
            let Ok(value) = value.to_str() else {
                continue;
            };
            if !value.trim().is_empty() {
                secrets.push(value.to_string());
            }
        }
    }
    secrets
}

fn bearer_token(value: &str) -> Option<&str> {
    let token = value
        .get(..7)
        .filter(|prefix| prefix.eq_ignore_ascii_case("Bearer "))
        .map(|_| &value[7..])?;
    (!token.trim().is_empty()).then_some(token)
}

fn should_redact_known_secret(secret: &str) -> bool {
    !secret.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redactor_replaces_literal_secrets_longest_first_without_debug_leak() {
        let redactor = SecretRedactor::new([
            "short",
            "",
            "nested-secret",
            "nested-secret-longer",
            "another-secret",
            "nested-secret",
        ]);

        let text = "values: nested-secret-longer, nested-secret, another-secret, short".to_string();
        let redacted = redactor.redact_text(&text);

        assert_eq!(
            redacted,
            "values: [REDACTED], [REDACTED], [REDACTED], [REDACTED]"
        );
        let debug = format!("{redactor:?}");
        assert!(debug.contains("secret_count"));
        assert!(!debug.contains("nested-secret"));
        assert!(!debug.contains("another-secret"));
    }

    #[test]
    fn redactor_replaces_known_short_credentials() {
        let redactor = SecretRedactor::new(["sk-1", "abc123", "pxy"]);

        let redacted =
            redactor.redact_text("short credentials sk-1, abc123, and pxy must not leak");

        assert_eq!(
            redacted,
            "short credentials [REDACTED], [REDACTED], and [REDACTED] must not leak"
        );
    }

    #[test]
    fn redacts_sse_event_type_metadata_without_requiring_data_changes() {
        let redactor = SecretRedactor::new(["event-secret"]);
        let frame = b"event: response.delta.event-secret\ndata: {\"delta\":\"Hello\"}\n\n";

        let redacted = String::from_utf8(redact_sse_frame(&redactor, frame)).expect("utf8 frame");

        assert_eq!(
            redacted,
            "event: response.delta.[REDACTED]\ndata: {\"delta\":\"Hello\"}\n\n"
        );
    }

    #[test]
    fn redacts_sse_id_comment_and_retry_metadata_without_data_changes() {
        let redactor = SecretRedactor::new(["metadata-secret"]);
        let frame = concat!(
            ": comment-metadata-secret\n",
            "id: id-metadata-secret\n",
            "retry: 1000-metadata-secret\n",
            "event: response.delta\n",
            "data: {\"delta\":\"Hello\"}\n\n"
        )
        .as_bytes();

        let redacted = String::from_utf8(redact_sse_frame(&redactor, frame)).expect("utf8 frame");

        assert_eq!(
            redacted,
            concat!(
                ": comment-[REDACTED]\n",
                "id: id-[REDACTED]\n",
                "retry: 1000-[REDACTED]\n",
                "event: response.delta\n",
                "data: {\"delta\":\"Hello\"}\n\n"
            )
        );
    }

    #[test]
    fn redacts_sse_json_data_without_dropping_safe_metadata() {
        let redactor = SecretRedactor::new(["data-secret"]);
        let frame = concat!(
            ": safe comment\n",
            "id: safe-id\n",
            "retry: 2000\n",
            "event: response.delta\n",
            "data: {\"delta\":\"Hello data-secret\"}\n\n"
        )
        .as_bytes();

        let redacted = String::from_utf8(redact_sse_frame(&redactor, frame)).expect("utf8 frame");

        assert_eq!(
            redacted,
            concat!(
                ": safe comment\n",
                "id: safe-id\n",
                "retry: 2000\n",
                "event: response.delta\n",
                "data: {\"delta\":\"Hello [REDACTED]\"}\n\n"
            )
        );
    }
}
