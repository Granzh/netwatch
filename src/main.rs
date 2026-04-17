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
use netwatch::watcher::ConfigStore;
use netwatch::{peer_sync, scheduler};

#[derive(Parser)]
#[command(name = "netwatch", about = "Network availability monitor", version)]
struct Cli {
    #[arg(
        long,
        default_value = "netwatch.toml",
        global = true,
        help = "Config file path"
    )]
    config: PathBuf,
    #[arg(
        long,
        default_value = "netwatch.db",
        global = true,
        help = "Database file path"
    )]
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
        /// Human-readable label (informational only)
        name: String,
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
        Command::Add { name: _, url } => cmd_add(&cli.config, &url)?,
        Command::Remove { url } => cmd_remove(&cli.config, &url)?,
        Command::AddPeer { url } => cmd_add_peer(&cli.config, &url)?,
        Command::RemovePeer { url } => cmd_remove_peer(&cli.config, &url)?,
        Command::List => cmd_list(&cli.config)?,
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

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.listen_port));
    drop(cfg);

    let cancel = CancellationToken::new();

    let listener = tokio::net::TcpListener::bind(addr).await?;
    log::info!("Listening on {addr}");
    let server_cancel = cancel.clone();
    tokio::spawn(async move {
        axum::serve(listener, router(state))
            .with_graceful_shutdown(async move { server_cancel.cancelled().await })
            .await
            .expect("server error");
    });

    let sched_cancel = cancel.clone();
    tokio::spawn(scheduler::run(
        Arc::clone(&config_arc),
        checker,
        Arc::clone(&db),
        sched_cancel,
    ));

    let sync_cancel = cancel.clone();
    tokio::spawn(peer_sync::run(
        Arc::clone(&config_arc),
        sync_client,
        Arc::clone(&db),
        sync_cancel,
    ));

    tokio::signal::ctrl_c().await?;
    log::info!("Shutting down...");
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(500)).await;

    Ok(())
}

fn cmd_status(db_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let db = Db::open(db_path).map_err(|e| format!("Cannot open DB at {db_path:?}: {e}"))?;
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
    let db = Db::open(db_path).map_err(|e| format!("Cannot open DB at {db_path:?}: {e}"))?;
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

fn cmd_add(config_path: &Path, url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = AppConfig::load_or_default(config_path);
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
    let mut config = AppConfig::load_or_default(config_path);
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
    let mut config = AppConfig::load_or_default(config_path);
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
    let mut config = AppConfig::load_or_default(config_path);
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
    let config = AppConfig::load_or_default(config_path);

    println!("Node ID:           {}", config.node_id);
    println!("Listen port:       {}", config.listen_port);
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
