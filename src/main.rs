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
use netwatch::update::parse_semver;
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

const GITHUB_REPO: &str = "Granzh/netwatch";
const UPDATE_TARGET: &str = if cfg!(all(target_arch = "x86_64", target_os = "linux", target_env = "musl")) {
    "x86_64-unknown-linux-musl"
} else if cfg!(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu")) {
    "x86_64-unknown-linux-gnu"
} else if cfg!(all(target_arch = "aarch64", target_os = "linux", target_env = "musl")) {
    "aarch64-unknown-linux-musl"
} else if cfg!(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu")) {
    "aarch64-unknown-linux-gnu"
} else if cfg!(all(target_arch = "x86_64", target_os = "macos")) {
    "x86_64-apple-darwin"
} else if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
    "aarch64-apple-darwin"
} else if cfg!(all(target_arch = "x86_64", target_os = "windows")) {
    "x86_64-pc-windows-msvc"
} else if cfg!(all(target_arch = "aarch64", target_os = "windows")) {
    "aarch64-pc-windows-msvc"
} else {
    "unsupported-target"
};

async fn fetch_release(
    client: &reqwest::Client,
    tag: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let url = if tag == "latest" {
        format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest")
    } else {
        format!("https://api.github.com/repos/{GITHUB_REPO}/releases/tags/{tag}")
    };
    let release = client
        .get(&url)
        .header(
            "User-Agent",
            format!("netwatch/{}", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    Ok(release)
}

async fn download_to(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::io::AsyncWriteExt;
    let mut resp = client
        .get(url)
        .header(
            "User-Agent",
            format!("netwatch/{}", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await?
        .error_for_status()?;

    let mut file = tokio::fs::File::create(dest).await?;
    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(())
}

fn find_checksum_url(assets: &[serde_json::Value], asset_name: &str) -> Option<String> {
    let candidates = [format!("{asset_name}.sha256"), "SHA256SUMS".to_string()];
    for name in &candidates {
        if let Some(url) = assets
            .iter()
            .find(|a| a["name"].as_str() == Some(name.as_str()))
            .and_then(|a| a["browser_download_url"].as_str())
        {
            return Some(url.to_string());
        }
    }
    None
}

fn verify_checksum(archive: &Path, sums_file: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let archive_name = archive.file_name().ok_or("archive has no filename")?;
    let sums = std::fs::read_to_string(sums_file)?;

    let expected = sums
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let checksum = parts.next()?;
            let filename = parts.next()?.trim_start_matches('*');

            (Path::new(filename).file_name() == Some(archive_name)).then(|| checksum.to_string())
        })
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

    let tag = pin_version.as_deref().unwrap_or("latest");
    print!("Fetching release info for '{tag}'... ");
    io::stdout().flush()?;

    let release = fetch_release(&client, tag).await?;
    let remote_tag = release["tag_name"]
        .as_str()
        .ok_or("GitHub response missing tag_name")?;

    println!("{remote_tag}");

    let current = env!("CARGO_PKG_VERSION");
    let current_parsed = parse_semver(current);
    let remote_parsed = parse_semver(remote_tag);

    let needs_update = match (current_parsed, remote_parsed) {
        (Some(cur), Some(rem)) => rem > cur,
        _ => {
            // Can't compare — treat as needing update when a version is pinned
            pin_version.is_some()
        }
    };

    if !needs_update {
        println!("Already up to date (v{current}).");
        return Ok(0);
    }

    if let Some(url) = release["html_url"].as_str() {
        println!("Changelog: {url}");
    }

    if check_only {
        println!("Update available: v{current} → {remote_tag}");
        println!("Run `netwatch update` (as root) to install.");
        return Ok(1);
    }

    // Find the right asset. Exclude checksum/SHA256SUMS-style artifacts and
    // prefer installable archive formats when multiple assets match.
    let assets = release["assets"].as_array().ok_or("no assets in release")?;
    let asset = assets
        .iter()
        .filter(|a| {
            let Some(name) = a["name"].as_str() else {
                return false;
            };

            if !name.contains(UPDATE_TARGET) {
                return false;
            }

            let lower = name.to_ascii_lowercase();
            !(lower.ends_with(".sha256")
                || lower.ends_with(".sha512")
                || lower == "sha256sums"
                || lower.ends_with(".sha256sums")
                || lower.contains("sha256sums"))
        })
        .min_by_key(|a| {
            let name = a["name"].as_str().unwrap_or_default();
            if name.ends_with(".tar.gz") {
                0
            } else if name.ends_with(".tgz") {
                1
            } else {
                2
            }
        })
        .ok_or_else(|| format!("no asset found for target '{UPDATE_TARGET}'"))?;

    let asset_url = asset["browser_download_url"]
        .as_str()
        .ok_or("asset missing browser_download_url")?;
    let asset_name = asset["name"].as_str().unwrap_or("netwatch.tar.gz");

    // All temp files live in a private randomly-named directory so predictable
    // paths in /tmp cannot be pre-created or symlinked by another process.
    let tmp_dir = tempfile::TempDir::new()?;
    let archive_path = tmp_dir.path().join(asset_name);

    println!("Downloading {asset_name}...");
    download_to(&client, asset_url, &archive_path).await?;

    // Verify checksum if a SHA256SUMS asset is present (also inside private tmp_dir)
    if let Some(checksum_url) = find_checksum_url(assets, asset_name) {
        let sums_path = tmp_dir.path().join("SHA256SUMS");
        download_to(&client, &checksum_url, &sums_path).await?;
        verify_checksum(&archive_path, &sums_path)?;
    }

    // Extract the netwatch binary from the archive.
    // Current release workflow packs at archive root (./netwatch); try that
    // first, then fall back to a subdirectory layout (netwatch-x.y.z/netwatch).
    // For .tar.gz assets, failing to extract is always an error — never treat
    // an archive file as a bare executable.
    let extracted = tmp_dir.path().join("netwatch");
    let is_archive = asset_name.ends_with(".tar.gz") || asset_name.ends_with(".tgz");

    if is_archive {
        // --no-same-owner / --no-same-permissions prevent the archive from
        // injecting unexpected ownership or setuid bits into the temp dir.
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
                return Err(
                    format!("failed to extract netwatch binary from '{asset_name}'").into(),
                );
            }
        }
    } else {
        // Bare binary asset
        std::fs::rename(&archive_path, &extracted)?;
    }

    // Reject symlinks and non-regular files before copying into the install
    // directory — a malicious archive could embed netwatch as a symlink to
    // redirect the subsequent copy/rename to an arbitrary path.
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

    // Stage next to current_exe (same filesystem) so the final rename is atomic.
    // tmp_dir may be on a different filesystem than the install location, so
    // use copy (cross-filesystem safe) then rename within the same directory.
    let current_exe = std::env::current_exe()?;
    let install_dir = current_exe
        .parent()
        .ok_or("cannot determine install directory")?;
    let staging = install_dir.join(".netwatch.new");

    std::fs::copy(&extracted, &staging)?;
    // tmp_dir and all its contents are cleaned up automatically on drop.

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
