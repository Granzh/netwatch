# Netwatch

Netwatch is a lightweight, distributed agent designed for monitoring network
resource availability. Built on a mesh-network architecture,
each node synchronizes service status data with its "peers" using a
gossip protocol, ensuring a resilient monitoring web without a single point of failure.

## Key Features

- Stealth Monitoring: Avoids suspicious ICMP traffic. Uses asynchronous HTTP/HTTPS
`HEAD` requests with randomized delays (jitter) and rotating User-Agents
to mimic legitimate browser behavior.

- Mesh Architecture: Implements a Gossip Protocol for decentralized
data synchronization between trusted nodes.

- Minimal Footprint: Written in Rust and statically compiled via musl. It
carries zero external dependencies and is optimized for ultra-low RAM and CPU usage.

- Smart Persistence: Maintains a local history of service availability
using a compact database with automatic log rotation to prevent storage bloat.

## Tech Stack

- Async Runtime: Tokio

- Networking: reqwest + rustls (configured for TLS fingerprinting)

- Storage: SQLite

- CI/CD: GitHub Actions for automated x86_64-unknown-linux-musl static builds.

## Installation

### Install script (recommended)

Downloads the latest release binary, creates a `netwatch` system user, writes a
default config to `/etc/netwatch/config.toml`, and registers a systemd service:

```bash
curl -fsSL 
https://raw.githubusercontent.com/Granzh/netwatch/main/deploy/install.sh |
sudo bash
```

To pin a specific version:

```bash
sudo VERSION=v0.2.0 bash <(curl -fsSL .../install.sh)
```

To uninstall (keeps data and config by default):

```bash
curl -fsSL
https://raw.githubusercontent.com/Granzh/netwatch/main/deploy/uninstall.sh |
sudo bash
# To also delete config and database:
sudo PURGE=1 bash <(curl -fsSL .../uninstall.sh)
```

### Debian / Ubuntu package

Download the `.deb`
from the [Releases](https://github.com/Granzh/netwatch/releases) page and install:

```bash
sudo dpkg -i netwatch_<version>_amd64.deb
```

The package automatically creates the `netwatch` user, writes a default config,
and enables the systemd service via `postinst`.

### Building from source

Requires Rust and the `x86_64-unknown-linux-musl` target:

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
# Binary: target/x86_64-unknown-linux-musl/release/netwatch
```

## Configuration

After installation, edit `/etc/netwatch/config.toml` (or any path passed via `--config`):

```toml
# URLs to probe — netwatch issues HTTP HEAD requests to each
sources = [
    "https://www.google.com",
    "https://www.cloudflare.com",
    "https://www.github.com",
]

# Latency above this value (ms) is treated as degraded
latency_threshold_ms = 500

# Base probe interval in seconds; actual interval = check_interval ± check_jitter
check_interval_seconds = 60
check_jitter_seconds = 5

# Maximum concurrent outbound probes
max_concurrent_checks = 10

# Per-request HTTP timeout (seconds)
request_timeout_secs = 10

# Follow HTTP redirects
follow_redirects = true

# Accept self-signed / invalid TLS certificates (use with care)
danger_accept_invalid_certs = false

# HTTP API — used by peers and the CLI to query status
http_api = "127.0.0.1"
listen_port = 8080

# Optional shared secret — set the same value on all peers
# api_secret = "change-me"

# Unique identifier for this node (defaults to hostname:port)
# node_id = "dc1-node1"

# Peer nodes — add the base URLs of other netwatch instances
peers = []
sync_interval_seconds = 60
max_concurrent_syncs = 5
sync_timeout_secs = 30
```

Netwatch watches the config file for changes and reloads without restart.

The system dynamically manages two types of endpoints:

- **Public Targets** — external services (e.g., Google, Cloudflare) to be monitored.
- **Internal Peers** — other Netwatch nodes for mesh data synchronisation.
