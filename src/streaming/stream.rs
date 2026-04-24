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

fn validate_openai_stream_tool_call_name(tool_call: &Value) -> Result<(), String> {
    validate_public_selector_visible_identity(tool_call)?;
    for name in [
        tool_call
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str),
        tool_call
            .get("custom")
            .and_then(|custom| custom.get("name"))
            .and_then(Value::as_str),
        tool_call.get("name").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    {
        validate_public_tool_name_not_reserved(name)?;
    }
    Ok(())
}

fn validate_openai_stream_choice_tool_names(choice: &Value) -> Result<(), String> {
    for container in [choice.get("delta"), choice.get("message")]
        .into_iter()
        .flatten()
    {
        if let Some(tool_calls) = container.get("tool_calls").and_then(Value::as_array) {
            for tool_call in tool_calls {
                validate_openai_stream_tool_call_name(tool_call)?;
            }
        }
        if let Some(function_call) = container.get("function_call") {
            validate_openai_stream_tool_call_name(function_call)?;
        }
    }
    Ok(())
}

fn validate_openai_stream_event_tool_names(event: &Value) -> Result<(), String> {
    if let Some(choices) = event.get("choices").and_then(Value::as_array) {
        for choice in choices {
            validate_openai_stream_choice_tool_names(choice)?;
        }
    }
    Ok(())
}

fn validate_anthropic_stream_content_tool_names(content: &Value) -> Result<(), String> {
    let Some(blocks) = content.as_array() else {
        return Ok(());
    };
    for block in blocks {
        if matches!(
            block.get("type").and_then(Value::as_str),
            Some("tool_use" | "server_tool_use")
        ) {
            if let Some(name) = block.get("name").and_then(Value::as_str) {
                validate_public_tool_name_not_reserved(name)?;
            }
        }
    }
    Ok(())
}

fn validate_anthropic_stream_event_tool_names(event: &Value) -> Result<(), String> {
    if matches!(
        event
            .get("content_block")
            .and_then(|block| block.get("type"))
            .and_then(Value::as_str),
        Some("tool_use" | "server_tool_use")
    ) {
        if let Some(name) = event
            .get("content_block")
            .and_then(|block| block.get("name"))
            .and_then(Value::as_str)
        {
            validate_public_tool_name_not_reserved(name)?;
        }
    }
    if let Some(content) = event.get("content") {
        validate_anthropic_stream_content_tool_names(content)?;
    }
    if let Some(message_content) = event
        .get("message")
        .and_then(|message| message.get("content"))
    {
        validate_anthropic_stream_content_tool_names(message_content)?;
    }
    Ok(())
}

fn validate_gemini_stream_content_tool_names(content: &Value) -> Result<(), String> {
    let Some(parts) = content.get("parts").and_then(Value::as_array) else {
        return Ok(());
    };
    for part in parts {
        for tool_part in [
            part.get("functionCall")
                .or_else(|| part.get("function_call")),
            part.get("functionResponse")
                .or_else(|| part.get("function_response")),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(name) = tool_part.get("name").and_then(Value::as_str) {
                validate_public_tool_name_not_reserved(name)?;
            }
        }
    }
    Ok(())
}

fn validate_gemini_stream_response_tool_names(body: &Value) -> Result<(), String> {
    if let Some(candidates) = body.get("candidates").and_then(Value::as_array) {
        for candidate in candidates {
            if let Some(content) = candidate.get("content") {
                validate_gemini_stream_content_tool_names(content)?;
            }
        }
    }
    Ok(())
}

fn validate_gemini_stream_event_tool_names(event: &Value) -> Result<(), String> {
    validate_gemini_stream_response_tool_names(event)?;
    if let Some(response) = event.get("response") {
        validate_gemini_stream_response_tool_names(response)?;
    }
    Ok(())
}

fn validate_responses_stream_item_tool_name(item: &Value) -> Result<(), String> {
    validate_responses_public_tool_call_item_identity(item)
}

fn validate_responses_stream_output_tool_names(output: &Value) -> Result<(), String> {
    validate_responses_public_output_tool_identity(output)
}

fn validate_responses_stream_event_tool_names(event: &Value) -> Result<(), String> {
    validate_responses_public_stream_event_tool_identity(event)?;
    if let Some(item) = event.get("item") {
        validate_responses_stream_item_tool_name(item)?;
    }
    if let Some(output) = event.get("output") {
        validate_responses_stream_output_tool_names(output)?;
    }
    if let Some(response) = event.get("response") {
        validate_responses_public_response_object_tool_identity(response)?;
    }
    Ok(())
}

fn validate_public_stream_response_event_tool_names(
    format: UpstreamFormat,
    event: &Value,
) -> Result<(), String> {
    if contains_internal_context_field(event) {
        return Err(INTERNAL_ARTIFACT_ERROR_MESSAGE.to_string());
    }
    match format {
        UpstreamFormat::OpenAiCompletion => validate_openai_stream_event_tool_names(event),
        UpstreamFormat::OpenAiResponses => validate_responses_stream_event_tool_names(event),
        UpstreamFormat::Anthropic => validate_anthropic_stream_event_tool_names(event),
        UpstreamFormat::Google => validate_gemini_stream_event_tool_names(event),
    }
}

fn validate_openai_chunks_for_public_stream(
    chunks: &[Value],
    state: &mut StreamState,
) -> Result<(), Vec<Value>> {
    for chunk in chunks {
        if let Err(message) = validate_openai_stream_event_tool_names(chunk) {
            return Err(reject_openai_stream(
                state,
                "invalid_request_error",
                "reserved_openai_custom_bridge_prefix",
                message,
            ));
        }
    }
    Ok(())
}

fn openai_chunks_to_client_sse(
    client_format: UpstreamFormat,
    openai_chunks: Vec<Value>,
    state: &mut StreamState,
) -> Vec<Vec<u8>> {
    match client_format {
        UpstreamFormat::OpenAiCompletion => openai_chunks
            .into_iter()
            .map(|c| format_sse_data(&c))
            .collect(),
        UpstreamFormat::Anthropic => {
            let mut out = Vec::new();
            for c in &openai_chunks {
                out.extend(openai_chunk_to_claude_sse(c, state));
            }
            out
        }
        UpstreamFormat::Google => {
            let mut out = Vec::new();
            for c in &openai_chunks {
                out.extend(openai_chunk_to_gemini_sse(c, state));
            }
            out
        }
        UpstreamFormat::OpenAiResponses => {
            let mut out = Vec::new();
            for c in &openai_chunks {
                out.extend(openai_chunk_to_responses_sse(c, state));
            }
            out
        }
    }
}

fn reject_public_stream_tool_name(
    client_format: UpstreamFormat,
    state: &mut StreamState,
    message: String,
) -> Vec<Vec<u8>> {
    let chunks = reject_openai_stream(
        state,
        "invalid_request_error",
        "reserved_openai_custom_bridge_prefix",
        message,
    );
    openai_chunks_to_client_sse(client_format, chunks, state)
}

pub fn translate_sse_event(
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
    event: &Value,
    state: &mut StreamState,
) -> Vec<Vec<u8>> {
    if state.fatal_rejection.is_some() {
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
            let mut out = Vec::new();
            flush_pending_responses_tool_calls(state, &response_id, true, &mut out);
            out.extend(emit_openai_responses_terminal(state, &response_id, 0, 0));
            return out;
        }
        return Vec::new();
    }
    if upstream_format == UpstreamFormat::Anthropic
        && event.get("type").and_then(Value::as_str) == Some("error")
    {
        return anthropic_error_event_to_client_sse(event, client_format, state);
    }
    if upstream_format == client_format {
        if event.get("_done").and_then(Value::as_bool) == Some(true) {
            return vec![b"data: [DONE]\n\n".to_vec()];
        }
        let mut public_event = event.clone();
        sanitize_public_stream_error_event(&mut public_event);
        if let Err(message) =
            validate_public_stream_response_event_tool_names(client_format, &public_event)
        {
            return reject_public_stream_tool_name(client_format, state, message);
        }
        return vec![format_sse_data(&public_event)];
    }
    if event.get("_done").and_then(Value::as_bool) != Some(true) {
        if let Err(message) =
            validate_public_stream_response_event_tool_names(upstream_format, event)
        {
            return reject_public_stream_tool_name(client_format, state, message);
        }
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
    let openai_chunks = match validate_openai_chunks_for_public_stream(&openai_chunks, state) {
        Ok(()) => openai_chunks,
        Err(rejected) => rejected,
    };
    openai_chunks_to_client_sse(client_format, openai_chunks, state)
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

fn stream_event_is_public_error(event: &Value) -> bool {
    event.get("error").is_some()
        || matches!(
            event.get("type").and_then(Value::as_str),
            Some("error" | "response.failed")
        )
}

fn sanitize_internal_artifact_strings(value: &mut Value) {
    match value {
        Value::String(text) => {
            *text = sanitize_public_error_message(text);
        }
        Value::Array(items) => {
            for item in items {
                sanitize_internal_artifact_strings(item);
            }
        }
        Value::Object(object) => {
            let keys = object.keys().cloned().collect::<Vec<_>>();
            for key in keys {
                if contains_internal_artifact_text(&key) {
                    object.remove(&key);
                    continue;
                }
                if let Some(value) = object.get_mut(&key) {
                    sanitize_internal_artifact_strings(value);
                }
            }
        }
        _ => {}
    }
}

fn sanitize_public_stream_error_event(event: &mut Value) {
    if stream_event_is_public_error(event) {
        sanitize_internal_artifact_strings(event);
    }
}

fn sse_frame_contains_internal_artifact(frame: &[u8]) -> bool {
    contains_internal_artifact_text(String::from_utf8_lossy(frame).as_ref())
}

fn canonical_sse_frame(event_type: Option<&str>, event: &Value) -> Vec<u8> {
    if event.get("_done").and_then(Value::as_bool) == Some(true) {
        return b"data: [DONE]\n\n".to_vec();
    }
    match event_type {
        Some(event_type) => format_sse_event(event_type, event),
        None => format_sse_data(event),
    }
}

/// Same-format SSE passthrough that validates public tool names before releasing each frame.
pub struct GuardedSseStream<S, E> {
    inner: S,
    buffer: Vec<u8>,
    client_format: UpstreamFormat,
    state: StreamState,
    output_queue: Vec<Vec<u8>>,
    output_pos: usize,
    close_after_output: bool,
    _error: std::marker::PhantomData<E>,
}

impl<S, E> GuardedSseStream<S, E> {
    pub fn new(inner: S, client_format: UpstreamFormat) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            client_format,
            state: StreamState::default(),
            output_queue: Vec::new(),
            output_pos: 0,
            close_after_output: false,
            _error: std::marker::PhantomData,
        }
    }

    fn drain_validated_frames(&mut self) {
        while let Some((frame, event)) = take_one_sse_frame(&mut self.buffer) {
            let raw_has_internal_artifact = sse_frame_contains_internal_artifact(&frame);
            let event_type = sse_frame_event_type(&frame);
            if event_type
                .as_deref()
                .is_some_and(contains_internal_artifact_text)
            {
                self.output_queue.extend(reject_public_stream_tool_name(
                    self.client_format,
                    &mut self.state,
                    INTERNAL_ARTIFACT_ERROR_MESSAGE.to_string(),
                ));
                self.close_after_output = true;
                break;
            }

            let Some(mut event) = event else {
                if raw_has_internal_artifact {
                    self.output_queue.extend(reject_public_stream_tool_name(
                        self.client_format,
                        &mut self.state,
                        INTERNAL_ARTIFACT_ERROR_MESSAGE.to_string(),
                    ));
                    self.close_after_output = true;
                    break;
                }
                self.output_queue.push(frame);
                continue;
            };

            if event.get("_done").and_then(Value::as_bool) == Some(true)
                && !raw_has_internal_artifact
            {
                self.output_queue.push(frame);
                continue;
            }

            sanitize_public_stream_error_event(&mut event);
            if event.get("_done").and_then(Value::as_bool) != Some(true) {
                if let Err(message) =
                    validate_public_stream_response_event_tool_names(self.client_format, &event)
                {
                    self.output_queue.extend(reject_public_stream_tool_name(
                        self.client_format,
                        &mut self.state,
                        message,
                    ));
                    self.close_after_output = true;
                    break;
                }
            }
            self.output_queue
                .push(canonical_sse_frame(event_type.as_deref(), &event));
        }
    }
}

impl<S, E> Stream for GuardedSseStream<S, E>
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
                    this.drain_validated_frames();
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
                    this.drain_validated_frames();
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
    let error_type = if contains_internal_artifact_text(error_type) {
        "api_error"
    } else {
        error_type
    };
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Anthropic streaming error");
    let message = sanitize_public_error_message(message);

    let (normalized_type, normalized_code, finish_reason) =
        normalize_anthropic_stream_error(error_type, &message);

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
        UpstreamFormat::Anthropic => {
            let sanitized = serde_json::json!({
                "type": "error",
                "error": {
                    "type": error_type,
                    "message": message
                }
            });
            vec![format_sse_event("error", &sanitized)]
        }
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

    pub fn with_request_scoped_tool_bridge_context(
        mut self,
        bridge_context: Option<Value>,
    ) -> Self {
        self.state.request_scoped_tool_bridge_context = bridge_context;
        self
    }

    fn reject_internal_artifact_frame(&mut self) {
        self.output_queue.extend(reject_public_stream_tool_name(
            self.client_format,
            &mut self.state,
            INTERNAL_ARTIFACT_ERROR_MESSAGE.to_string(),
        ));
        self.close_after_output = true;
    }

    fn drain_translated_frames(&mut self) {
        while let Some((frame, event)) = take_one_sse_frame(&mut self.buffer) {
            let event_type = sse_frame_event_type(&frame);
            if event_type
                .as_deref()
                .is_some_and(contains_internal_artifact_text)
            {
                self.reject_internal_artifact_frame();
                break;
            }

            let Some(event) = event else {
                if sse_frame_contains_internal_artifact(&frame) {
                    self.reject_internal_artifact_frame();
                    break;
                }
                continue;
            };

            let translated = translate_sse_event(
                self.upstream_format,
                self.client_format,
                &event,
                &mut self.state,
            );
            self.output_queue.extend(translated);
            if self.state.fatal_rejection.is_some() {
                self.close_after_output = true;
                break;
            }
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
                    this.drain_translated_frames();
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
                    this.drain_translated_frames();
                    if !this.close_after_output && this.upstream_format == UpstreamFormat::Google {
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
