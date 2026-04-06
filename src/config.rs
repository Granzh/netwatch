use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to parse config file: {0}")]
    Parse(#[from] toml::de::Error),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AppConfig {
    pub sources: Vec<String>,
    pub latency_threshold_ms: u32,
    pub check_interval_seconds: u64,
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
        }
    }
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let config = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn load_or_default(path: impl AsRef<Path>) -> Self {
        Self::load(path).unwrap_or_else(|_| Self::default())
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        let content = toml::to_string_pretty(self).expect("Failed to serialize config");
        std::fs::write(path, content)?;
        Ok(())
    }
}
