use netwatch::config::AppConfig;
use tempfile::tempdir;

#[test]
fn load_save_load_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let original = AppConfig {
        sources: vec!["https://example.com".to_string()],
        latency_threshold_ms: 200,
        check_interval_seconds: 30,
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
    };
    custom.save(&path).unwrap();

    let loaded = AppConfig::load_or_default(&path);
    assert_eq!(loaded, custom);
}