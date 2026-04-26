#!/usr/bin/env bash
set -euo pipefail

REPO="Granzh/netwatch"
BINARY="netwatch"
INSTALL_DIR="/usr/local/bin"
CONFIG_DIR="/etc/netwatch"
DATA_DIR="/var/lib/netwatch"
SERVICE_FILE="/etc/systemd/system/netwatch.service"
SERVICE_USER="netwatch"

BOLD="\033[1m"
GREEN="\033[32m"
RED="\033[31m"
RESET="\033[0m"

info()  { echo -e "${BOLD}[netwatch]${RESET} $*"; }
ok()    { echo -e "${GREEN}[netwatch]${RESET} $*"; }
die()   { echo -e "${RED}[netwatch] error:${RESET} $*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "run as root (sudo $0)"

# ── resolve version ────────────────────────────────────────────────────────────
if [ -z "${VERSION:-}" ]; then
    info "Fetching latest release tag..."
    VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
    [ -n "$VERSION" ] || die "Could not determine latest version"
fi
info "Installing netwatch ${VERSION}"

# ── detect architecture ───────────────────────────────────────────────────────
ARCH=$(uname -m)
case "$ARCH" in
    x86_64)
        RUST_TARGET="x86_64-unknown-linux-musl"
        ;;
    aarch64)
        die "Unsupported architecture: ${ARCH}. Prebuilt release artifacts are currently only published for x86_64."
        ;;
    armv7l)
        die "Unsupported architecture: ${ARCH}. Prebuilt release artifacts are currently only published for x86_64."
        ;;
    *)
        die "Unsupported architecture: ${ARCH}"
        ;;

esac

# ── download binary ────────────────────────────────────────────────────────────
ARCHIVE="netwatch-${VERSION}-${RUST_TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

info "Downloading ${URL} ..."
curl -fsSL "$URL" -o "${TMP}/${ARCHIVE}"
tar -xzf "${TMP}/${ARCHIVE}" -C "$TMP"
install -m 0755 "${TMP}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
ok "Binary installed to ${INSTALL_DIR}/${BINARY}"

# ── system user ───────────────────────────────────────────────────────────────
if ! getent group "$SERVICE_USER" >/dev/null; then
    groupadd -r "$SERVICE_USER"
    ok "Created system group '${SERVICE_USER}'"
else
    info "Group '${SERVICE_USER}' already exists, skipping"
fi

NOLOGIN=$(command -v nologin 2>/dev/null || echo /usr/sbin/nologin)
if ! id "$SERVICE_USER" &>/dev/null; then
    useradd -r -g "$SERVICE_USER" -s "$NOLOGIN" -d "$DATA_DIR" -M "$SERVICE_USER"
    ok "Created system user '${SERVICE_USER}'"
else
    info "User '${SERVICE_USER}' already exists, skipping"
fi

# ── directories ───────────────────────────────────────────────────────────────
install -d -m 0755 -o "$SERVICE_USER" -g "$SERVICE_USER" "$DATA_DIR"
install -d -m 0755 "$CONFIG_DIR"

# ── default config ─────────────────────────────────────────────────────────────
if [ ! -f "${CONFIG_DIR}/config.toml" ]; then
    if [ -t 0 ] && [ -t 1 ]; then
        # Interactive terminal — ask the user for key settings
        "${INSTALL_DIR}/${BINARY}" init
    else
        # Non-interactive (pipe/CI) — write defaults silently
        "${INSTALL_DIR}/${BINARY}" init --defaults
    fi
    chown root:"$SERVICE_USER" "${CONFIG_DIR}/config.toml"
    chmod 0640 "${CONFIG_DIR}/config.toml"
else
    info "Config already exists at ${CONFIG_DIR}/config.toml, skipping"
fi

# ── systemd unit ──────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
if [ -f "${SCRIPT_DIR}/netwatch.service" ]; then
    install -m 0644 "${SCRIPT_DIR}/netwatch.service" "$SERVICE_FILE"
else
    curl -fsSL "https://raw.githubusercontent.com/${REPO}/${VERSION}/deploy/netwatch.service" \
        -o "$SERVICE_FILE"
    chmod 0644 "$SERVICE_FILE"
fi
# Rewrite ExecStart to match the script install location (/usr/local/bin vs /usr/bin in .deb)
sed -i "s|ExecStart=.*netwatch |ExecStart=${INSTALL_DIR}/${BINARY} |" "$SERVICE_FILE"
ok "Systemd unit installed to ${SERVICE_FILE}"

if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
    systemctl daemon-reload
    systemctl enable netwatch
    systemctl restart netwatch
    ok "netwatch ${VERSION} is running"
    echo
    echo "  Status : systemctl status netwatch"
    echo "  Logs   : journalctl -u netwatch -f"
    echo "  Config : ${CONFIG_DIR}/config.toml"
else
    info "systemd is not available in this environment; skipping service enable/start"
    echo
    echo "  Service unit installed at: ${SERVICE_FILE}"
    echo "  Config               : ${CONFIG_DIR}/config.toml"
    echo "  Run manually         : ${INSTALL_DIR}/${BINARY} run"
fi
