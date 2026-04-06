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
    assert_eq!(t, t2);
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
    assert_eq!(cr, cr2);
}

#[test]
fn peer_report_roundtrip() {
    let pr = PeerReport {
        node_id: "node-1".to_string(),
        results: vec![
            CheckResult {
                host: "host1".to_string(),
                ok: false,
                latency_ms: 999,
                timestamp: Utc::now(),
                source: "node-1".to_string(),
            },
            CheckResult {
                host: "host2".to_string(),
                ok: true,
                latency_ms: 12,
                timestamp: Utc::now(),
                source: "node-1".to_string(),
            },
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
        results: vec![CheckResult {
            host: "host3".to_string(),
            ok: true,
            latency_ms: 5,
            timestamp: Utc::now(),
            source: "node-2".to_string(),
        }],
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
