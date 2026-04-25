use axum::{
    body::Body,
    http::{header, Response, StatusCode},
};

const INDEX_HTML: &str = include_str!("web_dashboard/index.html");
const APP_CSS: &str = include_str!("web_dashboard/app.css");
const APP_JS: &str = include_str!("web_dashboard/app.js");

pub(super) async fn handle_dashboard_index() -> Response<Body> {
    static_response("text/html; charset=utf-8", INDEX_HTML)
}

pub(super) async fn handle_dashboard_css() -> Response<Body> {
    static_response("text/css; charset=utf-8", APP_CSS)
}

pub(super) async fn handle_dashboard_js() -> Response<Body> {
    static_response("application/javascript; charset=utf-8", APP_JS)
}

fn static_response(content_type: &'static str, body: &'static str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .body(Body::from(body))
        .expect("dashboard static response")
}
