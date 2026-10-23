#!/usr/bin/env bash
# Install opf-watch and its tools on a Debian/Ubuntu server, as the user that will run it.
set -euo pipefail

cd "$(dirname "$0")"

if [ "$(id -u)" -eq 0 ]; then
    echo "Run as a normal user with sudo, not as root." >&2
    exit 1
fi

export PATH="$HOME/.local/bin:$PATH"

if ! command -v aria2c > /dev/null; then
    sudo apt-get update
    sudo apt-get install -y aria2 curl
fi

if ! command -v uv > /dev/null; then
    curl -LsSf https://astral.sh/uv/install.sh | sh
fi

if ! command -v rclone > /dev/null; then
    curl -fsSL https://rclone.org/install.sh | sudo bash
fi

uv tool install --reinstall .

mkdir -p "$HOME/.config/systemd/user"
cp opf-watch.service "$HOME/.config/systemd/user/"
systemctl --user daemon-reload
systemctl --user enable opf-watch
systemctl --user restart opf-watch
sudo loginctl enable-linger "$USER"

if ! rclone listremotes | grep -qx 'gdrive:'; then
    echo
    echo "WARNING: no rclone remote named 'gdrive'. Run 'rclone config', then:"
    echo "  systemctl --user restart opf-watch"
fi

echo
systemctl --user --no-pager status opf-watch || true
echo
echo "Logs: journalctl --user -u opf-watch -f"
