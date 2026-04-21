use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::Rng;
use tokio::sync::Semaphore;
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
        let jitter_secs = rand::rng().random_range(0..=jitter);
        base.saturating_add(jitter_secs)
    };
    Duration::from_secs(secs.max(1))
}

pub async fn run(
    config: Arc<arc_swap::ArcSwap<AppConfig>>,
    checker: Arc<Checker>,
    db: Arc<Mutex<Db>>,
    cancel: CancellationToken,
) {
    loop {
        if cancel.is_cancelled() {
            break;
        }

        let cfg = config.load();
        let targets = targets_from_config(&cfg);

        let results = check_all(&checker, &targets, cfg.max_concurrent_checks).await;

        if !results.is_empty() {
            let db = Arc::clone(&db);
            match tokio::task::spawn_blocking(move || {
                db.lock()
                    .map_err(|_| "mutex poisoned".to_string())
                    .and_then(|guard| guard.insert_batch(&results).map_err(|e| e.to_string()))
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => log::error!("db batch insert failed: {e}"),
                Err(e) => log::error!("db insert task panicked: {e}"),
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
    max_concurrent: usize,
) -> Vec<crate::models::CheckResult> {
    let semaphore = Arc::new(Semaphore::new(max_concurrent.max(1)));
    let mut set = JoinSet::new();

    for target in targets {
        let checker = Arc::clone(checker);
        let target = target.clone();
        let sem = Arc::clone(&semaphore);
        set.spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            checker.check(&target).await
        });
    }

    let mut results = Vec::with_capacity(targets.len());
    while let Some(res) = set.join_next().await {
        match res {
            Ok(check_result) => results.push(check_result),
            Err(e) => log::error!("check task failed to join: {e}"),
        }
    }
    results
}
