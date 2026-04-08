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
