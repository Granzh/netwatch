use crate::config::AppConfig;
use arc_swap::ArcSwap;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::atomic::Ordering;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicU64},
    time::{Duration, Instant},
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum WatcherError {
    #[error("Watcher error: {0}")]
    Notify(#[from] notify::Error),
}

/// Returns `true` if the event should be suppressed (debounced).
/// `prev_ns` is the timestamp of the last accepted event (`u64::MAX` means no prior event).
pub fn should_debounce(prev_ns: u64, now_ns: u64, debounce_ns: u64) -> bool {
    prev_ns != u64::MAX && now_ns.saturating_sub(prev_ns) < debounce_ns
}

pub struct ConfigStore {
    inner: Arc<ArcSwap<AppConfig>>,
    _watcher: RecommendedWatcher,
}

impl ConfigStore {
    pub fn new(path: impl AsRef<Path>, debounce: Duration) -> Result<Self, WatcherError> {
        let path = path.as_ref().to_path_buf();
        let initial = AppConfig::load_or_default(&path);
        let inner = Arc::new(ArcSwap::new(Arc::new(initial)));
        let watcher = spawn_watcher(path, Arc::clone(&inner), debounce)?;
        Ok(Self {
            inner,
            _watcher: watcher,
        })
    }

    #[inline]
    pub fn get(&self) -> arc_swap::Guard<Arc<AppConfig>> {
        self.inner.load()
    }

    pub fn arc(&self) -> Arc<ArcSwap<AppConfig>> {
        Arc::clone(&self.inner)
    }
}

fn spawn_watcher(
    path: PathBuf,
    store: Arc<ArcSwap<AppConfig>>,
    debounce: Duration,
) -> Result<RecommendedWatcher, WatcherError> {
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();

    let epoch = Instant::now();
    let last_event_w = Arc::new(AtomicU64::new(u64::MAX));

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res
            && matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_))
            && event.paths.iter().any(|event_path| event_path == &path)
        {
            let now = epoch.elapsed().as_nanos() as u64;
            let prev = last_event_w.load(Ordering::Relaxed);

            if should_debounce(prev, now, debounce.as_nanos() as u64) {
                return;
            }

            last_event_w.store(now, Ordering::Relaxed);

            if let Ok(new_cfg) = AppConfig::load(&path) {
                store.store(Arc::new(new_cfg));
            }
        }
    })?;

    watcher.watch(&dir, RecursiveMode::NonRecursive)?;

    log::info!("[config] Watching {dir:?}");
    Ok(watcher)
}
