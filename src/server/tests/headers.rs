use super::*;

#[test]
fn extract_forwardable_headers_keeps_only_protocol_headers() {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", HeaderValue::from_static("Bearer test"));
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    headers.insert("OpenAI-Organization", HeaderValue::from_static("org_123"));
    headers.insert("OpenAI-Project", HeaderValue::from_static("proj_123"));
    headers.insert("Idempotency-Key", HeaderValue::from_static("idem_123"));
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("accept-language", HeaderValue::from_static("*"));
    headers.insert("sec-fetch-mode", HeaderValue::from_static("cors"));
    headers.insert("connection", HeaderValue::from_static("keep-alive"));
    headers.insert(
        data_auth::DATA_TOKEN_HEADER,
        HeaderValue::from_static("data-token"),
    );

    let forwarded = extract_forwardable_headers(&headers);
    assert!(forwarded
        .iter()
        .any(|(k, v)| k == "authorization" && v == "Bearer test"));
    assert!(forwarded
        .iter()
        .any(|(k, v)| k == "anthropic-version" && v == "2023-06-01"));
    assert!(forwarded
        .iter()
        .any(|(k, v)| k == "openai-organization" && v == "org_123"));
    assert!(forwarded
        .iter()
        .any(|(k, v)| k == "openai-project" && v == "proj_123"));
    assert!(forwarded
        .iter()
        .any(|(k, v)| k == "idempotency-key" && v == "idem_123"));
    assert!(!forwarded.iter().any(|(k, _)| k == "content-type"));
    assert!(!forwarded.iter().any(|(k, _)| k == "accept-language"));
    assert!(!forwarded.iter().any(|(k, _)| k == "sec-fetch-mode"));
    assert!(!forwarded.iter().any(|(k, _)| k == "connection"));
    assert!(!forwarded
        .iter()
        .any(|(k, _)| k == data_auth::DATA_TOKEN_HEADER));
}
