use super::*;

#[test]
fn take_one_sse_event_parses_data_line() {
    let mut buf = b"data: {\"type\":\"message_start\"}\n\n".to_vec();
    let event = take_one_sse_event(&mut buf);
    assert!(event.is_some());
    assert_eq!(
        event.as_ref().unwrap().get("type").and_then(Value::as_str),
        Some("message_start")
    );
    assert!(buf.is_empty());
}

#[test]
fn take_one_sse_event_skips_event_line() {
    let mut buf = b"event: message_start\ndata: {\"type\":\"message_start\"}\n\n".to_vec();
    let event = take_one_sse_event(&mut buf);
    assert!(event.is_some());
    assert_eq!(
        event.as_ref().unwrap().get("type").and_then(Value::as_str),
        Some("message_start")
    );
}

#[test]
fn take_one_sse_event_handles_crlf_separators() {
    // Some upstream servers (e.g., vLLM/uvicorn) use \r\n\r\n as SSE separator
    let mut buf =
        b"data: {\"id\":\"chat123\",\"choices\":[{\"delta\":{\"content\":\"OK\"}}]}\r\n\r\n"
            .to_vec();
    let event = take_one_sse_event(&mut buf);
    assert!(event.is_some());
    assert_eq!(
        event.as_ref().unwrap().get("id").and_then(Value::as_str),
        Some("chat123")
    );
    assert!(buf.is_empty());
}

#[test]
fn take_one_sse_event_handles_crlf_done_marker() {
    let mut buf = b"data: [DONE]\r\n\r\n".to_vec();
    let event = take_one_sse_event(&mut buf);
    assert!(event.is_some());
    assert_eq!(
        event.as_ref().unwrap().get("_done"),
        Some(&serde_json::json!(true))
    );
}

#[test]
fn take_one_sse_event_handles_mixed_crlf_and_lf() {
    // Buffer with one CRLF event followed by one LF event
    let mut buf = b"data: {\"first\":true}\r\n\r\ndata: {\"second\":true}\n\n".to_vec();
    let e1 = take_one_sse_event(&mut buf);
    assert!(e1.is_some());
    assert_eq!(
        e1.as_ref().unwrap().get("first"),
        Some(&serde_json::json!(true))
    );
    let e2 = take_one_sse_event(&mut buf);
    assert!(e2.is_some());
    assert_eq!(
        e2.as_ref().unwrap().get("second"),
        Some(&serde_json::json!(true))
    );
    assert!(buf.is_empty());
}

#[test]
fn take_one_sse_event_skips_blank_data_frames_without_treating_them_as_done() {
    let mut buf = b"data:\n\ndata: {\"ok\":true}\n\n".to_vec();
    let event = take_one_sse_event(&mut buf);
    assert_eq!(event, Some(serde_json::json!({ "ok": true })));
    assert!(buf.is_empty());
}

#[test]
fn take_one_sse_event_joins_multiline_data_payload() {
    let mut buf = b"data: {\"outer\":\ndata: {\"inner\":1}}\n\n".to_vec();
    let event = take_one_sse_event(&mut buf);
    assert_eq!(event, Some(serde_json::json!({ "outer": { "inner": 1 } })));
    assert!(buf.is_empty());
}

#[test]
fn test_format_sse_data() {
    let v = serde_json::json!({ "x": 1 });
    let bytes = format_sse_data(&v);
    assert!(bytes.starts_with(b"data: "));
    assert!(bytes.ends_with(b"\n\n"));
}

#[test]
fn format_sse_event_includes_event_type() {
    let v = serde_json::json!({ "type": "message_start" });
    let bytes = format_sse_event("message_start", &v);
    assert!(bytes.starts_with(b"event: message_start\n"));
    assert!(bytes.windows(6).any(|w| w == b"data: "));
    assert!(bytes.ends_with(b"\n\n"));
}
