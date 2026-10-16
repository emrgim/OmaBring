#!/usr/bin/env bash
# Install the OmaBring app, launcher entry, and user service.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${HOME}/.local"
BIN_DIR="${PREFIX}/bin"
APP_DIR="${PREFIX}/share/applications"
ICON_DIR="${PREFIX}/share/icons/hicolor/scalable/apps"
UNIT_DIR="${HOME}/.config/systemd/user"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required to build OmaBring" >&2
  exit 1
fi
if ! pkg-config --exists gtk4 libadwaita-1; then
  echo "GTK 4 and libadwaita development files are required" >&2
  exit 1
fi

echo "Building OmaBring"
cargo build --release --manifest-path "${ROOT}/Cargo.toml"

mkdir -p "${BIN_DIR}" "${APP_DIR}" "${ICON_DIR}" "${UNIT_DIR}"
install -m 0755 "${ROOT}/target/release/omabring" "${BIN_DIR}/omabring"
install -m 0755 "${ROOT}/target/release/omabring-service" "${BIN_DIR}/omabring-service"
install -m 0644 "${ROOT}/assets/omabring.svg" "${ICON_DIR}/omabring.svg"

DESKTOP="${APP_DIR}/omabring.desktop"
sed "s|^Exec=.*|Exec=${BIN_DIR}/omabring|" "${ROOT}/packaging/omabring.desktop" > "${DESKTOP}"
chmod 0644 "${DESKTOP}"
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "${APP_DIR}" || true
fi

install -m 0644 "${ROOT}/packaging/omabring.service" "${UNIT_DIR}/omabring.service"
if command -v systemctl >/dev/null 2>&1; then
  systemctl --user daemon-reload || true
  systemctl --user enable --now omabring.service || true
fi

# OmaBring is not an Omarchy bar plugin. Drop a copy left by an older install.
rm -rf "${HOME}/.config/omarchy/plugins/emrgim.omabring"

# omarchy-theme-set renders this into the active theme as omabring.css,
# beside supermarket.css and the other per-app sheets.
mkdir -p "${HOME}/.config/omarchy/themed"
install -m 0644 "${ROOT}/packaging/omabring.css.tpl" "${HOME}/.config/omarchy/themed/omabring.css.tpl"

echo "OmaBring is installed."
echo "App: ${BIN_DIR}/omabring"
echo "Service: ${BIN_DIR}/omabring-service"
echo "Launcher entry: ${DESKTOP}"
