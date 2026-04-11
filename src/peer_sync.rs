use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::Rng;
use reqwest::Client;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::config::AppConfig;
use crate::db::Db;
use crate::models::PeerReport;

const JITTER_RANGE: u64 = 10;

fn sync_duration(config: &AppConfig) -> Duration {
    let base = config.sync_interval_seconds;
    let jitter = rand::rng().random_range(0..=JITTER_RANGE * 2);
    let secs = base.saturating_sub(JITTER_RANGE).saturating_add(jitter);
    Duration::from_secs(secs.max(1))
}

pub async fn run(
    config: Arc<arc_swap::ArcSwap<AppConfig>>,
    client: Client,
    db: Arc<Mutex<Db>>,
    cancel: CancellationToken,
) {
    loop {
        let cfg = config.load();

        if !cfg.peers.is_empty() {
            let our_report = build_local_report(&cfg.node_id, &db);
            sync_with_peers(&client, &cfg, &db, &our_report).await;
        }

        let sleep_dur = sync_duration(&cfg);

        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(sleep_dur) => {}
        }
    }
}

fn build_local_report(node_id: &str, db: &Arc<Mutex<Db>>) -> PeerReport {
    let results = db
        .lock()
        .ok()
        .and_then(|db| db.latest_status(1).ok())
        .unwrap_or_default();

    PeerReport {
        node_id: node_id.to_string(),
        results,
    }
}

struct PeerResults {
    results: Vec<crate::models::CheckResult>,
}

async fn sync_with_peers(
    client: &Client,
    config: &AppConfig,
    db: &Arc<Mutex<Db>>,
    our_report: &PeerReport,
) {
    let mut set = JoinSet::new();

    for peer_url in &config.peers {
        let client = client.clone();
        let url = format!("{peer_url}/api/sync");
        let report = our_report.clone();
        let secret = config.api_secret.clone();

        set.spawn(async move {
            let mut req = client.post(&url).json(&report);
            if let Some(secret) = &secret {
                req = req.header("X-Netwatch-Token", secret);
            }

            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    log::warn!("peer sync failed for {url}: {e}");
                    return None;
                }
            };

            if !resp.status().is_success() {
                log::warn!("peer sync {url} returned status {}", resp.status());
                return None;
            }

            let peer_report: PeerReport = match resp.json().await {
                Ok(r) => r,
                Err(e) => {
                    log::warn!("peer sync {url} invalid response: {e}");
                    return None;
                }
            };

            let source = format!("peer:{}", peer_report.node_id);
            let results = peer_report
                .results
                .into_iter()
                .map(|mut r| {
                    r.source = source.clone();
                    r
                })
                .collect();

            Some(PeerResults { results })
        });
    }

    let mut all_results = Vec::new();
    while let Some(join_result) = set.join_next().await {
        match join_result {
            Ok(Some(peer)) => all_results.extend(peer.results),
            Ok(None) => {} // already logged inside task
            Err(e) => log::error!("peer sync task panicked: {e}"),
        }
    }

    if !all_results.is_empty()
        && let Ok(db) = db.lock()
    {
        for result in &all_results {
            if let Err(e) = db.insert(result) {
                log::error!("db insert from peer sync failed: {e}");
            }
        }
    }
}
