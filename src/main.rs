use netwatch::config::{ConfigError, parse_listen_addr};
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::Path;
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
use netwatch::watcher::ConfigStore;
use netwatch::{peer_sync, scheduler};

const CONFIG_PATH: &str = "/etc/netwatch/config.toml";
const DB_PATH: &str = "/var/lib/netwatch/netwatch.db";

#[derive(Parser)]
#[command(name = "netwatch", about = "Network availability monitor", version)]
struct Cli {
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
    let config_path = Path::new(CONFIG_PATH);
    let db_path = Path::new(DB_PATH);

    match cli.command {
        Command::Run => cmd_run(config_path, db_path).await?,
        Command::Status => cmd_status(db_path)?,
        Command::History { host, limit } => cmd_history(db_path, &host, limit)?,
        Command::Add { url } => cmd_add(config_path, &url)?,
        Command::Remove { url } => cmd_remove(config_path, &url)?,
        Command::AddPeer { url } => cmd_add_peer(config_path, &url)?,
        Command::RemovePeer { url } => cmd_remove_peer(config_path, &url)?,
        Command::List => cmd_list(config_path)?,
        Command::Init { defaults } => cmd_init(config_path, defaults)?,
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
                "(config file '{}' not found — showing defaults; use --config or run `netwatch run` first)",
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
        if let Ok(p) = port_str.parse::<u16>() {
            config.set_port(p);
        }

        let bind = prompt("HTTP API bind address", &config.http_api)?;
        config.http_api = bind;

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

    if let Some(parent) = config_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    config.save(config_path)?;
    println!("Config written to '{}'.", config_path.display());
    println!("Edit it to customise sources, latency threshold, and other settings.");
    Ok(())
}
