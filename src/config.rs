use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to parse config file: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("Failed to serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AppConfig {
    pub sources: Vec<String>,
    pub latency_threshold_ms: u32,
    pub check_interval_seconds: u64,
    #[serde(default = "default_check_jitter_seconds")]
    pub check_jitter_seconds: u64,
    #[serde(default = "default_max_concurrent_checks")]
    pub max_concurrent_checks: usize,
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    #[serde(default = "default_true")]
    pub follow_redirects: bool,
    #[serde(default)]
    pub danger_accept_invalid_certs: bool,
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,
    #[serde(default)]
    pub api_secret: Option<String>,
    #[serde(default = "default_node_id")]
    pub node_id: String,
    #[serde(default)]
    pub peers: Vec<String>,
    #[serde(default = "default_sync_interval_seconds")]
    pub sync_interval_seconds: u64,
    #[serde(default = "default_max_concurrent_syncs")]
    pub max_concurrent_syncs: usize,
}

fn default_check_jitter_seconds() -> u64 {
    5
}

fn default_max_concurrent_checks() -> usize {
    10
}

fn default_request_timeout_secs() -> u64 {
    10
}

fn default_node_id() -> String {
    let host = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());
    format!("{host}:{}", default_listen_port())
}

fn default_sync_interval_seconds() -> u64 {
    60
}

fn default_max_concurrent_syncs() -> usize {
    5
}

fn default_listen_port() -> u16 {
    8080
}

fn default_true() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            sources: vec![
                "https://www.google.com".to_string(),
                "https://www.claude.ai".to_string(),
                "https://www.spotify.com".to_string(),
                "https://www.telegram.org".to_string(),
            ],
            latency_threshold_ms: 100,
            check_interval_seconds: 60,
            check_jitter_seconds: default_check_jitter_seconds(),
            max_concurrent_checks: default_max_concurrent_checks(),
            request_timeout_secs: default_request_timeout_secs(),
            follow_redirects: true,
            danger_accept_invalid_certs: false,
            listen_port: default_listen_port(),
            api_secret: None,
            node_id: default_node_id(),
            peers: vec![],
            sync_interval_seconds: default_sync_interval_seconds(),
            max_concurrent_syncs: default_max_concurrent_syncs(),
        }
    }
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path)?;
        let config = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn load_or_default(path: impl AsRef<Path>) -> Self {
        let path_ref = path.as_ref();

        Self::load(path_ref).unwrap_or_else(|_| {
            let config = Self::default();
            if let Ok(toml_str) = toml::to_string_pretty(&config) {
                let _ = fs::write(path_ref, toml_str);
            }

            config
        })
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        let content = toml::to_string_pretty(self).expect("Failed to serialize config");
        fs::write(path, content)?;
        Ok(())
    }
}
