use super::*;

#[tokio::test]
async fn build_runtime_namespace_state_exposes_resolved_per_upstream_clients() {
    let _env_guard = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let _http_proxy = ScopedEnvVar::remove("HTTP_PROXY");
    let _http_proxy_lower = ScopedEnvVar::remove("http_proxy");
    let _https_proxy = ScopedEnvVar::remove("HTTPS_PROXY");
    let _https_proxy_lower = ScopedEnvVar::remove("https_proxy");
    let _all_proxy = ScopedEnvVar::remove("ALL_PROXY");
    let _all_proxy_lower = ScopedEnvVar::remove("all_proxy");
    let _no_proxy = ScopedEnvVar::remove("NO_PROXY");
    let _no_proxy_lower = ScopedEnvVar::remove("no_proxy");

    let (api_root, requests, server) = spawn_openai_completion_mock(serde_json::json!({
        "id": "chatcmpl_test",
        "object": "chat.completion",
        "created": 1,
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }]
    }))
    .await;
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        proxy: None,
        upstreams: vec![crate::config::UpstreamConfig {
            name: "primary".to_string(),
            api_root: api_root.clone(),
            fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiCompletion),
            provider_key_env: None,
            provider_key: None,
            upstream_headers: Vec::new(),
            proxy: None,
            limits: None,
            surface_defaults: None,
        }],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        conversation_state_bridge: Default::default(),
        data_auth: None,
    };

    let namespace_state = crate::server::state::build_runtime_namespace_state(
        "rev-test".to_string(),
        config.clone(),
        &data_auth::DataAccess::ClientProviderKey,
    )
    .await
    .expect("build runtime namespace state");

    let upstream_state = namespace_state
        .upstreams
        .get("primary")
        .expect("primary upstream state");
    let streaming_client = upstream_state.streaming_client.clone();
    let upstream_client = upstream_state.client.clone();
    let url = crate::config::build_upstream_url(
        &api_root,
        crate::formats::UpstreamFormat::OpenAiCompletion,
        None,
        false,
    );
    let response = crate::upstream::call_upstream_resource(
        &upstream_client,
        reqwest::Method::POST,
        &url,
        Some(&serde_json::json!({
            "model": "gpt-4o",
            "messages": []
        })),
        &[],
    )
    .await
    .expect("call upstream through resolved client");

    assert!(response.status().is_success());
    assert!(crate::upstream::call_upstream_resource(
        &streaming_client,
        reqwest::Method::POST,
        &url,
        Some(&serde_json::json!({
            "model": "gpt-4o",
            "messages": []
        })),
        &[],
    )
    .await
    .expect("call upstream through resolved streaming client")
    .status()
    .is_success());
    assert_eq!(
        &upstream_state.resolved_proxy,
        &crate::upstream::ResolvedProxyMetadata {
            source: crate::upstream::ResolvedProxySource::None,
            target: crate::upstream::ResolvedProxyTarget::Inherited,
        }
    );
    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 2, "requests = {recorded:?}");

    server.abort();
}

#[tokio::test]
async fn dashboard_runtime_snapshot_tracks_live_namespace_state() {
    let mut config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![crate::config::UpstreamConfig {
            name: "auto".to_string(),
            api_root: "https://example.com/v1".to_string(),
            fixed_upstream_format: None,
            provider_key_env: None,
            provider_key: None,
            upstream_headers: Vec::new(),
            proxy: None,
            limits: None,
            surface_defaults: None,
        }],
        model_aliases: Default::default(),
        hooks: crate::config::HookConfig {
            exchange: Some(crate::config::HookEndpointConfig {
                url: "https://example.com/hooks/exchange".to_string(),
                authorization: Some("Bearer hook-1".to_string()),
            }),
            ..Default::default()
        },
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        conversation_state_bridge: Default::default(),
        data_auth: None,
    };
    config.model_aliases.insert(
        "alias-1".to_string(),
        crate::config::ModelAlias {
            upstream_name: "auto".to_string(),
            upstream_model: "model-a".to_string(),
            limits: None,
            surface: None,
        },
    );
    let initial_hooks = crate::hooks::HookDispatcher::new(&config.hooks);
    let mut upstreams = BTreeMap::new();
    let (client, streaming_client, resolved_proxy) = crate::upstream::build_upstream_clients(
        &config,
        config.upstreams[0].proxy.as_ref(),
        config.proxy.as_ref(),
    )
    .expect("build dashboard upstream clients");
    let no_auto_decompression_client = crate::upstream::build_no_auto_decompression_client(
        config.upstream_timeout,
        &resolved_proxy,
    )
    .expect("build dashboard no-auto-decompression upstream client");
    upstreams.insert(
        "auto".to_string(),
        UpstreamState {
            config: config.upstreams[0].clone(),
            provider_key: None,
            capability: None,
            availability: crate::discovery::UpstreamAvailability::Unavailable {
                reason: "protocol discovery returned no supported formats".to_string(),
            },
            client,
            streaming_client,
            no_auto_decompression_client,
            resolved_proxy,
        },
    );

    let mut runtime = RuntimeState::default();
    runtime.namespaces.insert(
        DEFAULT_NAMESPACE.to_string(),
        RuntimeNamespaceState {
            revision: "rev-1".to_string(),
            config: config.clone(),
            hooks: initial_hooks,
            debug_trace: None,
            upstreams,
        },
    );

    let handle = DashboardRuntimeHandle::new(Arc::new(RwLock::new(runtime)));
    let snapshot = handle.snapshot();

    assert_eq!(snapshot.config.model_aliases.len(), 1);
    assert_eq!(snapshot.upstreams.len(), 1);
    assert_eq!(snapshot.upstreams[0].name, "auto");
    assert_eq!(snapshot.upstreams[0].availability_status, "unavailable");
    assert_eq!(
        snapshot.upstreams[0].availability_reason.as_deref(),
        Some("protocol discovery returned no supported formats")
    );
    assert!(snapshot.hooks.is_some());

    {
        let mut runtime = handle.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.model_aliases.insert(
            "alias-2".to_string(),
            crate::config::ModelAlias {
                upstream_name: "auto".to_string(),
                upstream_model: "model-b".to_string(),
                limits: None,
                surface: None,
            },
        );
        namespace.upstreams.get_mut("auto").unwrap().availability =
            crate::discovery::UpstreamAvailability::Available;
        namespace.hooks = crate::hooks::HookDispatcher::new(&crate::config::HookConfig::default());
    }

    let updated = handle.snapshot();
    assert_eq!(updated.config.model_aliases.len(), 2);
    assert_eq!(updated.upstreams[0].availability_status, "available");
    assert!(updated.upstreams[0].availability_reason.is_none());
    assert!(updated.hooks.is_none());
}
