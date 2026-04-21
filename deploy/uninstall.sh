#!/usr/bin/env bash
set -euo pipefail

BINARY="/usr/local/bin/netwatch"
CONFIG_DIR="/etc/netwatch"
DATA_DIR="/var/lib/netwatch"
SERVICE_FILE="/etc/systemd/system/netwatch.service"
SERVICE_USER="netwatch"

BOLD="\033[1m"
GREEN="\033[32m"
YELLOW="\033[33m"
RED="\033[31m"
RESET="\033[0m"

info()  { echo -e "${BOLD}[netwatch]${RESET} $*"; }
ok()    { echo -e "${GREEN}[netwatch]${RESET} $*"; }
warn()  { echo -e "${YELLOW}[netwatch]${RESET} $*"; }
die()   { echo -e "${RED}[netwatch] error:${RESET} $*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "run as root (sudo $0)"

# ── stop and disable service ──────────────────────────────────────────────────
if systemctl is-active --quiet netwatch 2>/dev/null; then
    systemctl stop netwatch
    ok "Service stopped"
fi

if systemctl is-enabled --quiet netwatch 2>/dev/null; then
    systemctl disable netwatch
    ok "Service disabled"
fi

# ── remove unit file ──────────────────────────────────────────────────────────
if [ -f "$SERVICE_FILE" ]; then
    rm -f "$SERVICE_FILE"
    systemctl daemon-reload
    ok "Systemd unit removed"
fi

# ── remove binary ─────────────────────────────────────────────────────────────
if [ -f "$BINARY" ]; then
    rm -f "$BINARY"
    ok "Binary removed: ${BINARY}"
fi

# ── remove config (ask first) ─────────────────────────────────────────────────
if [ -d "$CONFIG_DIR" ]; then
    if [ "${PURGE:-0}" = "1" ]; then
        rm -rf "$CONFIG_DIR"
        ok "Config directory removed: ${CONFIG_DIR}"
    else
        warn "Config kept at ${CONFIG_DIR}  (re-run with PURGE=1 to delete)"
    fi
fi

# ── remove data directory (ask first) ────────────────────────────────────────
if [ -d "$DATA_DIR" ]; then
    if [ "${PURGE:-0}" = "1" ]; then
        rm -rf "$DATA_DIR"
        ok "Data directory removed: ${DATA_DIR}"
    else
        warn "Data kept at ${DATA_DIR}  (re-run with PURGE=1 to delete)"
    fi
fi

# ── remove system user ────────────────────────────────────────────────────────
if id "$SERVICE_USER" &>/dev/null; then
    userdel "$SERVICE_USER"
    ok "System user '${SERVICE_USER}' removed"
fi

ok "Uninstall complete"
