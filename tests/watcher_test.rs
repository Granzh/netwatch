use netwatch::config::AppConfig;
use netwatch::watcher::{ConfigStore, should_debounce};
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
fn debounce_first_event_always_passes() {
    // prev=MAX means no prior event — should never debounce
    assert!(!should_debounce(u64::MAX, 1_000_000, 5_000_000_000));
}

#[test]
fn debounce_suppresses_within_window() {
    let debounce_ns = 5_000_000_000; // 5 seconds
    let first_event = 1_000_000_000; // 1s mark
    let second_event = 1_200_000_000; // 1.2s mark (200ms later)

    assert!(!should_debounce(u64::MAX, first_event, debounce_ns));
    assert!(should_debounce(first_event, second_event, debounce_ns));
}

#[test]
fn debounce_allows_after_window_expires() {
    let debounce_ns = 5_000_000_000; // 5 seconds
    let first_event = 1_000_000_000; // 1s mark
    let after_window = 7_000_000_000; // 7s mark (6s later, past 5s window)

    assert!(!should_debounce(first_event, after_window, debounce_ns));
}

#[test]
fn debounce_zero_window_never_suppresses() {
    assert!(!should_debounce(100, 101, 0));
    assert!(!should_debounce(100, 100, 0));
}

#[test]
fn debounce_exact_boundary_suppresses() {
    let debounce_ns = 1_000;
    // Exactly at the boundary edge (999ns elapsed < 1000ns window) — should suppress
    assert!(should_debounce(1_000, 1_999, debounce_ns));
    // Exactly at the boundary (1000ns elapsed == 1000ns window) — should pass
    assert!(!should_debounce(1_000, 2_000, debounce_ns));
}
