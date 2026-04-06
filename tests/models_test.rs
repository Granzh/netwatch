use chrono::Utc;
use netwatch::models::{CheckResult, NodeStatus, PeerReport, Target};

#[test]
fn target_roundtrip() {
    let t = Target {
        name: "example".to_string(),
        url: "http://example.com".to_string(),
        is_peer: true,
    };
    let json = serde_json::to_string(&t).unwrap();
    let t2: Target = serde_json::from_str(&json).unwrap();
    assert_eq!(t.name, t2.name);
    assert_eq!(t.url, t2.url);
    assert_eq!(t.is_peer, t2.is_peer);
}

#[test]
fn check_result_roundtrip() {
    let cr = CheckResult {
        host: "host1".to_string(),
        ok: true,
        latency_ms: 42,
        timestamp: Utc::now(),
        source: "node-a".to_string(),
    };
    let json = serde_json::to_string(&cr).unwrap();
    let cr2: CheckResult = serde_json::from_str(&json).unwrap();
    assert_eq!(cr.host, cr2.host);
    assert_eq!(cr.ok, cr2.ok);
    assert_eq!(cr.latency_ms, cr2.latency_ms);
    assert_eq!(cr.timestamp, cr2.timestamp);
    assert_eq!(cr.source, cr2.source);
}

#[test]
fn peer_report_roundtrip() {
    let pr = PeerReport {
        node_id: "node-1".to_string(),
        results: vec![CheckResult {
            host: "host1".to_string(),
            ok: false,
            latency_ms: 999,
            timestamp: Utc::now(),
            source: "node-1".to_string(),
        }],
    };
    let json = serde_json::to_string(&pr).unwrap();
    let pr2: PeerReport = serde_json::from_str(&json).unwrap();
    assert_eq!(pr.node_id, pr2.node_id);
    assert_eq!(pr.results.len(), pr2.results.len());
}

#[test]
fn node_status_roundtrip() {
    let ns = NodeStatus {
        node_id: "node-2".to_string(),
        last_seen: Utc::now(),
        results: vec![],
    };
    let json = serde_json::to_string(&ns).unwrap();
    let ns2: NodeStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(ns.node_id, ns2.node_id);
    assert_eq!(ns.last_seen, ns2.last_seen);
}
