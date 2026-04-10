use std::sync::Arc;
use std::time::Duration;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use netwatch::checker::{Checker, build_client};
use netwatch::config::AppConfig;
use netwatch::models::Target;

fn target(url: &str) -> Target {
    Target {
        name: "test".to_string(),
        url: url.to_string(),
        is_peer: false,
    }
}

#[tokio::test]
async fn check_200_ok() {
    let server = MockServer::start().await;

    Mock::given(method("HEAD"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let config = AppConfig::default();
    let client = Arc::new(build_client(&config).unwrap());
    let checker = Checker::new(client, "test-node");

    let result = checker
        .check(&target(&format!("{}/health", server.uri())))
        .await;

    assert!(result.ok);
    assert!(result.latency_ms < 5000);
    assert_eq!(result.source, "test-node");
}

#[tokio::test]
async fn check_503_service_unavailable() {
    let server = MockServer::start().await;

    Mock::given(method("HEAD"))
        .and(path("/down"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let config = AppConfig::default();
    let client = Arc::new(build_client(&config).unwrap());
    let checker = Checker::new(client, "test-node");

    let result = checker
        .check(&target(&format!("{}/down", server.uri())))
        .await;

    assert!(!result.ok);
}

#[tokio::test]
async fn check_head_405_falls_back_to_get() {
    let server = MockServer::start().await;

    Mock::given(method("HEAD"))
        .and(path("/no-head"))
        .respond_with(ResponseTemplate::new(405))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/no-head"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let config = AppConfig::default();
    let client = Arc::new(build_client(&config).unwrap());
    let checker = Checker::new(client, "test-node");

    let result = checker
        .check(&target(&format!("{}/no-head", server.uri())))
        .await;

    assert!(result.ok);
}

#[tokio::test]
async fn check_timeout() {
    let server = MockServer::start().await;

    Mock::given(method("HEAD"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("ok")
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&server)
        .await;

    let config = AppConfig {
        request_timeout_secs: 1,
        ..AppConfig::default()
    };

    let client = Arc::new(build_client(&config).unwrap());
    let checker = Checker::new(client, "test-node");

    let result = checker
        .check(&target(&format!("{}/slow", server.uri())))
        .await;

    assert!(!result.ok);
}

#[tokio::test]
async fn check_redirect_followed() {
    let server = MockServer::start().await;

    Mock::given(method("HEAD"))
        .and(path("/old"))
        .respond_with(
            ResponseTemplate::new(301).insert_header("Location", &format!("{}/new", server.uri())),
        )
        .mount(&server)
        .await;

    Mock::given(method("HEAD"))
        .and(path("/new"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let config = AppConfig::default();
    let client = Arc::new(build_client(&config).unwrap());
    let checker = Checker::new(client, "test-node");

    let result = checker
        .check(&target(&format!("{}/old", server.uri())))
        .await;

    assert!(result.ok);
}

#[tokio::test]
async fn check_unreachable_host_does_not_panic() {
    let config = AppConfig {
        request_timeout_secs: 1,
        ..AppConfig::default()
    };
    let client = Arc::new(build_client(&config).unwrap());
    let checker = Checker::new(client, "test-node");

    let result = checker.check(&target("http://127.0.0.1:1")).await;

    assert!(!result.ok);
    assert!(!result.host.is_empty());
}

#[tokio::test]
async fn check_redirect_not_followed_when_disabled() {
    let server = MockServer::start().await;

    Mock::given(method("HEAD"))
        .and(path("/redir"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", &format!("{}/dest", server.uri())),
        )
        .mount(&server)
        .await;

    let config = AppConfig {
        follow_redirects: false,
        ..AppConfig::default()
    };

    let client = Arc::new(build_client(&config).unwrap());
    let checker = Checker::new(client, "test-node");

    let result = checker
        .check(&target(&format!("{}/redir", server.uri())))
        .await;

    // 302 is a redirection status, so ok = true (is_redirection)
    assert!(result.ok);
}

#[tokio::test]
async fn check_records_latency() {
    let server = MockServer::start().await;

    Mock::given(method("HEAD"))
        .and(path("/latency"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(100)))
        .mount(&server)
        .await;

    let config = AppConfig::default();
    let client = Arc::new(build_client(&config).unwrap());
    let checker = Checker::new(client, "test-node");

    let result = checker
        .check(&target(&format!("{}/latency", server.uri())))
        .await;

    assert!(result.ok);
    assert!(
        result.latency_ms >= 50,
        "latency should be at least ~100ms, got {}",
        result.latency_ms
    );
}
