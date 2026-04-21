use super::*;

#[test]
fn responses_stateful_request_controls_detect_provider_owned_fields() {
    let controls = responses_stateful_request_controls(&serde_json::json!({
        "previous_response_id": "resp_1",
        "conversation": { "id": "conv_1" },
        "background": true,
        "store": true,
        "prompt": { "id": "pmpt_1" }
    }));

    assert_eq!(
        controls,
        vec![
            "previous_response_id",
            "conversation",
            "background",
            "store",
            "prompt"
        ]
    );
}

#[test]
fn resolve_native_responses_stateful_route_or_error_prefers_unique_available_native_upstream() {
    let namespace_state = runtime_namespace_state_for_tests(&[
        (
            "responses",
            crate::formats::UpstreamFormat::OpenAiResponses,
            true,
        ),
        ("anthropic", crate::formats::UpstreamFormat::Anthropic, true),
    ]);

    let resolved = resolve_native_responses_stateful_route_or_error(
        &namespace_state,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "previous_response_id": "resp_1" }),
    )
    .expect("resolver should succeed")
    .expect("native route should be selected");

    assert_eq!(resolved.upstream_name, "responses");
    assert!(resolved.upstream_model.is_empty());
}

#[test]
fn resolve_native_responses_stateful_route_or_error_rejects_multiple_native_upstreams() {
    let namespace_state = runtime_namespace_state_for_tests(&[
        ("a", crate::formats::UpstreamFormat::OpenAiResponses, true),
        ("b", crate::formats::UpstreamFormat::OpenAiResponses, true),
    ]);

    let error = resolve_native_responses_stateful_route_or_error(
        &namespace_state,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "background": true }),
    )
    .expect_err("multiple native upstreams should fail");

    assert!(error.contains("multiple configured native OpenAI Responses upstreams"));
}

#[test]
fn resolve_native_responses_stateful_route_or_error_rejects_multiple_configured_native_upstreams_even_if_only_one_is_available(
) {
    let namespace_state = runtime_namespace_state_for_tests(&[
        ("a", crate::formats::UpstreamFormat::OpenAiResponses, true),
        ("b", crate::formats::UpstreamFormat::OpenAiResponses, false),
    ]);

    let error = resolve_native_responses_stateful_route_or_error(
        &namespace_state,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "previous_response_id": "resp_1" }),
    )
    .expect_err("configured ownership should stay ambiguous");

    assert!(error.contains("multiple"));
    assert!(error.contains("configured"));
    assert!(error.contains("previous_response_id"));
}

#[test]
fn resolve_native_responses_stateful_route_or_error_rejects_multi_upstream_auto_discovery_without_explicit_owner_pin(
) {
    let pinned = test_upstream_config("responses", crate::formats::UpstreamFormat::OpenAiResponses);
    let auto = test_upstream_config_with_fixed_format("auto", None);
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        compatibility_mode: crate::config::CompatibilityMode::Balanced,
        upstreams: vec![pinned.clone(), auto.clone()],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
    };
    let upstreams = std::collections::BTreeMap::from([
        (
            "responses".to_string(),
            UpstreamState {
                config: pinned,
                capability: Some(crate::discovery::UpstreamCapability::fixed(
                    crate::formats::UpstreamFormat::OpenAiResponses,
                )),
                availability: crate::discovery::UpstreamAvailability::available(),
            },
        ),
        (
            "auto".to_string(),
            UpstreamState {
                config: auto,
                capability: Some(crate::discovery::UpstreamCapability::fixed(
                    crate::formats::UpstreamFormat::Anthropic,
                )),
                availability: crate::discovery::UpstreamAvailability::available(),
            },
        ),
    ]);
    let namespace_state = RuntimeNamespaceState {
        revision: "test-revision".to_string(),
        config,
        upstreams,
        client: Client::new(),
        streaming_client: Client::new(),
        hooks: None,
        debug_trace: None,
    };

    let error = resolve_native_responses_stateful_route_or_error(
        &namespace_state,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "background": true }),
    )
    .expect_err("auto-discovery should block provenance-free routing");

    assert!(error.contains("auto-discovery"));
    assert!(error.contains("fixed_upstream_format"));
}
