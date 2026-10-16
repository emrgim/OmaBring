#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$root"
cargo build --release
mkdir -p "$HOME/.local/bin" "$HOME/.config/systemd/user"
install -m 755 "$root/target/release/omabring" "$HOME/.local/bin/omabring"
install -m 644 "$root/deploy/omabring.service" "$HOME/.config/systemd/user/omabring.service"
systemctl --user daemon-reload
systemctl --user enable --now omabring.service
echo "OmaBring is running at http://127.0.0.1:8877/"
