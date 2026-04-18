use super::*;

#[tokio::test]
async fn dashboard_runtime_snapshot_tracks_live_namespace_state() {
    let mut config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        upstreams: vec![crate::config::UpstreamConfig {
            name: "auto".to_string(),
            api_root: "https://example.com/v1".to_string(),
            fixed_upstream_format: None,
            fallback_credential_env: None,
            fallback_credential_actual: None,
            fallback_api_key: None,
            auth_policy: crate::config::AuthPolicy::ClientOrFallback,
            upstream_headers: Vec::new(),
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
    };
    config.model_aliases.insert(
        "alias-1".to_string(),
        crate::config::ModelAlias {
            upstream_name: "auto".to_string(),
            upstream_model: "model-a".to_string(),
        },
    );
    let initial_hooks = crate::hooks::HookDispatcher::new(&config.hooks);
    let mut upstreams = BTreeMap::new();
    upstreams.insert(
        "auto".to_string(),
        UpstreamState {
            config: config.upstreams[0].clone(),
            capability: None,
            availability: crate::discovery::UpstreamAvailability::Unavailable {
                reason: "protocol discovery returned no supported formats".to_string(),
            },
        },
    );

    let mut runtime = RuntimeState::default();
    runtime.namespaces.insert(
        DEFAULT_NAMESPACE.to_string(),
        RuntimeNamespaceState {
            revision: "rev-1".to_string(),
            config: config.clone(),
            client: crate::upstream::build_client(&config),
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
