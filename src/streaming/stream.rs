use super::anthropic_source::*;
use super::gemini_source::*;
use super::openai_sink::*;
use super::responses_source::*;
use super::state::*;
use super::wire::*;
use super::*;

pub fn needs_stream_translation(
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
) -> bool {
    upstream_format != client_format
}

pub fn openai_event_as_chunk(event: &Value) -> Option<Value> {
    if event.get("_done").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    if event.get("usage").is_some() {
        return Some(event.clone());
    }
    if event
        .get("choices")
        .and_then(Value::as_array)
        .map(|c| !c.is_empty())
        .unwrap_or(false)
    {
        return Some(event.clone());
    }
    None
}

pub(super) fn reject_openai_multi_choice_for_non_openai_sink(
    state: &mut StreamState,
) -> Vec<Value> {
    reject_openai_stream(
        state,
        "invalid_request_error",
        "unsupported_openai_stream_event",
        "OpenAI streaming response with multiple choices cannot be translated losslessly.",
    )
}

pub(super) fn ensure_single_openai_choice_for_non_openai_sink(
    chunk: &Value,
    state: &mut StreamState,
) -> Result<(), Vec<Value>> {
    if state.message_id.is_none() {
        state.message_id = chunk.get("id").and_then(Value::as_str).map(String::from);
    }
    if state.model.is_none() {
        state.model = chunk.get("model").and_then(Value::as_str).map(String::from);
    }
    if let Some(usage) = chunk.get("usage") {
        state.usage = Some(usage.clone());
    }

    let Some(choices) = chunk.get("choices").and_then(Value::as_array) else {
        return Ok(());
    };
    if choices.is_empty() {
        return Ok(());
    }
    if choices.len() > 1 {
        return Err(reject_openai_multi_choice_for_non_openai_sink(state));
    }

    let choice_index = choices[0].get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
    match state.openai_choice_index {
        Some(previous) if previous != choice_index || choice_index != 0 => {
            Err(reject_openai_multi_choice_for_non_openai_sink(state))
        }
        None if choice_index != 0 => Err(reject_openai_multi_choice_for_non_openai_sink(state)),
        None => {
            state.openai_choice_index = Some(choice_index);
            Ok(())
        }
        Some(_) => Ok(()),
    }
}

pub fn translate_sse_event(
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
    event: &Value,
    state: &mut StreamState,
) -> Vec<Vec<u8>> {
    if upstream_format != client_format && state.fatal_rejection.is_some() {
        return Vec::new();
    }
    if upstream_format == UpstreamFormat::OpenAiCompletion
        && client_format == UpstreamFormat::OpenAiResponses
        && event.get("_done").and_then(Value::as_bool) == Some(true)
    {
        if !state.responses_terminal_sent {
            let response_id = state
                .message_id
                .clone()
                .unwrap_or_else(|| "resp_0".to_string());
            return emit_openai_responses_terminal(state, &response_id, 0, 0);
        }
        return Vec::new();
    }
    if upstream_format == client_format {
        if event.get("_done").and_then(Value::as_bool) == Some(true) {
            return vec![b"data: [DONE]\n\n".to_vec()];
        }
        return vec![format_sse_data(event)];
    }
    if upstream_format == UpstreamFormat::Anthropic
        && event.get("type").and_then(Value::as_str) == Some("error")
    {
        return anthropic_error_event_to_client_sse(event, client_format, state);
    }
    let openai_chunks: Vec<Value> = match upstream_format {
        UpstreamFormat::OpenAiCompletion => openai_event_as_chunk(event).into_iter().collect(),
        UpstreamFormat::Anthropic => claude_event_to_openai_chunks(event, state),
        UpstreamFormat::Google => gemini_event_to_openai_chunks(event, state),
        UpstreamFormat::OpenAiResponses => responses_event_to_openai_chunks(event, state),
    };
    let openai_chunks = if upstream_format == UpstreamFormat::OpenAiCompletion
        && client_format != UpstreamFormat::OpenAiCompletion
    {
        let mut validated = Vec::with_capacity(openai_chunks.len());
        let mut rejection = None;
        for chunk in openai_chunks {
            match ensure_single_openai_choice_for_non_openai_sink(&chunk, state) {
                Ok(()) => validated.push(chunk),
                Err(rejected) => {
                    rejection = Some(rejected);
                    break;
                }
            }
        }
        rejection.unwrap_or(validated)
    } else {
        openai_chunks
    };
    if client_format == UpstreamFormat::OpenAiCompletion {
        return openai_chunks
            .into_iter()
            .map(|c| format_sse_data(&c))
            .collect();
    }
    if client_format == UpstreamFormat::Anthropic {
        let mut out = Vec::new();
        for c in &openai_chunks {
            out.extend(openai_chunk_to_claude_sse(c, state));
        }
        return out;
    }
    if client_format == UpstreamFormat::Google {
        let mut out = Vec::new();
        for c in &openai_chunks {
            out.extend(openai_chunk_to_gemini_sse(c, state));
        }
        return out;
    }
    if client_format == UpstreamFormat::OpenAiResponses {
        let mut out = Vec::new();
        for c in &openai_chunks {
            out.extend(openai_chunk_to_responses_sse(c, state));
        }
        return out;
    }
    openai_chunks
        .into_iter()
        .map(|c| format_sse_data(&c))
        .collect()
}

pub fn translate_response_chunk(
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
    chunk: &[u8],
    state: &mut StreamState,
) -> Result<Vec<Vec<u8>>, String> {
    if upstream_format == client_format {
        return Ok(vec![chunk.to_vec()]);
    }
    let event: Value = serde_json::from_slice(chunk).map_err(|e| e.to_string())?;
    Ok(translate_sse_event(
        upstream_format,
        client_format,
        &event,
        state,
    ))
}

pub(super) fn anthropic_error_event_to_client_sse(
    event: &Value,
    client_format: UpstreamFormat,
    state: &mut StreamState,
) -> Vec<Vec<u8>> {
    let error = event.get("error").unwrap_or(&Value::Null);
    let error_type = error
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("api_error");
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Anthropic streaming error");

    let (normalized_type, normalized_code, finish_reason) =
        normalize_anthropic_stream_error(error_type, message);

    match client_format {
        UpstreamFormat::OpenAiResponses => {
            state.responses_seq += 1;
            let response_id = state
                .message_id
                .clone()
                .unwrap_or_else(|| format!("resp_error_{}", uuid::Uuid::new_v4().simple()));
            let failed = serde_json::json!({
                "type": "response.failed",
                "sequence_number": state.responses_seq,
                "response": {
                    "id": response_id,
                    "object": "response",
                    "created_at": 0,
                    "status": "failed",
                    "background": false,
                    "error": {
                        "type": normalized_type,
                        "code": normalized_code,
                        "message": message
                    },
                    "incomplete_details": null,
                    "usage": null,
                    "metadata": {}
                }
            });
            vec![format_sse_event("response.failed", &failed)]
        }
        UpstreamFormat::OpenAiCompletion => {
            let mut chunk = openai_chunk(state, serde_json::json!({}), Some(finish_reason));
            chunk["error"] = serde_json::json!({
                "type": normalized_type,
                "code": normalized_code,
                "message": message
            });
            vec![format_sse_data(&chunk), b"data: [DONE]\n\n".to_vec()]
        }
        UpstreamFormat::Anthropic => vec![format_sse_data(event)],
        UpstreamFormat::Google => vec![],
    }
}

pub(super) fn normalize_anthropic_stream_error(
    error_type: &str,
    message: &str,
) -> (&'static str, Option<&'static str>, &'static str) {
    let lower_type = error_type.to_ascii_lowercase();
    let lower_message = message.to_ascii_lowercase();
    if lower_type.contains("overloaded") || lower_type.contains("api_error") {
        let code = Some("server_is_overloaded");
        return (
            "server_error",
            code,
            classify_portable_non_success_terminal(code),
        );
    }
    if lower_type.contains("rate_limit") {
        let code = Some("rate_limit_exceeded");
        return (
            "rate_limit_error",
            code,
            classify_portable_non_success_terminal(code),
        );
    }
    if lower_type.contains("invalid_request")
        && (lower_message.contains("context window")
            || lower_message.contains("context_length_exceeded")
            || lower_message.contains("too many tokens")
            || lower_message.contains("maximum context length"))
    {
        return (
            "invalid_request_error",
            Some("context_length_exceeded"),
            "context_length_exceeded",
        );
    }
    if lower_type.contains("invalid_request")
        && (lower_message.contains("refusal") || lower_message.contains("content filter"))
    {
        return (
            "invalid_request_error",
            Some("content_filter"),
            "content_filter",
        );
    }
    let code = Some("server_is_overloaded");
    (
        "server_error",
        code,
        classify_portable_non_success_terminal(code),
    )
}

/// Stream that buffers upstream bytes, parses SSE events, and yields translated SSE bytes.
pub struct TranslateSseStream<S, E> {
    inner: S,
    buffer: Vec<u8>,
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
    state: StreamState,
    output_queue: Vec<Vec<u8>>,
    output_pos: usize,
    close_after_output: bool,
    _error: std::marker::PhantomData<E>,
}

impl<S, E> TranslateSseStream<S, E> {
    pub fn new(inner: S, upstream_format: UpstreamFormat, client_format: UpstreamFormat) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            upstream_format,
            client_format,
            state: StreamState::default(),
            output_queue: Vec::new(),
            output_pos: 0,
            close_after_output: false,
            _error: std::marker::PhantomData,
        }
    }
}

impl<S, E> Stream for TranslateSseStream<S, E>
where
    S: Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: Into<Box<dyn std::error::Error + Send + Sync>> + Unpin,
{
    type Item = Result<bytes::Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if this.output_pos < this.output_queue.len() {
                let next = this.output_queue[this.output_pos].clone();
                this.output_pos += 1;
                if this.output_pos >= this.output_queue.len() {
                    this.output_queue.clear();
                    this.output_pos = 0;
                }
                return Poll::Ready(Some(Ok(bytes::Bytes::from(next))));
            }
            if this.close_after_output {
                return Poll::Ready(None);
            }

            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    this.buffer.extend_from_slice(&chunk);
                    while let Some(event) = take_one_sse_event(&mut this.buffer) {
                        let translated = translate_sse_event(
                            this.upstream_format,
                            this.client_format,
                            &event,
                            &mut this.state,
                        );
                        this.output_queue.extend(translated);
                        if this.upstream_format != this.client_format
                            && this.state.fatal_rejection.is_some()
                        {
                            this.close_after_output = true;
                            break;
                        }
                    }
                    if !this.output_queue.is_empty() {
                        continue;
                    }
                    if this.close_after_output {
                        return Poll::Ready(None);
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(std::io::Error::other(e.into().to_string()))));
                }
                Poll::Ready(None) => {
                    while let Some(event) = take_one_sse_event(&mut this.buffer) {
                        let translated = translate_sse_event(
                            this.upstream_format,
                            this.client_format,
                            &event,
                            &mut this.state,
                        );
                        this.output_queue.extend(translated);
                        if this.upstream_format != this.client_format
                            && this.state.fatal_rejection.is_some()
                        {
                            this.close_after_output = true;
                            break;
                        }
                    }
                    if this.upstream_format == UpstreamFormat::Google {
                        if let Some(chunk) = flush_pending_gemini_finish_chunk(&mut this.state) {
                            let translated = match this.client_format {
                                UpstreamFormat::OpenAiCompletion => vec![format_sse_data(&chunk)],
                                UpstreamFormat::Anthropic => {
                                    openai_chunk_to_claude_sse(&chunk, &mut this.state)
                                }
                                UpstreamFormat::Google => vec![format_sse_data(&chunk)],
                                UpstreamFormat::OpenAiResponses => {
                                    openai_chunk_to_responses_sse(&chunk, &mut this.state)
                                }
                            };
                            this.output_queue.extend(translated);
                        }
                    }
                    if !this.output_queue.is_empty() {
                        continue;
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
