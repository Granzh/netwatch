use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use netwatch::checker::{Checker, build_client};
use netwatch::config::AppConfig;
use netwatch::db::Db;
use netwatch::scheduler::{check_all, jitter_duration, run, targets_from_config};

fn test_config(sources: Vec<String>) -> AppConfig {
    AppConfig {
        sources,
        check_interval_seconds: 0,
        check_jitter_seconds: 0,
        ..AppConfig::default()
    }
}

#[tokio::test]
async fn all_targets_processed_in_one_cycle() {
    let server = MockServer::start().await;

    for p in ["/a", "/b", "/c"] {
        Mock::given(method("HEAD"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
    }

    let sources: Vec<String> = ["/a", "/b", "/c"]
        .iter()
        .map(|p| format!("{}{p}", server.uri()))
        .collect();

    let cfg = test_config(sources);
    let client = Arc::new(build_client(&cfg).unwrap());
    let checker = Arc::new(Checker::new(client, "test"));
    let targets = targets_from_config(&cfg);

    let results = check_all(&checker, &targets).await;

    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| r.ok));
    // wiremock's expect(1) verifies each endpoint was hit exactly once
}

#[tokio::test]
async fn jitter_in_correct_range() {
    let config = AppConfig {
        check_interval_seconds: 10,
        check_jitter_seconds: 5,
        ..AppConfig::default()
    };

    let mut seen = std::collections::HashSet::new();
    for _ in 0..200 {
        let dur = jitter_duration(&config);
        let secs = dur.as_secs();
        assert!(
            (10..=15).contains(&secs),
            "jitter {secs}s outside range 10..=15"
        );
        seen.insert(secs);
    }
    // With 200 samples over 6 possible values, we should hit at least 3
    assert!(
        seen.len() >= 3,
        "expected at least 3 distinct jitter values, got {}",
        seen.len()
    );
}

#[tokio::test]
async fn jitter_zero_means_fixed_interval() {
    let config = AppConfig {
        check_interval_seconds: 30,
        check_jitter_seconds: 0,
        ..AppConfig::default()
    };

    for _ in 0..50 {
        assert_eq!(jitter_duration(&config).as_secs(), 30);
    }
}

#[tokio::test]
async fn graceful_shutdown_does_not_panic() {
    let server = MockServer::start().await;

    Mock::given(method("HEAD"))
        .and(path("/x"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let cfg = test_config(vec![format!("{}/x", server.uri())]);
    let config = Arc::new(ArcSwap::new(Arc::new(cfg.clone())));
    let client = Arc::new(build_client(&cfg).unwrap());
    let checker = Arc::new(Checker::new(client, "test"));
    let db = Db::open_in_memory().unwrap();
    let cancel = CancellationToken::new();

    // Cancel immediately — run() should complete its first cycle then exit
    cancel.cancel();

    run(config, checker, db, cancel).await;
    // If we reach here, shutdown was graceful
}

#[tokio::test]
async fn results_saved_to_db() {
    let server = MockServer::start().await;

    Mock::given(method("HEAD"))
        .and(path("/db-test"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let cfg = test_config(vec![format!("{}/db-test", server.uri())]);
    let client = Arc::new(build_client(&cfg).unwrap());
    let checker = Arc::new(Checker::new(client, "test"));
    let db = Db::open_in_memory().unwrap();

    // Use check_all directly to verify results + db insert
    let targets = targets_from_config(&cfg);
    let results = check_all(&checker, &targets).await;

    assert_eq!(results.len(), 1);
    assert!(results[0].ok);

    db.insert(&results[0]).unwrap();
    let history = db.history(&results[0].host, 10).unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0].ok);
}
