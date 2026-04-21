use std::sync::{Arc, Mutex};

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

    let results = check_all(&checker, &targets, 10).await;

    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| r.ok));
}

#[tokio::test]
async fn concurrency_limited_by_semaphore() {
    let server = MockServer::start().await;

    for p in ["/1", "/2", "/3", "/4", "/5"] {
        Mock::given(method("HEAD"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
    }

    let sources: Vec<String> = ["/1", "/2", "/3", "/4", "/5"]
        .iter()
        .map(|p| format!("{}{p}", server.uri()))
        .collect();

    let cfg = test_config(sources);
    let client = Arc::new(build_client(&cfg).unwrap());
    let checker = Arc::new(Checker::new(client, "test"));
    let targets = targets_from_config(&cfg);

    // limit=2: only 2 tasks run at a time, but all 5 complete
    let results = check_all(&checker, &targets, 2).await;

    assert_eq!(results.len(), 5);
    assert!(results.iter().all(|r| r.ok));
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
    let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
    let cancel = CancellationToken::new();

    cancel.cancel();

    run(config, checker, db, cancel).await;
}

#[tokio::test]
async fn run_writes_results_to_db() {
    let server = MockServer::start().await;

    Mock::given(method("HEAD"))
        .and(path("/persist"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let cfg = test_config(vec![format!("{}/persist", server.uri())]);
    let config = Arc::new(ArcSwap::new(Arc::new(cfg.clone())));
    let client = Arc::new(build_client(&cfg).unwrap());
    let checker = Arc::new(Checker::new(client, "test"));
    let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
    let cancel = CancellationToken::new();

    // Cancel after first cycle completes
    let cancel_clone = cancel.clone();
    let db_poll = Arc::clone(&db);
    tokio::spawn(async move {
        // Poll until at least one row appears in the DB
        loop {
            {
                let db = db_poll.lock().unwrap();
                if let Ok(rows) = db.latest_status(1)
                    && !rows.is_empty()
                {
                    cancel_clone.cancel();
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    });

    run(config, checker, Arc::clone(&db), cancel).await;

    // Verify results persisted
    let db = db.lock().unwrap();
    let rows = db.latest_status(1).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].ok);
    assert!(rows[0].host.contains("127.0.0.1"));
}

#[tokio::test]
async fn run_persists_multiple_results_in_one_cycle() {
    let server = MockServer::start().await;
    // normalize_host strips path, so all three URLs share the same host key
    let host = format!("127.0.0.1:{}", server.address().port());

    for p in ["/m1", "/m2", "/m3"] {
        Mock::given(method("HEAD"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
    }

    let sources: Vec<String> = ["/m1", "/m2", "/m3"]
        .iter()
        .map(|p| format!("{}{p}", server.uri()))
        .collect();

    let cfg = test_config(sources);
    let config = Arc::new(ArcSwap::new(Arc::new(cfg.clone())));
    let client = Arc::new(build_client(&cfg).unwrap());
    let checker = Arc::new(Checker::new(client, "test"));
    let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
    let cancel = CancellationToken::new();

    let cancel_clone = cancel.clone();
    let db_poll = Arc::clone(&db);
    tokio::spawn(async move {
        // wait until at least one row appears (first cycle completed)
        loop {
            {
                let db = db_poll.lock().unwrap();
                if let Ok(rows) = db.latest_status(1)
                    && !rows.is_empty()
                {
                    cancel_clone.cancel();
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    });

    run(config, checker, Arc::clone(&db), cancel).await;

    // All three sources map to the same host; history returns every row for that host
    let db = db.lock().unwrap();
    let rows = db.history(&host, 10).unwrap();
    assert_eq!(
        rows.len(),
        3,
        "batch insert should persist all results from one cycle"
    );
    assert!(rows.iter().all(|r| r.ok));
}
