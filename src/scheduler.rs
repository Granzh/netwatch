use std::sync::Arc;
use std::time::Duration;

use rand::Rng;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::checker::Checker;
use crate::config::AppConfig;
use crate::db::Db;
use crate::models::Target;

pub fn targets_from_config(config: &AppConfig) -> Vec<Target> {
    config
        .sources
        .iter()
        .map(|url| Target {
            name: url.clone(),
            url: url.clone(),
            is_peer: false,
        })
        .collect()
}

pub fn jitter_duration(config: &AppConfig) -> Duration {
    let base = config.check_interval_seconds;
    let jitter = config.check_jitter_seconds;
    let secs = if jitter == 0 {
        base
    } else {
        base + rand::rng().random_range(0..=jitter)
    };
    Duration::from_secs(secs)
}

pub async fn run(
    config: Arc<arc_swap::ArcSwap<AppConfig>>,
    checker: Arc<Checker>,
    db: Db,
    cancel: CancellationToken,
) {
    loop {
        let cfg = config.load();
        let targets = targets_from_config(&cfg);

        let results = check_all(&checker, &targets).await;

        for result in &results {
            if let Err(e) = db.insert(result) {
                log::error!("db insert failed: {e}");
            }
        }

        let sleep_dur = jitter_duration(&cfg);

        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(sleep_dur) => {}
        }
    }
}

pub async fn check_all(
    checker: &Arc<Checker>,
    targets: &[Target],
) -> Vec<crate::models::CheckResult> {
    let mut set = JoinSet::new();

    for target in targets {
        let checker = Arc::clone(checker);
        let target = target.clone();
        set.spawn(async move { checker.check(&target).await });
    }

    let mut results = Vec::with_capacity(targets.len());
    while let Some(res) = set.join_next().await {
        if let Ok(check_result) = res {
            results.push(check_result);
        }
    }
    results
}
