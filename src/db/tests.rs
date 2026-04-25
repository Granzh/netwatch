use super::*;
use chrono::Utc;

fn make_result(host: &str, ok: bool, latency_ms: u32) -> CheckResult {
    CheckResult {
        host: host.to_string(),
        ok,
        latency_ms,
        timestamp: Utc::now(),
        source: "test".to_string(),
    }
}

fn make_result_by_other_node(host: &str, ok: bool, latency_ms: u32) -> CheckResult {
    CheckResult {
        host: host.to_string(),
        ok,
        latency_ms,
        timestamp: Utc::now(),
        source: "peer:test".to_string(),
    }
}

#[test]
fn open_in_memory_and_migrate() {
    Db::open_in_memory().expect("should open");
}

#[test]
fn insert_and_history() {
    let db = Db::open_in_memory().unwrap();

    db.insert(&make_result("host-a", true, 10)).unwrap();
    db.insert(&make_result("host-a", false, 20)).unwrap();
    db.insert(&make_result("host-a", true, 30)).unwrap();

    let hist = db.history("host-a", 10).unwrap();
    assert_eq!(hist.len(), 3);
    // newest first
    assert_eq!(hist[0].latency_ms, 30);
}

#[test]
fn results_from_same_host_and_different_nodes_not_merged() {
    let db = Db::open_in_memory().unwrap();

    db.insert(&make_result("host-a", true, 10)).unwrap();
    db.insert(&make_result_by_other_node("host-a", true, 11))
        .unwrap();

    let latest_statuses = db.latest_status(1).unwrap();
    assert_eq!(latest_statuses.len(), 2);
    assert_eq!(latest_statuses[0].host, "host-a");
    assert_eq!(latest_statuses[1].host, "host-a");
    let has_node1 = latest_statuses.iter().any(|r| r.source == "test");
    let has_node2 = latest_statuses.iter().any(|r| r.source == "peer:test");
    assert!(has_node1 && has_node2);
}

#[test]
fn history_limit() {
    let db = Db::open_in_memory().unwrap();
    for i in 0..10u32 {
        db.insert(&make_result("host-b", true, i)).unwrap();
    }
    let hist = db.history("host-b", 3).unwrap();
    assert_eq!(hist.len(), 3);
}

#[test]
fn latest_status_returns_one_per_host() {
    let db = Db::open_in_memory().unwrap();

    db.insert(&make_result("host-a", false, 100)).unwrap();
    db.insert(&make_result("host-a", true, 50)).unwrap();
    db.insert(&make_result("host-b", true, 20)).unwrap();

    let status = db.latest_status(1).unwrap();
    assert_eq!(status.len(), 2);

    let a = status.iter().find(|r| r.host == "host-a").unwrap();
    assert!(a.ok);
    assert_eq!(a.latency_ms, 50);
}

#[test]
fn cleanup_removes_old_records() {
    let db = Db::open_in_memory().unwrap();

    // Insert a record with a very old timestamp manually
    let old_ts = Utc::now().timestamp_millis() - 10 * 86_400_000; // 10 days ago
    db.conn
        .execute(
            "INSERT INTO checks (ts, host, ok, latency_ms, source) VALUES (?1, 'old-host', 1, 5, 'test')",
            params![old_ts],
        )
        .unwrap();

    db.insert(&make_result("new-host", true, 1)).unwrap();

    let deleted = db.cleanup(5).unwrap();
    assert_eq!(deleted, 1);

    let remaining = db.history("new-host", 10).unwrap();
    assert_eq!(remaining.len(), 1);

    let gone = db.history("old-host", 10).unwrap();
    assert!(gone.is_empty());
}

#[test]
fn latest_status_excludes_outside_window() {
    let db = Db::open_in_memory().unwrap();

    // Insert a record 2 hours ago — outside a 1-hour window
    let old_ts = Utc::now().timestamp_millis() - 2 * 3_600_000;
    db.conn
        .execute(
            "INSERT INTO checks (ts, host, ok, latency_ms, source) VALUES (?1, 'stale', 1, 5, 'test')",
            params![old_ts],
        )
        .unwrap();

    let status = db.latest_status(1).unwrap();
    assert!(status.is_empty());
}
