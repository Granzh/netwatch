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

## Building from Source

To generate a fully optimized, "stripped" binary for Linux:

```Bash
cargo build --release --target x86_64-unknown-linux-musl
```

The resulting binary is self-contained and ready for deployment
on any Linux distribution without additional shared libraries.

## Configuration

The system dynamically manages two types of endpoints:

- Public Targets: External services (e.g., Google, Telegram, Claude) to be monitored.

- Internal Peers: Special addresses of other Netwatch nodes for data exchange.
