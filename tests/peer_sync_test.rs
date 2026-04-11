use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use netwatch::api::{AppState, router};
use netwatch::config::AppConfig;
use netwatch::db::Db;
use netwatch::models::CheckResult;
use netwatch::peer_sync::resolve_sync_url;

fn make_result(host: &str, source: &str) -> CheckResult {
    CheckResult {
        host: host.to_string(),
        ok: true,
        latency_ms: 10,
        timestamp: Utc::now(),
        source: source.to_string(),
    }
}

async fn start_node(node_id: &str, api_secret: Option<String>) -> (String, Arc<Mutex<Db>>) {
    let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
    let state = AppState {
        node_id: node_id.to_string(),
        db: Arc::clone(&db),
        api_secret,
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://{addr}"), db)
}

#[tokio::test]
async fn two_nodes_sync_data() {
    // Start two nodes
    let (_url_a, db_a) = start_node("node-a", None).await;
    let (url_b, db_b) = start_node("node-b", None).await;

    // Seed each node with local data
    {
        let db = db_a.lock().unwrap();
        db.insert(&make_result("host-from-a.com", "node-a"))
            .unwrap();
    }
    {
        let db = db_b.lock().unwrap();
        db.insert(&make_result("host-from-b.com", "node-b"))
            .unwrap();
    }

    // Configure peer sync: node-a syncs with node-b
    let config_a = AppConfig {
        node_id: "node-a".to_string(),
        peers: vec![url_b.clone()],
        sync_interval_seconds: 1,
        ..AppConfig::default()
    };
    let config_arc = Arc::new(arc_swap::ArcSwap::new(Arc::new(config_a)));
    let client = reqwest::Client::new();
    let cancel = CancellationToken::new();

    // Run one sync cycle then cancel
    let cancel_clone = cancel.clone();
    let db_a_poll = Arc::clone(&db_a);
    tokio::spawn(async move {
        loop {
            if let Ok(db) = db_a_poll.lock() {
                // Check if peer data arrived
                let history = db.history("host-from-b.com", 10).unwrap_or_default();
                if !history.is_empty() {
                    cancel_clone.cancel();
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    // Timeout safety
    let cancel_timeout = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        cancel_timeout.cancel();
    });

    netwatch::peer_sync::run(config_arc, client, Arc::clone(&db_a), cancel).await;

    // Verify: node-a should have node-b's data with source "peer:node-b"
    let db = db_a.lock().unwrap();
    let history = db.history("host-from-b.com", 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].source, "peer:node-b");
    assert!(history[0].ok);

    // node-b should have node-a's data (returned in sync response, stored by API handler)
    // Actually, the API sync handler stores incoming data, so node-b should have node-a's data
    let db = db_b.lock().unwrap();
    let history = db.history("host-from-a.com", 10).unwrap();
    assert_eq!(history.len(), 1);
}

#[tokio::test]
async fn unavailable_peer_does_not_crash() {
    let config = AppConfig {
        node_id: "node-alone".to_string(),
        peers: vec!["http://127.0.0.1:1".to_string()],
        sync_interval_seconds: 1,
        ..AppConfig::default()
    };

    let config_arc = Arc::new(arc_swap::ArcSwap::new(Arc::new(config)));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
    let cancel = CancellationToken::new();

    // Cancel immediately — should complete one cycle without panicking
    cancel.cancel();

    netwatch::peer_sync::run(config_arc, client, db, cancel).await;
}

#[tokio::test]
async fn sync_with_secret_header() {
    let secret = "shared-secret".to_string();

    let (url_a, db_a) = start_node("node-a", Some(secret.clone())).await;
    let (_url_b, db_b) = start_node("node-b", Some(secret.clone())).await;

    // Seed node-a
    {
        let db = db_a.lock().unwrap();
        db.insert(&make_result("guarded.com", "node-a")).unwrap();
    }

    // node-b syncs with node-a using the correct secret
    let config_b = AppConfig {
        node_id: "node-b".to_string(),
        peers: vec![url_a],
        sync_interval_seconds: 1,
        api_secret: Some(secret),
        ..AppConfig::default()
    };

    let config_arc = Arc::new(arc_swap::ArcSwap::new(Arc::new(config_b)));
    let client = reqwest::Client::new();
    let cancel = CancellationToken::new();

    let cancel_clone = cancel.clone();
    let db_b_poll = Arc::clone(&db_b);
    tokio::spawn(async move {
        loop {
            if let Ok(db) = db_b_poll.lock()
                && !db.history("guarded.com", 10).unwrap_or_default().is_empty()
            {
                cancel_clone.cancel();
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    let cancel_timeout = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        cancel_timeout.cancel();
    });

    netwatch::peer_sync::run(config_arc, client, Arc::clone(&db_b), cancel).await;

    let db = db_b.lock().unwrap();
    let history = db.history("guarded.com", 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].source, "peer:node-a");
}

#[test]
fn resolve_sync_url_basic() {
    let url = resolve_sync_url("http://127.0.0.1:8080").unwrap();
    assert_eq!(url.as_str(), "http://127.0.0.1:8080/api/sync");
}

#[test]
fn resolve_sync_url_with_trailing_slash() {
    let url = resolve_sync_url("http://peer.local:9090/").unwrap();
    assert_eq!(url.as_str(), "http://peer.local:9090/api/sync");
}

#[test]
fn resolve_sync_url_with_path() {
    let url = resolve_sync_url("http://peer.local:9090/some/prefix").unwrap();
    assert_eq!(url.as_str(), "http://peer.local:9090/api/sync");
}

#[test]
fn resolve_sync_url_invalid() {
    assert!(resolve_sync_url("not a url").is_none());
}

#[tokio::test]
async fn empty_peers_list_does_nothing() {
    let config = AppConfig {
        node_id: "node-lonely".to_string(),
        peers: vec![],
        sync_interval_seconds: 1,
        ..AppConfig::default()
    };

    let config_arc = Arc::new(arc_swap::ArcSwap::new(Arc::new(config)));
    let client = reqwest::Client::new();
    let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
    let cancel = CancellationToken::new();

    cancel.cancel();
    netwatch::peer_sync::run(config_arc, client, db, cancel).await;
    // No panic = success
}
