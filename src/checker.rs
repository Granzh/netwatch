use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use rand::Rng;
use reqwest::{Client, StatusCode, redirect};
use thiserror::Error;
use tokio::time::Instant;

use crate::config::AppConfig;
use crate::models::{CheckResult, Target};

const USER_AGENTS: &[&str] = &[
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_4_1) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4.1 Safari/605.1.15",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:125.0) Gecko/20100101 Firefox/125.0",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_4_1) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36 Edg/124.0.0.0",
];

#[derive(Error, Debug)]
pub enum CheckerError {
    #[error("Failed to build HTTP client: {0}")]
    ClientBuild(#[from] reqwest::Error),
}

pub struct Checker {
    client: Arc<Client>,
    source: String,
}

pub fn build_client(config: &AppConfig) -> Result<Client, CheckerError> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(config.request_timeout_secs))
        .connect_timeout(Duration::from_secs(5))
        .pool_max_idle_per_host(10)
        .tcp_nodelay(true)
        .danger_accept_invalid_certs(config.danger_accept_invalid_certs);

    if config.follow_redirects {
        builder = builder.redirect(redirect::Policy::limited(10));
    } else {
        builder = builder.redirect(redirect::Policy::none());
    }

    Ok(builder.build()?)
}

impl Checker {
    pub fn new(client: Arc<Client>, source: impl Into<String>) -> Self {
        Self {
            client,
            source: source.into(),
        }
    }

    fn random_user_agent() -> &'static str {
        let idx = rand::rng().random_range(0..USER_AGENTS.len());
        USER_AGENTS[idx]
    }

    pub async fn check(&self, target: &Target) -> CheckResult {
        let ua = Self::random_user_agent();
        let start = Instant::now();

        let result = self.try_head_then_get(&target.url, ua).await;

        let latency_ms = start.elapsed().as_millis().min(u32::MAX as u128) as u32;

        let ok = match &result {
            Ok(status) => status.is_success() || status.is_redirection(),
            Err(_) => false,
        };

        CheckResult {
            host: target.url.clone(),
            ok,
            latency_ms,
            timestamp: Utc::now(),
            source: self.source.clone(),
        }
    }

    async fn try_head_then_get(&self, url: &str, ua: &str) -> Result<StatusCode, reqwest::Error> {
        let head_result = self.client.head(url).header("User-Agent", ua).send().await;

        match head_result {
            Ok(resp) if resp.status() != StatusCode::METHOD_NOT_ALLOWED => Ok(resp.status()),
            // Timeout - don't retry with GET, the host is slow/unreachable
            Err(e) if e.is_timeout() || e.is_connect() => Err(e),
            _ => {
                let resp = self
                    .client
                    .get(url)
                    .header("User-Agent", ua)
                    .header("Range", "bytes=0-0")
                    .send()
                    .await?;
                Ok(resp.status())
            }
        }
    }
}
