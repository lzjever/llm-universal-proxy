use super::*;

async fn start_test_proxy() -> (
    String,
    tokio::task::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind proxy");
    let port = listener.local_addr().expect("proxy local addr").port();
    let base = format!("http://127.0.0.1:{port}");
    let handle =
        tokio::spawn(
            async move { run_with_listener(crate::config::Config::default(), listener).await },
        );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (base, handle)
}

fn dashboard_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("dashboard test client")
}

#[tokio::test]
async fn dashboard_shell_is_public_when_admin_token_is_configured() {
    let _env_guard = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::set("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN", "dashboard-secret");
    let (proxy_base, _proxy) = start_test_proxy().await;
    let client = dashboard_client();

    let response = client
        .get(format!("{proxy_base}/dashboard"))
        .header("origin", "https://example.com")
        .send()
        .await
        .expect("dashboard response");

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
    assert!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html")),
        "dashboard index should be served as HTML"
    );
    let body = response.text().await.expect("dashboard body");
    assert!(body.contains("LLM Universal Proxy Admin"));
    assert!(body.contains("Admin token"));
    assert!(body.contains("Bearer token"));
    assert!(body.contains("existing admin API"));
    assert!(body.contains("placeholder=\"Paste admin token\""));
    assert!(!body.contains("placeholder=\"Paste LLM_UNIVERSAL_PROXY_ADMIN_TOKEN\""));
}

#[tokio::test]
async fn dashboard_static_assets_are_public_shell_resources_with_content_types() {
    let _env_guard = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::set("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN", "dashboard-secret");
    let (proxy_base, _proxy) = start_test_proxy().await;
    let client = dashboard_client();

    let js = client
        .get(format!("{proxy_base}/dashboard/assets/app.js"))
        .send()
        .await
        .expect("dashboard app script response");
    assert_eq!(js.status(), StatusCode::OK);
    assert!(
        js.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/javascript")),
        "dashboard app script should be served as JavaScript"
    );
    let js_body = js.text().await.expect("asset body");
    assert!(js_body.contains("DashboardClient"));
    assert!(js_body.contains("Authorization"));
    assert!(js_body.contains("/admin/state"));
    assert!(js_body.contains("/admin/namespaces/"));

    let css = client
        .get(format!("{proxy_base}/dashboard/assets/app.css"))
        .header("origin", "https://example.com")
        .send()
        .await
        .expect("dashboard stylesheet response");
    assert_eq!(css.status(), StatusCode::OK);
    assert!(css.headers().get("access-control-allow-origin").is_none());
    assert!(
        css.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/css")),
        "dashboard stylesheet should be served as CSS"
    );
    let css_body = css.text().await.expect("css body");
    assert!(css_body.contains(".dashboard"));
    assert!(css_body.contains("@media (max-width: 640px)"));
    assert!(css_body.contains("width: calc(100vw - 3rem)"));
    assert!(css_body.contains("max-width: calc(100vw - 3rem)"));
    assert!(css_body.contains("margin-left: 1rem"));
    assert!(css_body.contains("overflow-x: hidden"));
    assert!(css_body.contains("min-width: 0"));
    assert!(css_body.contains("overflow-wrap: anywhere"));
    assert!(css_body.contains("word-break: break-word"));
    assert!(css_body.contains("font-size: clamp(1.75rem, 10vw, 2.35rem)"));
    assert!(css_body.contains(".auth-panel p"));
    assert!(css_body.contains(".namespace-card span"));
    assert!(css_body.contains("justify-items: stretch"));
    assert!(css_body.contains("width: 100%"));
    assert!(css_body.contains(".auth-panel > *"));
    assert!(css_body.contains(".auth-row button"));
}

#[tokio::test]
async fn admin_endpoints_still_require_bearer_when_dashboard_shell_is_public() {
    let _env_guard = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::set("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN", "dashboard-secret");
    let (proxy_base, _proxy) = start_test_proxy().await;
    let client = dashboard_client();

    let dashboard = client
        .get(format!("{proxy_base}/dashboard"))
        .send()
        .await
        .expect("dashboard shell response");
    assert_eq!(dashboard.status(), StatusCode::OK);

    let missing = client
        .get(format!("{proxy_base}/admin/state"))
        .send()
        .await
        .expect("missing admin token response");
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

    let wrong = client
        .get(format!("{proxy_base}/admin/state"))
        .header("authorization", "Bearer wrong-token")
        .send()
        .await
        .expect("wrong admin token response");
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let admin = client
        .get(format!("{proxy_base}/admin/state"))
        .header("authorization", "Bearer dashboard-secret")
        .send()
        .await
        .expect("authorized admin state response");
    assert_eq!(admin.status(), StatusCode::OK);
    let body: serde_json::Value = admin.json().await.expect("admin state json");
    assert!(body["namespaces"].is_array());
}

#[tokio::test]
async fn dashboard_copy_keeps_redacted_state_read_only_and_requires_full_payload() {
    let _env_guard = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::set("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN", "dashboard-secret");
    let (proxy_base, _proxy) = start_test_proxy().await;
    let client = dashboard_client();

    let body = client
        .get(format!("{proxy_base}/dashboard"))
        .send()
        .await
        .expect("dashboard response")
        .text()
        .await
        .expect("dashboard body");

    assert!(body.contains("Redacted State"));
    assert!(body.contains("Paste a complete runtime config payload"));
    assert!(body.contains("Do not submit redacted state from above"));
    assert!(body.contains("redacted secrets are intentionally not editable payloads"));
}
