#!/usr/bin/env bash
set -euo pipefail

SCRIPT_BINARY="/usr/local/bin/netwatch"
DEB_BINARY="/usr/bin/netwatch"
CONFIG_DIR="/etc/netwatch"
DATA_DIR="/var/lib/netwatch"
SERVICE_FILE="/etc/systemd/system/netwatch.service"
DEB_SERVICE_FILE="/lib/systemd/system/netwatch.service"
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
UNIT_REMOVED=0
for unit in "$SERVICE_FILE" "$DEB_SERVICE_FILE"; do
    if [ -f "$unit" ]; then
        rm -f "$unit"
        ok "Systemd unit removed: ${unit}"
        UNIT_REMOVED=1
    fi
done
if [ "$UNIT_REMOVED" -eq 1 ]; then
    if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
        systemctl daemon-reload
    else
        warn "Skipping systemctl daemon-reload (systemd not available)"
    fi
fi

# ── remove binary ─────────────────────────────────────────────────────────────
for bin in "$SCRIPT_BINARY" "$DEB_BINARY"; do
    if [ -f "$bin" ]; then
        rm -f "$bin"
        ok "Binary removed: ${bin}"
    fi
done

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
