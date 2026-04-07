use crate::config::AppConfig;
use arc_swap::ArcSwap;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicU64},
    time::Duration,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum WatcherError {
    #[error("Watcher error: {0}")]
    Notify(#[from] notify::Error),
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

    let last_event_w = Arc::new(AtomicU64::new(0));

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res
            && matches!(event.kind, EventKind::Modify(_))
        {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;

            let prev = last_event_w.load(Ordering::Relaxed);

            if now.saturating_sub(prev) < debounce.as_millis() as u64 {
                last_event_w.store(now, Ordering::Relaxed);
                return;
            }

            last_event_w.store(now, Ordering::Relaxed);

            if let Ok(new_cfg) = AppConfig::load(&path) {
                store.store(Arc::new(new_cfg));
            }
        }
    })?;

    watcher.watch(&dir, RecursiveMode::NonRecursive)?;

    eprintln!("[config] Watching {dir:?}");
    Ok(watcher)
}
