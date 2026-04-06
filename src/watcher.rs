use crate::config::{AppConfig, ConfigError};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{RwLock, mpsc};

pub struct ConfigStore {
    inner: Arc<RwLock<AppConfig>>,
    _watcher: RecommendedWatcher,
}

impl ConfigStore {
    pub fn new(path: impl AsRef<Path>, debounce: Duration) -> Result<Self, ConfigError> {
        let path = path.as_ref().to_path_buf();
        let initial = AppConfig::load_or_default(&path);
        let inner = Arc::new(RwLock::new(initial));
        let watcher = spawn_watcher(path, Arc::clone(&inner), debounce)?;
        Ok(Self {
            inner,
            _watcher: watcher,
        })
    }

    pub async fn get(&self) -> AppConfig {
        self.inner.read().await.clone()
    }

    pub fn arc(&self) -> Arc<RwLock<AppConfig>> {
        Arc::clone(&self.inner)
    }
}

fn spawn_watcher(
    path: PathBuf,
    store: Arc<RwLock<AppConfig>>,
    debounce: Duration,
) -> Result<RecommendedWatcher, ConfigError> {
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();

    let (tx, mut rx) = mpsc::unbounded_channel::<()>();

    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            tokio::time::sleep(debounce).await;
            while rx.try_recv().is_ok() {}
            reload_config(&path, &store).await;
        }
        eprintln!("[config] Watcher task stopped");
    });

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res
            && matches!(event.kind, EventKind::Modify(_))
        {
            let _ = tx.send(());
        }
    })?;

    watcher.watch(&dir, RecursiveMode::NonRecursive)?;

    eprintln!("[config] Watching {dir:?}");
    Ok(watcher)
}

async fn reload_config(path: &Path, store: &Arc<RwLock<AppConfig>>) {
    match AppConfig::load(path) {
        Ok(new_config) => {
            let mut guard = store.write().await;
            if *guard != new_config {
                *guard = new_config;
                eprintln!("[config] Config reloaded: {path:?}");
            }
        }
        Err(e) => eprintln!("[config] Failed to reload config: {e}"),
    }
}
