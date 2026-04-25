//! Source-aware multimodal e2e contracts for requests targeting Anthropic.

mod common;

use common::mock_upstream::{
    spawn_asserting_anthropic_mock, CapturedMockRequest, CapturedMockRequests,
};
use common::proxy_helpers::proxy_config;
use common::runtime_proxy::start_proxy;
use llm_universal_proxy::formats::UpstreamFormat;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

const AUDIO_WAV_B64: &str = "UklGRiQAAABXQVZFZm10IBAAAAABAAEAESsAACJWAAACABAAZGF0YQAAAAA=";
const PDF_B64: &str = "JVBERi0x";
const PDF_DATA_URI: &str = "data:application/pdf;base64,JVBERi0x";
const POLLUTED_REMOTE_IMAGE_URL: &str = "https://example.test/assets/cat.png\nfile:///tmp/cat.png";
const CONTROL_POLLUTED_REMOTE_IMAGE_URL: &str = "https://example.test/assets/\u{0007}cat.png";
const POLLUTED_REMOTE_PDF_URL: &str =
    "https://example.test/papers/policy.pdf\nfile:///tmp/policy.pdf";
const CONTROL_POLLUTED_REMOTE_PDF_URL: &str = "https://example.test/papers/\u{0007}policy.pdf";
const REMOTE_IMAGE_URL: &str = "https://example.test/assets/cat.png";
const REMOTE_PDF_URL: &str = "https://example.test/papers/policy.pdf";
const TEXT_DATA_URI: &str = "data:text/plain;base64,SGVsbG8=";
const VIDEO_DATA_URI: &str = "data:video/mp4;base64,AAAA";

#[tokio::test]
async fn openai_chat_remote_image_url_to_anthropic_uses_url_source() {
    let (mock_base, _mock, captured) =
        spawn_asserting_anthropic_mock(assert_anthropic_remote_image_url_source).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3-5-sonnet",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Describe this remote image" },
                    { "type": "image_url", "image_url": { "url": REMOTE_IMAGE_URL } }
                ]
            }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_success_response(response).await;
    assert_upstream_called_once(&captured).await;
}

#[tokio::test]
async fn openai_responses_remote_image_url_to_anthropic_uses_url_source() {
    let (mock_base, _mock, captured) =
        spawn_asserting_anthropic_mock(assert_anthropic_remote_image_url_source).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3-5-sonnet",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "Describe this remote image" },
                    { "type": "input_image", "image_url": REMOTE_IMAGE_URL }
                ]
            }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_success_response(response).await;
    assert_upstream_called_once(&captured).await;
}

#[tokio::test]
async fn openai_polluted_http_image_urls_to_anthropic_fail_closed_before_upstream() {
    for (label, path, body) in [
        (
            "chat image_url with embedded newline",
            "/openai/v1/chat/completions",
            json!({
                "model": "claude-3-5-sonnet",
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Describe this remote image" },
                        { "type": "image_url", "image_url": { "url": POLLUTED_REMOTE_IMAGE_URL } }
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "responses image_url with embedded control",
            "/openai/v1/responses",
            json!({
                "model": "claude-3-5-sonnet",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Describe this remote image" },
                        { "type": "input_image", "image_url": CONTROL_POLLUTED_REMOTE_IMAGE_URL }
                    ]
                }],
                "stream": false
            }),
        ),
    ] {
        let (mock_base, _mock, captured) = spawn_asserting_anthropic_mock(move |_| {
            Err(format!("{label} reached Anthropic upstream"))
        })
        .await;
        let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
        let (proxy_base, _proxy) = start_proxy(config).await;

        let response = Client::new()
            .post(format!("{proxy_base}{path}"))
            .json(&body)
            .send()
            .await
            .unwrap();

        assert_failure_response(response).await;
        assert_no_upstream_request(&captured).await;
    }
}

#[tokio::test]
async fn openai_non_http_image_urls_to_anthropic_fail_closed_before_upstream() {
    for (label, path, body) in [
        (
            "chat gs image_url",
            "/openai/v1/chat/completions",
            json!({
                "model": "claude-3-5-sonnet",
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Describe this remote image" },
                        { "type": "image_url", "image_url": { "url": "gs://bucket/cat.png" } }
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "chat file image_url",
            "/openai/v1/chat/completions",
            json!({
                "model": "claude-3-5-sonnet",
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Describe this remote image" },
                        { "type": "image_url", "image_url": { "url": "file:///tmp/cat.png" } }
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "chat s3 image_url",
            "/openai/v1/chat/completions",
            json!({
                "model": "claude-3-5-sonnet",
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Describe this remote image" },
                        { "type": "image_url", "image_url": { "url": "s3://bucket/cat.png" } }
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "responses gs image_url",
            "/openai/v1/responses",
            json!({
                "model": "claude-3-5-sonnet",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Describe this remote image" },
                        { "type": "input_image", "image_url": "gs://bucket/cat.png" }
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "responses file image_url",
            "/openai/v1/responses",
            json!({
                "model": "claude-3-5-sonnet",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Describe this remote image" },
                        { "type": "input_image", "image_url": "file:///tmp/cat.png" }
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "responses s3 image_url",
            "/openai/v1/responses",
            json!({
                "model": "claude-3-5-sonnet",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Describe this remote image" },
                        { "type": "input_image", "image_url": "s3://bucket/cat.png" }
                    ]
                }],
                "stream": false
            }),
        ),
    ] {
        let (mock_base, _mock, captured) = spawn_asserting_anthropic_mock(move |_| {
            Err(format!("{label} reached Anthropic upstream"))
        })
        .await;
        let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
        let (proxy_base, _proxy) = start_proxy(config).await;

        let response = Client::new()
            .post(format!("{proxy_base}{path}"))
            .json(&body)
            .send()
            .await
            .unwrap();

        assert_failure_response(response).await;
        assert_no_upstream_request(&captured).await;
    }
}

#[tokio::test]
async fn openai_chat_pdf_data_uri_to_anthropic_document_base64() {
    let (mock_base, _mock, captured) =
        spawn_asserting_anthropic_mock(assert_anthropic_pdf_base64_document).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3-5-sonnet",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Summarize this PDF" },
                    { "type": "file", "file": {
                        "file_data": PDF_DATA_URI,
                        "filename": "policy.pdf",
                        "mime_type": "application/pdf"
                    }}
                ]
            }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_success_response(response).await;
    assert_upstream_called_once(&captured).await;
}

#[tokio::test]
async fn openai_responses_pdf_data_uri_to_anthropic_document_base64() {
    let (mock_base, _mock, captured) =
        spawn_asserting_anthropic_mock(assert_anthropic_pdf_base64_document).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3-5-sonnet",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "Summarize this PDF" },
                    { "type": "input_file",
                      "file_data": PDF_DATA_URI,
                      "filename": "policy.pdf",
                      "mime_type": "application/pdf" }
                ]
            }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_success_response(response).await;
    assert_upstream_called_once(&captured).await;
}

#[tokio::test]
async fn openai_chat_pdf_url_to_anthropic_document_url_with_pdf_provenance() {
    let (mock_base, _mock, captured) =
        spawn_asserting_anthropic_mock(assert_anthropic_pdf_url_document).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3-5-sonnet",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Summarize this PDF" },
                    { "type": "file", "file": {
                        "file_data": REMOTE_PDF_URL,
                        "filename": "policy.pdf",
                        "mime_type": "application/pdf"
                    }}
                ]
            }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_success_response(response).await;
    assert_upstream_called_once(&captured).await;
}

#[tokio::test]
async fn openai_responses_pdf_file_url_to_anthropic_document_url_with_pdf_provenance() {
    let (mock_base, _mock, captured) =
        spawn_asserting_anthropic_mock(assert_anthropic_pdf_url_document).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3-5-sonnet",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "Summarize this PDF" },
                    { "type": "input_file",
                      "file_url": REMOTE_PDF_URL,
                      "filename": "policy.pdf",
                      "mime_type": "application/pdf" }
                ]
            }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_success_response(response).await;
    assert_upstream_called_once(&captured).await;
}

#[tokio::test]
async fn openai_polluted_http_pdf_urls_to_anthropic_fail_closed_before_upstream() {
    for (label, path, body) in [
        (
            "chat pdf file_data with embedded newline",
            "/openai/v1/chat/completions",
            json!({
                "model": "claude-3-5-sonnet",
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Summarize this PDF" },
                        { "type": "file", "file": {
                            "file_data": POLLUTED_REMOTE_PDF_URL,
                            "filename": "policy.pdf",
                            "mime_type": "application/pdf"
                        }}
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "responses pdf file_url with embedded control",
            "/openai/v1/responses",
            json!({
                "model": "claude-3-5-sonnet",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Summarize this PDF" },
                        { "type": "input_file",
                          "file_url": CONTROL_POLLUTED_REMOTE_PDF_URL,
                          "filename": "policy.pdf",
                          "mime_type": "application/pdf" }
                    ]
                }],
                "stream": false
            }),
        ),
    ] {
        let (mock_base, _mock, captured) = spawn_asserting_anthropic_mock(move |_| {
            Err(format!("{label} reached Anthropic upstream"))
        })
        .await;
        let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
        let (proxy_base, _proxy) = start_proxy(config).await;

        let response = Client::new()
            .post(format!("{proxy_base}{path}"))
            .json(&body)
            .send()
            .await
            .unwrap();

        assert_failure_response(response).await;
        assert_no_upstream_request(&captured).await;
    }
}

#[tokio::test]
async fn openai_non_http_pdf_urls_to_anthropic_fail_closed_before_upstream() {
    for (label, path, body) in [
        (
            "chat file pdf file_data",
            "/openai/v1/chat/completions",
            json!({
                "model": "claude-3-5-sonnet",
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Summarize this PDF" },
                        { "type": "file", "file": {
                            "file_data": "file:///tmp/policy.pdf",
                            "filename": "policy.pdf",
                            "mime_type": "application/pdf"
                        }}
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "responses gs pdf file_url",
            "/openai/v1/responses",
            json!({
                "model": "claude-3-5-sonnet",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Summarize this PDF" },
                        { "type": "input_file",
                          "file_url": "gs://bucket/policy.pdf",
                          "filename": "policy.pdf",
                          "mime_type": "application/pdf" }
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "responses s3 pdf file_url",
            "/openai/v1/responses",
            json!({
                "model": "claude-3-5-sonnet",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Summarize this PDF" },
                        { "type": "input_file",
                          "file_url": "s3://bucket/policy.pdf",
                          "filename": "policy.pdf",
                          "mime_type": "application/pdf" }
                    ]
                }],
                "stream": false
            }),
        ),
    ] {
        let (mock_base, _mock, captured) = spawn_asserting_anthropic_mock(move |_| {
            Err(format!("{label} reached Anthropic upstream"))
        })
        .await;
        let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
        let (proxy_base, _proxy) = start_proxy(config).await;

        let response = Client::new()
            .post(format!("{proxy_base}{path}"))
            .json(&body)
            .send()
            .await
            .unwrap();

        assert_failure_response(response).await;
        assert_no_upstream_request(&captured).await;
    }
}

#[tokio::test]
async fn openai_file_data_and_file_url_to_anthropic_fail_closed_before_upstream() {
    for (label, path, body) in [
        (
            "chat file with both nested sources",
            "/openai/v1/chat/completions",
            json!({
                "model": "claude-3-5-sonnet",
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Summarize this PDF" },
                        { "type": "file", "file": {
                            "file_data": PDF_DATA_URI,
                            "file_url": REMOTE_PDF_URL,
                            "filename": "policy.pdf",
                            "mime_type": "application/pdf"
                        }}
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "chat file with top-level and nested source conflict",
            "/openai/v1/chat/completions",
            json!({
                "model": "claude-3-5-sonnet",
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Summarize this PDF" },
                        { "type": "file",
                          "file_url": REMOTE_PDF_URL,
                          "file": {
                            "file_data": PDF_DATA_URI,
                            "filename": "policy.pdf",
                            "mime_type": "application/pdf"
                        }}
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "responses input_file with both sources",
            "/openai/v1/responses",
            json!({
                "model": "claude-3-5-sonnet",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Summarize this PDF" },
                        { "type": "input_file",
                          "file_data": PDF_DATA_URI,
                          "file_url": REMOTE_PDF_URL,
                          "filename": "policy.pdf",
                          "mime_type": "application/pdf" }
                    ]
                }],
                "stream": false
            }),
        ),
    ] {
        let (mock_base, _mock, captured) = spawn_asserting_anthropic_mock(move |_| {
            Err(format!("{label} reached Anthropic upstream"))
        })
        .await;
        let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
        let (proxy_base, _proxy) = start_proxy(config).await;

        let response = Client::new()
            .post(format!("{proxy_base}{path}"))
            .json(&body)
            .send()
            .await
            .unwrap();

        assert_failure_response(response).await;
        assert_no_upstream_request(&captured).await;
    }
}

#[tokio::test]
async fn openai_file_id_with_file_data_to_anthropic_fails_closed_before_upstream() {
    for (label, path, body) in [
        (
            "chat file_id with file_data",
            "/openai/v1/chat/completions",
            json!({
                "model": "claude-3-5-sonnet",
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Summarize this PDF" },
                        { "type": "file", "file": {
                            "file_id": "file_123",
                            "file_data": PDF_DATA_URI,
                            "filename": "policy.pdf",
                            "mime_type": "application/pdf"
                        }}
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "responses file_id with file_data",
            "/openai/v1/responses",
            json!({
                "model": "claude-3-5-sonnet",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Summarize this PDF" },
                        { "type": "input_file",
                          "file_id": "file_123",
                          "file_data": PDF_DATA_URI,
                          "filename": "policy.pdf",
                          "mime_type": "application/pdf" }
                    ]
                }],
                "stream": false
            }),
        ),
    ] {
        let (mock_base, _mock, captured) = spawn_asserting_anthropic_mock(move |_| {
            Err(format!("{label} reached Anthropic upstream"))
        })
        .await;
        let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
        let (proxy_base, _proxy) = start_proxy(config).await;

        let response = Client::new()
            .post(format!("{proxy_base}{path}"))
            .json(&body)
            .send()
            .await
            .unwrap();

        assert_failure_response(response).await;
        assert_no_upstream_request(&captured).await;
    }
}

#[tokio::test]
async fn openai_non_string_file_id_with_file_data_to_anthropic_fails_closed_before_upstream() {
    for (label, path, body) in [
        (
            "chat object file_id with file_data",
            "/openai/v1/chat/completions",
            json!({
                "model": "claude-3-5-sonnet",
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Summarize this PDF" },
                        { "type": "file", "file": {
                            "file_id": { "id": "file_123" },
                            "file_data": PDF_DATA_URI,
                            "filename": "policy.pdf",
                            "mime_type": "application/pdf"
                        }}
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "responses numeric file_id with file_data",
            "/openai/v1/responses",
            json!({
                "model": "claude-3-5-sonnet",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Summarize this PDF" },
                        { "type": "input_file",
                          "file_id": 123,
                          "file_data": PDF_DATA_URI,
                          "filename": "policy.pdf",
                          "mime_type": "application/pdf" }
                    ]
                }],
                "stream": false
            }),
        ),
    ] {
        let (mock_base, _mock, captured) = spawn_asserting_anthropic_mock(move |_| {
            Err(format!("{label} reached Anthropic upstream"))
        })
        .await;
        let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
        let (proxy_base, _proxy) = start_proxy(config).await;

        let response = Client::new()
            .post(format!("{proxy_base}{path}"))
            .json(&body)
            .send()
            .await
            .unwrap();

        assert_failure_response(response).await;
        assert_no_upstream_request(&captured).await;
    }
}

#[tokio::test]
async fn openai_non_pdf_file_audio_and_video_to_anthropic_fail_closed_before_upstream() {
    for (label, path, body) in [
        (
            "chat audio",
            "/openai/v1/chat/completions",
            json!({
                "model": "claude-3-5-sonnet",
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Transcribe this audio" },
                        { "type": "input_audio", "input_audio": { "data": AUDIO_WAV_B64, "format": "wav" } }
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "responses audio",
            "/openai/v1/responses",
            json!({
                "model": "claude-3-5-sonnet",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Transcribe this audio" },
                        { "type": "input_audio", "input_audio": { "data": AUDIO_WAV_B64, "format": "wav" } }
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "chat non-PDF file",
            "/openai/v1/chat/completions",
            json!({
                "model": "claude-3-5-sonnet",
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Read this text file" },
                        { "type": "file", "file": {
                            "file_data": TEXT_DATA_URI,
                            "filename": "notes.txt",
                            "mime_type": "text/plain"
                        }}
                    ]
                }],
                "stream": false
            }),
        ),
        (
            "responses video",
            "/openai/v1/responses",
            json!({
                "model": "claude-3-5-sonnet",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Describe this video" },
                        { "type": "input_file",
                          "file_data": VIDEO_DATA_URI,
                          "filename": "clip.mp4",
                          "mime_type": "video/mp4" }
                    ]
                }],
                "stream": false
            }),
        ),
    ] {
        let (mock_base, _mock, captured) = spawn_asserting_anthropic_mock(move |_| {
            Err(format!("{label} reached Anthropic upstream"))
        })
        .await;
        let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
        let (proxy_base, _proxy) = start_proxy(config).await;

        let response = Client::new()
            .post(format!("{proxy_base}{path}"))
            .json(&body)
            .send()
            .await
            .unwrap();

        assert_failure_response(response).await;
        assert_no_upstream_request(&captured).await;
    }
}

#[tokio::test]
async fn gemini_pdf_inline_and_file_data_to_anthropic_documents() {
    let (mock_base, _mock, captured) =
        spawn_asserting_anthropic_mock(assert_anthropic_gemini_pdf_documents).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!(
            "{proxy_base}/google/v1beta/models/claude-3-5-sonnet:generateContent"
        ))
        .json(&json!({
            "model": "claude-3-5-sonnet",
            "contents": [{
                "role": "user",
                "parts": [
                    { "text": "Summarize these PDFs" },
                    { "inlineData": { "mimeType": "application/pdf", "data": PDF_B64 } },
                    { "fileData": {
                        "mimeType": "application/pdf",
                        "fileUri": REMOTE_PDF_URL,
                        "displayName": "policy.pdf"
                    }}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();

    assert_success_response(response).await;
    assert_upstream_called_once(&captured).await;
}

#[tokio::test]
async fn gemini_audio_and_video_to_anthropic_fail_closed_before_upstream() {
    for (label, part) in [
        (
            "Gemini audio",
            json!({ "inlineData": { "mimeType": "audio/wav", "data": AUDIO_WAV_B64 } }),
        ),
        (
            "Gemini video",
            json!({ "inlineData": { "mimeType": "video/mp4", "data": "AAAA" } }),
        ),
    ] {
        let (mock_base, _mock, captured) = spawn_asserting_anthropic_mock(move |_| {
            Err(format!("{label} reached Anthropic upstream"))
        })
        .await;
        let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
        let (proxy_base, _proxy) = start_proxy(config).await;

        let response = Client::new()
            .post(format!(
                "{proxy_base}/google/v1beta/models/claude-3-5-sonnet:generateContent"
            ))
            .json(&json!({
                "model": "claude-3-5-sonnet",
                "contents": [{
                    "role": "user",
                    "parts": [
                        { "text": "Inspect this media" },
                        part
                    ]
                }]
            }))
            .send()
            .await
            .unwrap();

        assert_failure_response(response).await;
        assert_no_upstream_request(&captured).await;
    }
}

async fn assert_success_response(response: reqwest::Response) -> Value {
    let status = response.status();
    let body = response.text().await.unwrap();
    assert!(status.is_success(), "status: {status}, body: {body}");
    serde_json::from_str(&body).unwrap_or_else(|err| panic!("invalid JSON response: {err}; {body}"))
}

async fn assert_failure_response(response: reqwest::Response) -> Value {
    let status = response.status();
    let body = response.text().await.unwrap();
    assert!(
        !status.is_success(),
        "request should fail closed before upstream; status: {status}, body: {body}"
    );
    serde_json::from_str(&body).unwrap_or(Value::String(body))
}

async fn assert_upstream_called_once(captured: &CapturedMockRequests) {
    let requests = captured.wait_for_count(1, Duration::from_secs(1)).await;
    assert_eq!(requests.len(), 1, "upstream request count: {requests:?}");
}

async fn assert_no_upstream_request(captured: &CapturedMockRequests) {
    let requests = captured.wait_for_count(1, Duration::from_millis(150)).await;
    assert!(
        requests.is_empty(),
        "fail-closed path must not contact upstream: {requests:?}"
    );
}

fn assert_anthropic_remote_image_url_source(request: &CapturedMockRequest) -> Result<(), String> {
    expect_pointer(&request.body, "/messages/0/role", json!("user"))?;
    expect_pointer(
        &request.body,
        "/messages/0/content/0/text",
        json!("Describe this remote image"),
    )?;
    expect_pointer(&request.body, "/messages/0/content/1/type", json!("image"))?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/type",
        json!("url"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/url",
        json!(REMOTE_IMAGE_URL),
    )
}

fn assert_anthropic_pdf_base64_document(request: &CapturedMockRequest) -> Result<(), String> {
    expect_pointer(&request.body, "/messages/0/role", json!("user"))?;
    expect_pointer(
        &request.body,
        "/messages/0/content/0/text",
        json!("Summarize this PDF"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/type",
        json!("document"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/type",
        json!("base64"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/media_type",
        json!("application/pdf"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/data",
        json!(PDF_B64),
    )
}

fn assert_anthropic_pdf_url_document(request: &CapturedMockRequest) -> Result<(), String> {
    expect_pointer(&request.body, "/messages/0/role", json!("user"))?;
    expect_pointer(
        &request.body,
        "/messages/0/content/0/text",
        json!("Summarize this PDF"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/type",
        json!("document"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/type",
        json!("url"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/url",
        json!(REMOTE_PDF_URL),
    )
}

fn assert_anthropic_gemini_pdf_documents(request: &CapturedMockRequest) -> Result<(), String> {
    expect_pointer(&request.body, "/messages/0/role", json!("user"))?;
    expect_pointer(
        &request.body,
        "/messages/0/content/0/text",
        json!("Summarize these PDFs"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/type",
        json!("document"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/type",
        json!("base64"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/media_type",
        json!("application/pdf"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/data",
        json!(PDF_B64),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/2/type",
        json!("document"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/2/source/type",
        json!("url"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/2/source/url",
        json!(REMOTE_PDF_URL),
    )
}

fn expect_pointer(body: &Value, pointer: &str, expected: Value) -> Result<(), String> {
    let actual = body
        .pointer(pointer)
        .ok_or_else(|| format!("missing JSON pointer `{pointer}` in body: {body}"))?;
    if actual == &expected {
        Ok(())
    } else {
        Err(format!(
            "JSON pointer `{pointer}` expected {expected}, got {actual}; body: {body}"
        ))
    }
}
