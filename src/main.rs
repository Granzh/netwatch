use netwatch::config::{ConfigError, parse_listen_addr};
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Parser, Subcommand};
use tabled::settings::Style;
use tabled::{Table, Tabled};
use tokio_util::sync::CancellationToken;

use netwatch::api::{AppState, router};
use netwatch::checker::{Checker, build_client};
use netwatch::config::AppConfig;
use netwatch::db::Db;
use netwatch::update::{
    UpdateStatus, check_update, download_to, find_checksum_url, parse_expected_checksum,
    select_asset,
};
use netwatch::watcher::ConfigStore;
use netwatch::{peer_sync, scheduler};

const CONFIG_PATH: &str = "/etc/netwatch/config.toml";
const DB_PATH: &str = "/var/lib/netwatch/netwatch.db";

#[derive(Parser)]
#[command(name = "netwatch", about = "Network availability monitor", version)]
struct Cli {
    #[arg(long, default_value = CONFIG_PATH, global = true, help = "Config file path")]
    config: PathBuf,
    #[arg(long, default_value = DB_PATH, global = true, help = "Database file path")]
    db: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the scheduler, API server, and peer sync
    Run,
    /// Show the latest check status for all monitored hosts
    Status,
    /// Show check history for a specific host
    History {
        host: String,
        #[arg(
            short = 'n',
            long,
            default_value_t = 20,
            help = "Number of records to show"
        )]
        limit: u32,
    },
    /// Add a URL to monitored sources
    Add {
        /// URL to monitor
        url: String,
    },
    /// Remove a monitored source by URL
    Remove {
        /// URL of the source to remove
        url: String,
    },
    /// Add a peer node
    AddPeer {
        /// Peer base URL (e.g. http://10.0.0.2:8080)
        url: String,
    },
    /// Remove a peer node
    RemovePeer {
        /// Peer base URL to remove
        url: String,
    },
    /// Show the current configuration
    List,
    /// Create a default config file, prompting for key settings
    Init {
        /// Write defaults without any prompts
        #[arg(long)]
        defaults: bool,
    },
    /// Update the netwatch binary to the latest (or a specific) release
    Update {
        /// Only check if an update is available; don't install (exits 1 if update found)
        #[arg(long)]
        check: bool,
        /// Install a specific release version (e.g. v0.1.5)
        #[arg(long)]
        version: Option<String>,
    },
}

#[derive(Tabled)]
struct StatusRow {
    #[tabled(rename = "Host")]
    host: String,
    #[tabled(rename = "Status")]
    status: String,
    #[tabled(rename = "Latency ms")]
    latency_ms: u32,
    #[tabled(rename = "Last seen (UTC)")]
    timestamp: String,
    #[tabled(rename = "Source")]
    source: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Run => cmd_run(&cli.config, &cli.db).await?,
        Command::Status => cmd_status(&cli.db)?,
        Command::History { host, limit } => cmd_history(&cli.db, &host, limit)?,
        Command::Add { url } => cmd_add(&cli.config, &url)?,
        Command::Remove { url } => cmd_remove(&cli.config, &url)?,
        Command::AddPeer { url } => cmd_add_peer(&cli.config, &url)?,
        Command::RemovePeer { url } => cmd_remove_peer(&cli.config, &url)?,
        Command::List => cmd_list(&cli.config)?,
        Command::Init { defaults } => cmd_init(&cli.config, defaults)?,
        Command::Update { check, version } => std::process::exit(cmd_update(check, version).await?),
    }

    Ok(())
}

async fn cmd_run(config_path: &Path, db_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .init();

    let store = ConfigStore::new(config_path, Duration::from_millis(300))?;
    let config_arc = store.arc();
    let cfg = store.get();

    let db = Arc::new(Mutex::new(Db::open(db_path)?));
    let client = build_client(&cfg)?;
    let sync_client = client.clone();
    let checker = Arc::new(Checker::new(Arc::new(client), cfg.node_id.clone()));

    let state = AppState {
        node_id: cfg.node_id.clone(),
        db: Arc::clone(&db),
        api_secret: cfg.api_secret.clone(),
    };

    let ip = parse_listen_addr(&cfg.http_api)?;
    let addr = SocketAddr::from((ip, cfg.listen_port));
    drop(cfg);

    let cancel = CancellationToken::new();

    let listener = tokio::net::TcpListener::bind(addr).await?;
    log::info!("Listening on {addr}");
    let server_cancel = cancel.clone();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router(state))
            .with_graceful_shutdown(async move { server_cancel.cancelled().await })
            .await
    });

    let sched_cancel = cancel.clone();
    let sched_handle = tokio::spawn(scheduler::run(
        Arc::clone(&config_arc),
        checker,
        Arc::clone(&db),
        sched_cancel,
    ));

    let sync_cancel = cancel.clone();
    let sync_handle = tokio::spawn(peer_sync::run(
        Arc::clone(&config_arc),
        sync_client,
        Arc::clone(&db),
        sync_cancel,
    ));

    tokio::signal::ctrl_c().await?;
    log::info!("Shutting down...");
    cancel.cancel();

    let (server_result, sched_result, sync_result) =
        tokio::join!(server_handle, sched_handle, sync_handle);
    match server_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log::error!("server exited with error: {e}"),
        Err(e) => log::error!("server task panicked: {e}"),
    }
    if let Err(e) = sched_result {
        log::error!("scheduler task panicked: {e}");
    }
    if let Err(e) = sync_result {
        log::error!("peer sync task panicked: {e}");
    }

    Ok(())
}

fn cmd_status(db_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let db = Db::open(db_path)?;
    let results = db.latest_status(24)?;

    if results.is_empty() {
        println!("No data. Run `netwatch run` first.");
        return Ok(());
    }

    let rows: Vec<StatusRow> = results.iter().map(result_to_row).collect();
    println!("{}", Table::new(rows).with(Style::sharp()));
    Ok(())
}

fn cmd_history(db_path: &Path, host: &str, limit: u32) -> Result<(), Box<dyn std::error::Error>> {
    let db = Db::open(db_path)?;
    let results = db.history(host, limit)?;

    if results.is_empty() {
        println!("No history for '{host}'.");
        return Ok(());
    }

    let rows: Vec<StatusRow> = results.iter().map(result_to_row).collect();
    println!("{}", Table::new(rows).with(Style::sharp()));
    Ok(())
}

fn result_to_row(r: &netwatch::models::CheckResult) -> StatusRow {
    StatusRow {
        host: r.host.clone(),
        status: if r.ok {
            "UP".to_string()
        } else {
            "DOWN".to_string()
        },
        latency_ms: r.latency_ms,
        timestamp: r.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
        source: r.source.clone(),
    }
}

fn load_config_or_default(path: &Path) -> Result<AppConfig, Box<dyn std::error::Error>> {
    match AppConfig::load(path) {
        Ok(c) => Ok(c),
        Err(ConfigError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(AppConfig::default())
        }
        Err(e) => Err(e.into()),
    }
}

fn cmd_add(config_path: &Path, url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = load_config_or_default(config_path)?;
    if config.sources.iter().any(|s| s == url) {
        println!("Already monitored: {url}");
        return Ok(());
    }
    config.sources.push(url.to_string());
    config.save(config_path)?;
    println!("Added: {url}");
    Ok(())
}

fn cmd_remove(config_path: &Path, url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = match AppConfig::load(config_path) {
        Ok(c) => c,
        Err(ConfigError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("Config file not found; nothing to remove.");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    let before = config.sources.len();
    config.sources.retain(|s| s != url);
    if config.sources.len() == before {
        println!("Not found: {url}");
        return Ok(());
    }
    config.save(config_path)?;
    println!("Removed: {url}");
    Ok(())
}

fn cmd_add_peer(config_path: &Path, url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = load_config_or_default(config_path)?;
    if config.peers.iter().any(|p| p == url) {
        println!("Peer already present: {url}");
        return Ok(());
    }
    config.peers.push(url.to_string());
    config.save(config_path)?;
    println!("Added peer: {url}");
    Ok(())
}

fn cmd_remove_peer(config_path: &Path, url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = match AppConfig::load(config_path) {
        Ok(c) => c,
        Err(ConfigError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("Config file not found: {}", config_path.display());
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    let before = config.peers.len();
    config.peers.retain(|p| p != url);
    if config.peers.len() == before {
        println!("Peer not found: {url}");
        return Ok(());
    }
    config.save(config_path)?;
    println!("Removed peer: {url}");
    Ok(())
}

fn cmd_list(config_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let config = match AppConfig::load(config_path) {
        Ok(c) => c,
        Err(ConfigError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "(config file '{}' not found — showing defaults; run `netwatch init` to create one)",
                config_path.display()
            );
            AppConfig::default()
        }
        Err(e) => return Err(e.into()),
    };
    let addr = SocketAddr::from((parse_listen_addr(&config.http_api)?, config.listen_port));

    println!("Node ID:           {}", config.node_id);
    println!("Listen address:    {addr}");
    println!(
        "Check interval:    {}s (±{}s jitter)",
        config.check_interval_seconds, config.check_jitter_seconds
    );
    println!("Sync interval:     {}s", config.sync_interval_seconds);
    println!("Sync timeout:      {}s", config.sync_timeout_secs);
    println!("Latency threshold: {}ms", config.latency_threshold_ms);
    println!("Log check results: {}", config.log_check_results);

    println!("\nSources ({}):", config.sources.len());
    if config.sources.is_empty() {
        println!("  (none)");
    } else {
        for s in &config.sources {
            println!("  {s}");
        }
    }

    println!("\nPeers ({}):", config.peers.len());
    if config.peers.is_empty() {
        println!("  (none)");
    } else {
        for p in &config.peers {
            println!("  {p}");
        }
    }

    Ok(())
}

fn prompt(label: &str, default: &str) -> io::Result<String> {
    if default.is_empty() {
        print!("{label}: ");
    } else {
        print!("{label} [{}]: ", default);
    }
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed)
    }
}

fn cmd_init(config_path: &Path, defaults: bool) -> Result<(), Box<dyn std::error::Error>> {
    if config_path.exists() {
        println!(
            "Config already exists at '{}', skipping.",
            config_path.display()
        );
        return Ok(());
    }

    let mut config = AppConfig::default();

    if !defaults {
        println!("Initialising netwatch. Press Enter to accept each default.");
        println!();

        let port_str = prompt("Listen port", &config.listen_port.to_string())?;
        if port_str.trim().is_empty() {
            // Keep the default listen port.
        } else if let Ok(p) = port_str.parse::<u16>() {
            config.set_port(p);
        } else {
            eprintln!(
                "Invalid listen port '{}'; using default {}.",
                port_str, config.listen_port
            );
        }

        loop {
            let bind = prompt("HTTP API bind address", &config.http_api)?;
            match parse_listen_addr(&bind) {
                Ok(_) => {
                    config.http_api = bind;
                    break;
                }
                Err(_) => println!("  '{}' is not a valid IP address, please try again.", bind),
            }
        }

        let peers_str = prompt(
            "Peer node URLs (space or comma separated, or Enter for none)",
            "",
        )?;
        config.peers = peers_str
            .split([',', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();

        println!();
    }

    if let Some(parent) = config_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    config.save(config_path)?;
    println!("Config written to '{}'.", config_path.display());
    println!("Edit it to customise sources, latency threshold, and other settings.");
    Ok(())
}

// ── self-update ────────────────────────────────────────────────────────────────

const GITHUB_API: &str = "https://api.github.com";
const GITHUB_REPO: &str = "Granzh/netwatch";
const UPDATE_TARGET: &str = if cfg!(all(
    target_arch = "x86_64",
    target_os = "linux",
    target_env = "musl"
)) {
    "x86_64-unknown-linux-musl"
} else if cfg!(all(
    target_arch = "x86_64",
    target_os = "linux",
    target_env = "gnu"
)) {
    "x86_64-unknown-linux-gnu"
} else if cfg!(all(
    target_arch = "aarch64",
    target_os = "linux",
    target_env = "musl"
)) {
    "aarch64-unknown-linux-musl"
} else if cfg!(all(
    target_arch = "aarch64",
    target_os = "linux",
    target_env = "gnu"
)) {
    "aarch64-unknown-linux-gnu"
} else if cfg!(all(target_arch = "x86_64", target_os = "macos")) {
    "x86_64-apple-darwin"
} else if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
    "aarch64-apple-darwin"
} else {
    "unsupported-target"
};

fn verify_checksum(archive: &Path, sums_file: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let archive_filename = archive
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("archive has no filename")?;
    let sums = std::fs::read_to_string(sums_file)?;

    let expected = parse_expected_checksum(&sums, archive_filename)
        .ok_or("checksum not found in SHA256SUMS")?;

    let output = std::process::Command::new("sha256sum")
        .arg(archive)
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "sha256sum failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let computed = String::from_utf8_lossy(&output.stdout);
    let computed_hash = computed
        .split_whitespace()
        .next()
        .ok_or("sha256sum produced no output")?;

    if computed_hash != expected {
        return Err(format!("checksum mismatch: expected {expected}, got {computed_hash}").into());
    }

    println!("Checksum verified.");
    Ok(())
}

/// Returns exit code: 0 = success / up-to-date, 1 = update available (--check only).
async fn cmd_update(
    check_only: bool,
    pin_version: Option<String>,
) -> Result<i32, Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .user_agent(format!("netwatch/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()?;

    let current = env!("CARGO_PKG_VERSION");
    let pin = pin_version.as_deref();

    print!(
        "Fetching release info for '{}'... ",
        pin.unwrap_or("latest")
    );
    io::stdout().flush()?;

    let status = check_update(&client, GITHUB_API, GITHUB_REPO, current, pin).await?;

    match status {
        UpdateStatus::UpToDate => {
            println!("Already up to date (v{current}).");
            Ok(0)
        }
        UpdateStatus::Available {
            tag: remote_tag,
            changelog_url,
            assets,
        } => {
            println!("{remote_tag}");

            if let Some(url) = &changelog_url {
                println!("Changelog: {url}");
            }

            if check_only {
                println!("Update available: v{current} → {remote_tag}");
                println!("Run `netwatch update` (as root) to install.");
                return Ok(1);
            }

            // Select the installable archive asset for this platform.
            let asset = select_asset(&assets, UPDATE_TARGET)
                .ok_or_else(|| format!("no asset found for target '{UPDATE_TARGET}'"))?;

            let asset_url = asset["browser_download_url"]
                .as_str()
                .ok_or("asset missing browser_download_url")?;
            let asset_name = asset["name"].as_str().unwrap_or("netwatch.tar.gz");

            // All temp files live in a private randomly-named directory so
            // predictable paths in /tmp cannot be pre-created or symlinked.
            let tmp_dir = tempfile::TempDir::new()?;
            let archive_path = tmp_dir.path().join(asset_name);

            println!("Downloading {asset_name}...");
            download_to(&client, asset_url, &archive_path).await?;

            // Verify checksum when a SHA256SUMS asset is available.
            if let Some(checksum_url) = find_checksum_url(&assets, asset_name) {
                let sums_path = tmp_dir.path().join("SHA256SUMS");
                download_to(&client, &checksum_url, &sums_path).await?;
                verify_checksum(&archive_path, &sums_path)?;
            }

            // Extract binary. Root-layout (./netwatch) is tried first;
            // subdirectory layout (name/netwatch) is the fallback.
            // For .tar.gz assets, extraction failure is always an error.
            let extracted = tmp_dir.path().join("netwatch");
            let is_archive = asset_name.ends_with(".tar.gz") || asset_name.ends_with(".tgz");

            if is_archive {
                let root_ok = std::process::Command::new("tar")
                    .arg("-xzf")
                    .arg(&archive_path)
                    .arg("-C")
                    .arg(tmp_dir.path())
                    .arg("--no-same-owner")
                    .arg("--no-same-permissions")
                    .arg("netwatch")
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
                    && extracted.exists();

                if !root_ok {
                    let sub_ok = std::process::Command::new("tar")
                        .arg("-xzf")
                        .arg(&archive_path)
                        .arg("-C")
                        .arg(tmp_dir.path())
                        .arg("--no-same-owner")
                        .arg("--no-same-permissions")
                        .arg("--wildcards")
                        .arg("--strip-components=1")
                        .arg("*/netwatch")
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false)
                        && extracted.exists();

                    if !sub_ok {
                        return Err(format!(
                            "failed to extract netwatch binary from '{asset_name}'"
                        )
                        .into());
                    }
                }
            } else {
                std::fs::rename(&archive_path, &extracted)?;
            }

            // Reject symlinks and non-regular files before copying.
            {
                let meta = std::fs::symlink_metadata(&extracted)?;
                if !meta.file_type().is_file() {
                    return Err(format!(
                        "extracted '{}' is not a regular file (symlink or special file rejected)",
                        extracted.display()
                    )
                    .into());
                }
            }

            // Stage in the same directory as current_exe so rename is atomic.
            let current_exe = std::env::current_exe()?;
            let install_dir = current_exe
                .parent()
                .ok_or("cannot determine install directory")?;
            let staging = install_dir.join(".netwatch.new");

            std::fs::copy(&extracted, &staging)?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&staging)?.permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&staging, perms)?;
            }

            std::fs::rename(&staging, &current_exe)?;

            println!("Updated to {remote_tag} at {}.", current_exe.display());
            println!("Run: sudo systemctl restart netwatch");
            Ok(0)
        }
    }
}
