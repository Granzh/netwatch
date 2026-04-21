use netwatch::config::{AppConfig, parse_listen_addr};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use tempfile::tempdir;

#[test]
fn load_save_load_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let original = AppConfig {
        sources: vec!["https://example.com".to_string()],
        latency_threshold_ms: 200,
        http_api: "127.0.0.1".to_string(),
        check_interval_seconds: 30,
        check_jitter_seconds: 3,
        max_concurrent_checks: 5,
        request_timeout_secs: 5,
        follow_redirects: true,
        danger_accept_invalid_certs: false,
        listen_port: 9090,
        api_secret: Some("test-secret".to_string()),
        node_id: "test-node".to_string(),
        peers: vec!["http://peer1:8080".to_string()],
        sync_interval_seconds: 30,
        max_concurrent_syncs: 5,
        sync_timeout_secs: 30,
    };

    original.save(&path).unwrap();
    let loaded = AppConfig::load(&path).unwrap();

    assert_eq!(original, loaded);
}

#[test]
fn load_or_default_creates_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    assert!(!path.exists());

    let config = AppConfig::load_or_default(&path);

    assert!(path.exists(), "config.toml should be created on first run");
    assert_eq!(config, AppConfig::default());

    let reloaded = AppConfig::load(&path).unwrap();
    assert_eq!(config, reloaded);
}

#[test]
fn load_or_default_returns_existing() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let custom = AppConfig {
        sources: vec!["https://custom.example.com".to_string()],
        latency_threshold_ms: 500,
        check_interval_seconds: 10,
        check_jitter_seconds: 2,
        max_concurrent_checks: 20,
        request_timeout_secs: 15,
        follow_redirects: false,
        http_api: "127.0.0.1".to_string(),
        danger_accept_invalid_certs: true,
        listen_port: 3000,
        api_secret: None,
        node_id: "custom-node".to_string(),
        peers: vec![],
        sync_interval_seconds: 120,
        max_concurrent_syncs: 5,
        sync_timeout_secs: 30,
    };
    custom.save(&path).unwrap();

    let loaded = AppConfig::load_or_default(&path);
    assert_eq!(loaded, custom);
}

#[test]
fn parse_listen_addr_ipv4() {
    let ip = parse_listen_addr("127.0.0.1").unwrap();
    assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
}

#[test]
fn parse_listen_addr_ipv4_wildcard() {
    let ip = parse_listen_addr("0.0.0.0").unwrap();
    assert_eq!(ip, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
}

#[test]
fn parse_listen_addr_ipv6() {
    let ip = parse_listen_addr("::1").unwrap();
    assert_eq!(ip, IpAddr::V6(Ipv6Addr::LOCALHOST));
}

#[test]
fn parse_listen_addr_ipv6_wildcard() {
    let ip = parse_listen_addr("::").unwrap();
    assert_eq!(ip, IpAddr::V6(Ipv6Addr::UNSPECIFIED));
}

#[test]
fn parse_listen_addr_invalid_returns_error() {
    let err = parse_listen_addr("not-an-ip").unwrap_err();
    assert!(err.to_string().contains("not-an-ip"));
}

#[test]
fn load_returns_error_on_missing_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("nonexistent.toml");
    let err = AppConfig::load(&path).unwrap_err();
    assert!(
        matches!(err, netwatch::config::ConfigError::Io(ref e) if e.kind() == std::io::ErrorKind::NotFound)
    );
}

#[test]
fn load_returns_error_on_invalid_toml() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad.toml");
    std::fs::write(&path, "not = [valid toml syntax {{{{").unwrap();
    assert!(matches!(
        AppConfig::load(&path).unwrap_err(),
        netwatch::config::ConfigError::Parse(_)
    ));
}
