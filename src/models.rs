use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub name: String,
    pub url: String,
    pub is_peer: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub host: String,
    pub ok: bool,
    pub latency_ms: u32,
    pub timestamp: DateTime<Utc>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerReport {
    pub node_id: String,
    pub results: Vec<CheckResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStatus {
    pub node_id: String,
    pub last_seen: DateTime<Utc>,
    pub results: Vec<CheckResult>,
}
