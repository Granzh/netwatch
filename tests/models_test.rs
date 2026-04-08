use chrono::Utc;
use netwatch::models::{CheckResult, NodeStatus, PeerReport, Target};

fn make_result(host: &str, ok: bool, latency_ms: u32) -> CheckResult {
    CheckResult {
        host: host.to_string(),
        ok,
        latency_ms,
        timestamp: Utc::now(),
        source: "test-node".to_string(),
    }
}

// --- Roundtrip ---

#[test]
fn target_roundtrip() {
    let t = Target {
        name: "example".to_string(),
        url: "http://example.com".to_string(),
        is_peer: true,
    };
    let json = serde_json::to_string(&t).unwrap();
    let t2: Target = serde_json::from_str(&json).unwrap();
    assert_eq!(t, t2);
}

#[test]
fn check_result_roundtrip() {
    let cr = make_result("host1", true, 42);
    let json = serde_json::to_string(&cr).unwrap();
    let cr2: CheckResult = serde_json::from_str(&json).unwrap();
    assert_eq!(cr, cr2);
}

#[test]
fn peer_report_roundtrip() {
    let pr = PeerReport {
        node_id: "node-1".to_string(),
        results: vec![
            make_result("host1", false, 999),
            make_result("host2", true, 12),
        ],
    };
    let json = serde_json::to_string(&pr).unwrap();
    let pr2: PeerReport = serde_json::from_str(&json).unwrap();
    assert_eq!(pr.node_id, pr2.node_id);
    assert_eq!(pr.results.len(), pr2.results.len());
    for (a, b) in pr.results.iter().zip(pr2.results.iter()) {
        assert_eq!(a.host, b.host);
        assert_eq!(a.ok, b.ok);
        assert_eq!(a.latency_ms, b.latency_ms);
        assert_eq!(a.timestamp, b.timestamp);
        assert_eq!(a.source, b.source);
    }
}

#[test]
fn node_status_roundtrip() {
    let ns = NodeStatus {
        node_id: "node-2".to_string(),
        last_seen: Utc::now(),
        results: vec![make_result("host3", true, 5)],
    };
    let json = serde_json::to_string(&ns).unwrap();
    let ns2: NodeStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(ns.node_id, ns2.node_id);
    assert_eq!(ns.last_seen, ns2.last_seen);
    assert_eq!(ns.results.len(), ns2.results.len());
    let (r, r2) = (&ns.results[0], &ns2.results[0]);
    assert_eq!(r.host, r2.host);
    assert_eq!(r.ok, r2.ok);
    assert_eq!(r.latency_ms, r2.latency_ms);
    assert_eq!(r.timestamp, r2.timestamp);
    assert_eq!(r.source, r2.source);
}

// --- Logic ---

#[test]
fn targets_with_different_fields_are_not_equal() {
    let a = Target {
        name: "a".to_string(),
        url: "http://a.com".to_string(),
        is_peer: false,
    };
    let b = Target {
        name: "b".to_string(),
        url: "http://a.com".to_string(),
        is_peer: false,
    };
    let c = Target {
        name: "a".to_string(),
        url: "http://a.com".to_string(),
        is_peer: true,
    };
    assert_ne!(a, b);
    assert_ne!(a, c);
}

#[test]
fn clone_is_independent() {
    let original = Target {
        name: "original".to_string(),
        url: "http://x.com".to_string(),
        is_peer: false,
    };
    let mut cloned = original.clone();
    cloned.name = "mutated".to_string();
    assert_eq!(original.name, "original");
    assert_eq!(cloned.name, "mutated");
}

#[test]
fn peer_report_count_failures() {
    let pr = PeerReport {
        node_id: "node-1".to_string(),
        results: vec![
            make_result("h1", false, 500),
            make_result("h2", true, 10),
            make_result("h3", false, 800),
            make_result("h4", true, 20),
        ],
    };
    let failures: Vec<_> = pr.results.iter().filter(|r| !r.ok).collect();
    let successes: Vec<_> = pr.results.iter().filter(|r| r.ok).collect();
    assert_eq!(failures.len(), 2);
    assert_eq!(successes.len(), 2);
    assert!(failures.iter().all(|r| r.latency_ms > 100));
}

#[test]
fn peer_report_empty_results_is_valid() {
    let pr = PeerReport {
        node_id: "node-x".to_string(),
        results: vec![],
    };
    let json = serde_json::to_string(&pr).unwrap();
    let pr2: PeerReport = serde_json::from_str(&json).unwrap();
    assert_eq!(pr2.results.len(), 0);
}

#[test]
fn node_status_max_latency() {
    let ns = NodeStatus {
        node_id: "node-3".to_string(),
        last_seen: Utc::now(),
        results: vec![
            make_result("h1", true, 50),
            make_result("h2", true, 200),
            make_result("h3", false, 999),
        ],
    };
    let max = ns.results.iter().map(|r| r.latency_ms).max().unwrap();
    assert_eq!(max, 999);
}

#[test]
fn node_status_last_seen_is_in_the_past() {
    let past = Utc::now() - chrono::Duration::seconds(60);
    let ns = NodeStatus {
        node_id: "node-4".to_string(),
        last_seen: past,
        results: vec![],
    };
    assert!(ns.last_seen < Utc::now());
}
