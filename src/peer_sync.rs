use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::Rng;
use reqwest::{Client, Url};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::config::AppConfig;
use crate::db::Db;
use crate::models::PeerReport;

const JITTER_RANGE: u64 = 10;

fn sync_duration(config: &AppConfig) -> Duration {
    let base = config.sync_interval_seconds;
    let jitter_range = JITTER_RANGE.min(base);
    let jitter = rand::rng().random_range(0..=jitter_range * 2);
    let secs = base.saturating_sub(jitter_range).saturating_add(jitter);
    Duration::from_secs(secs.max(1))
}

pub fn resolve_sync_url(peer_url: &str) -> Option<Url> {
    let base = Url::parse(peer_url).ok()?;
    base.join("/api/sync").ok()
}

pub async fn run(
    config: Arc<arc_swap::ArcSwap<AppConfig>>,
    client: Client,
    db: Arc<Mutex<Db>>,
    cancel: CancellationToken,
) {
    loop {
        if cancel.is_cancelled() {
            break;
        }

        let cfg = config.load();

        if !cfg.peers.is_empty() {
            let node_id = cfg.node_id.clone();
            let db_arc = Arc::clone(&db);
            let our_report =
                match tokio::task::spawn_blocking(move || build_local_report(&node_id, &db_arc))
                    .await
                {
                    Ok(report) => report,
                    Err(e) => {
                        log::error!("build_local_report task panicked: {e}");
                        continue;
                    }
                };
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
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.source == node_id)
        .collect();

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
    let semaphore = Arc::new(Semaphore::new(config.max_concurrent_syncs.max(1)));
    let report = Arc::new(our_report.clone());
    let mut set = JoinSet::new();

    for peer_url in &config.peers {
        let url = match resolve_sync_url(peer_url) {
            Some(u) => u,
            None => {
                log::warn!("invalid peer URL: {peer_url}");
                continue;
            }
        };

        let client = client.clone();
        let report = Arc::clone(&report);
        let secret = config.api_secret.clone();
        let permit = Arc::clone(&semaphore);
        let timeout = Duration::from_secs(config.sync_timeout_secs);

        set.spawn(async move {
            let _permit = match permit.acquire().await {
                Ok(p) => p,
                Err(_) => return None,
            };

            let mut req = client.post(url.as_str()).json(&*report).timeout(timeout);
            if let Some(secret) = &secret {
                req = req.header(crate::api::SECRET_HEADER, secret);
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

            let peer_node_id = peer_report.node_id.clone();
            let source = format!("peer:{peer_node_id}");
            let results = peer_report
                .results
                .into_iter()
                .filter(|r| r.source == peer_node_id)
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
            Ok(None) => {}
            Err(e) => log::error!("peer sync task panicked: {e}"),
        }
    }

    if !all_results.is_empty() {
        let db = Arc::clone(db);
        match tokio::task::spawn_blocking(move || match db.lock() {
            Ok(guard) => {
                if let Err(e) = guard.insert_batch(&all_results) {
                    log::error!("db batch insert from peer sync failed: {e}");
                }
            }
            Err(_) => log::error!("db mutex poisoned"),
        })
        .await
        {
            Ok(()) => {}
            Err(e) => log::error!("db insert task panicked: {e}"),
        }
    }
}
