use netwatch::config::AppConfig;
use netwatch::watcher::ConfigStore;
use std::time::Duration;
use tempfile::tempdir;

fn custom_config() -> AppConfig {
    AppConfig {
        sources: vec!["https://custom.example.com".to_string()],
        latency_threshold_ms: 250,
        check_interval_seconds: 15,
        ..AppConfig::default()
    }
}

#[test]
fn loads_default_when_file_missing() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let store = ConfigStore::new(&path, Duration::from_millis(50)).unwrap();
    assert_eq!(**store.get(), AppConfig::default());
}

#[test]
fn loads_existing_config_from_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let cfg = custom_config();
    cfg.save(&path).unwrap();

    let store = ConfigStore::new(&path, Duration::from_millis(50)).unwrap();
    assert_eq!(**store.get(), cfg);
}

#[test]
fn get_reflects_current_config() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let cfg = custom_config();
    cfg.save(&path).unwrap();

    let store = ConfigStore::new(&path, Duration::from_millis(50)).unwrap();
    let got = store.get();
    assert_eq!(got.latency_threshold_ms, 250);
    assert_eq!(got.check_interval_seconds, 15);
    assert_eq!(got.sources, vec!["https://custom.example.com"]);
}

#[test]
fn arc_reflects_same_config_as_get() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let cfg = custom_config();
    cfg.save(&path).unwrap();

    let store = ConfigStore::new(&path, Duration::from_millis(50)).unwrap();
    let via_get = store.get();
    let arc = store.arc();
    let via_arc = arc.load();
    assert_eq!(**via_get, **via_arc);
}

#[test]
fn hot_reload_on_file_change() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let initial = AppConfig::default();
    initial.save(&path).unwrap();

    let store = ConfigStore::new(&path, Duration::from_millis(0)).unwrap();
    assert_eq!(**store.get(), initial);

    let updated = AppConfig {
        sources: vec!["https://updated.example.com".to_string()],
        latency_threshold_ms: 42,
        check_interval_seconds: 5,
        ..AppConfig::default()
    };
    updated.save(&path).unwrap();

    // Poll until the store reflects the new config or we time out.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if **store.get() == updated {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for hot reload"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn debounce_suppresses_rapid_reloads() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let initial = AppConfig::default();
    initial.save(&path).unwrap();

    // Large debounce window.
    let store = ConfigStore::new(&path, Duration::from_millis(5_000)).unwrap();

    // Wait for any spurious startup events to settle and pass through the
    // debounce gate, so the next intentional write lands inside the window.
    std::thread::sleep(Duration::from_millis(500));

    // First write — may or may not pass debounce depending on whether the
    // watcher fired a startup event. Either way, write a second time
    // immediately after and verify it is suppressed.
    let first_write = AppConfig {
        sources: vec!["https://first.example.com".to_string()],
        latency_threshold_ms: 10,
        check_interval_seconds: 1,
        ..AppConfig::default()
    };
    first_write.save(&path).unwrap();
    std::thread::sleep(Duration::from_millis(200));

    let second_write = AppConfig {
        sources: vec!["https://second.example.com".to_string()],
        latency_threshold_ms: 20,
        check_interval_seconds: 2,
        ..AppConfig::default()
    };
    second_write.save(&path).unwrap();
    std::thread::sleep(Duration::from_millis(300));

    // The store should NOT reflect the second write — it must have been
    // debounced because it arrived within the 5s window of the prior event.
    let got = store.get();
    assert_ne!(
        **got, second_write,
        "second rapid write should be debounced"
    );
}
