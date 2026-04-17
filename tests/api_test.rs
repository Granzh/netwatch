use std::sync::{Arc, Mutex};

use chrono::Utc;
use tokio::net::TcpListener;

use netwatch::api::{AppState, router};
use netwatch::db::Db;
use netwatch::models::{CheckResult, PeerReport};

fn sample_result(host: &str, ok: bool) -> CheckResult {
    CheckResult {
        host: host.to_string(),
        ok,
        latency_ms: 42,
        timestamp: Utc::now(),
        source: "test-node".to_string(),
    }
}

fn result_from(host: &str, source: &str) -> CheckResult {
    CheckResult {
        host: host.to_string(),
        ok: true,
        latency_ms: 10,
        timestamp: Utc::now(),
        source: source.to_string(),
    }
}

async fn spawn_server(api_secret: Option<String>) -> (String, Arc<Mutex<Db>>) {
    let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
    let state = AppState {
        node_id: "node-test".to_string(),
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

async fn spawn_open_server() -> (String, Arc<Mutex<Db>>) {
    spawn_server(None).await
}

// --- status ---

#[tokio::test]
async fn status_returns_empty_json_initially() {
    let (base, _db) = spawn_open_server().await;

    let resp = reqwest::get(format!("{base}/api/status")).await.unwrap();
    assert_eq!(resp.status(), 200);

    let body: Vec<CheckResult> = resp.json().await.unwrap();
    assert!(body.is_empty());
}

#[tokio::test]
async fn status_returns_inserted_results() {
    let (base, db) = spawn_open_server().await;

    {
        let db = db.lock().unwrap();
        db.insert(&sample_result("example.com", true)).unwrap();
        db.insert(&sample_result("down.com", false)).unwrap();
    }

    let resp = reqwest::get(format!("{base}/api/status")).await.unwrap();
    assert_eq!(resp.status(), 200);

    let body: Vec<CheckResult> = resp.json().await.unwrap();
    assert_eq!(body.len(), 2);

    let hosts: Vec<&str> = body.iter().map(|r| r.host.as_str()).collect();
    assert!(hosts.contains(&"down.com"));
    assert!(hosts.contains(&"example.com"));
}

// --- history ---

#[tokio::test]
async fn history_returns_results_for_host() {
    let (base, db) = spawn_open_server().await;

    {
        let db = db.lock().unwrap();
        db.insert(&sample_result("target.com", true)).unwrap();
        db.insert(&sample_result("target.com", false)).unwrap();
        db.insert(&sample_result("other.com", true)).unwrap();
    }

    let resp = reqwest::get(format!("{base}/api/history/target.com"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: Vec<CheckResult> = resp.json().await.unwrap();
    assert_eq!(body.len(), 2);
    assert!(body.iter().all(|r| r.host == "target.com"));
}

#[tokio::test]
async fn history_returns_404_for_unknown_host() {
    let (base, _db) = spawn_open_server().await;

    let resp = reqwest::get(format!("{base}/api/history/unknown.com"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// --- sync ---

#[tokio::test]
async fn sync_response_node_id_matches_server() {
    let (base, _db) = spawn_open_server().await;

    let report = PeerReport {
        node_id: "peer-1".to_string(),
        results: vec![],
    };
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/api/sync"))
        .json(&report)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let our_report: PeerReport = resp.json().await.unwrap();
    assert_eq!(our_report.node_id, "node-test");
}

#[tokio::test]
async fn sync_response_contains_local_data() {
    let (base, db) = spawn_open_server().await;

    {
        let db = db.lock().unwrap();
        db.insert(&sample_result("local-host.com", true)).unwrap();
    }

    let report = PeerReport {
        node_id: "peer-1".to_string(),
        results: vec![],
    };
    let client = reqwest::Client::new();
    let our_report: PeerReport = client
        .post(format!("{base}/api/sync"))
        .json(&report)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert!(
        our_report
            .results
            .iter()
            .any(|r| r.host == "local-host.com")
    );
}

#[tokio::test]
async fn sync_stores_peer_results_with_peer_prefix() {
    let (base, db) = spawn_open_server().await;

    // source matches peer node_id → must be stored as "peer:<node_id>"
    let report = PeerReport {
        node_id: "peer-1".to_string(),
        results: vec![result_from("peer-host.com", "peer-1")],
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/api/sync"))
        .json(&report)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let history = db.lock().unwrap().history("peer-host.com", 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].source, "peer:peer-1");
}

#[tokio::test]
async fn sync_drops_results_with_foreign_source() {
    let (base, db) = spawn_open_server().await;

    // source does not match peer node_id → must be filtered out
    let report = PeerReport {
        node_id: "peer-1".to_string(),
        results: vec![result_from("foreign-host.com", "some-other-node")],
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/api/sync"))
        .json(&report)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let history = db.lock().unwrap().history("foreign-host.com", 10).unwrap();
    assert!(history.is_empty());
}

#[tokio::test]
async fn sync_stores_only_matching_results_from_mixed_report() {
    let (base, db) = spawn_open_server().await;

    let report = PeerReport {
        node_id: "peer-1".to_string(),
        results: vec![
            result_from("valid-host.com", "peer-1"),       // accepted
            result_from("foreign-host.com", "other-node"), // dropped
        ],
    };

    let client = reqwest::Client::new();
    client
        .post(format!("{base}/api/sync"))
        .json(&report)
        .send()
        .await
        .unwrap();

    let db = db.lock().unwrap();
    assert_eq!(db.history("valid-host.com", 10).unwrap().len(), 1);
    assert!(db.history("foreign-host.com", 10).unwrap().is_empty());
}

#[tokio::test]
async fn sync_returns_valid_json_with_empty_peer_report() {
    let (base, _db) = spawn_open_server().await;

    let empty_report = PeerReport {
        node_id: "peer-empty".to_string(),
        results: vec![],
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/api/sync"))
        .json(&empty_report)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let report: PeerReport = resp.json().await.unwrap();
    assert_eq!(report.node_id, "node-test");
}

// --- secret header ---

#[tokio::test]
async fn secret_header_required_returns_404_without_it() {
    let (base, _db) = spawn_server(Some("my-secret-token".to_string())).await;

    // No header → 404
    let resp = reqwest::get(format!("{base}/api/status")).await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn secret_header_wrong_value_returns_404() {
    let (base, _db) = spawn_server(Some("correct-token".to_string())).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/api/status"))
        .header("X-Netwatch-Token", "wrong-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn secret_header_correct_value_passes() {
    let (base, db) = spawn_server(Some("correct-token".to_string())).await;

    {
        let db = db.lock().unwrap();
        db.insert(&sample_result("guarded.com", true)).unwrap();
    }

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/api/status"))
        .header("X-Netwatch-Token", "correct-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: Vec<CheckResult> = resp.json().await.unwrap();
    assert_eq!(body.len(), 1);
    assert_eq!(body[0].host, "guarded.com");
}

#[tokio::test]
async fn secret_header_applies_to_all_endpoints() {
    let (base, _db) = spawn_server(Some("secret".to_string())).await;

    let client = reqwest::Client::new();

    // GET /api/status → 404
    let resp = client
        .get(format!("{base}/api/status"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // GET /api/history/x → 404
    let resp = client
        .get(format!("{base}/api/history/x"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // POST /api/sync → 404
    let resp = client
        .post(format!("{base}/api/sync"))
        .json(&PeerReport {
            node_id: "p".to_string(),
            results: vec![],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}
