# opf-watch

Polls opfitalia.net for the next One Piece FHD torrent, downloads it with `aria2c`
and uploads it to Google Drive with `rclone`. The next episode number is kept in
`~/.opf-watch/state.json`.

## Quick install (Debian/Ubuntu)

As the user that will run it (not root), with sudo available:

```
./install.sh
```

It installs `aria2`, `uv` and `rclone` if missing, installs the command, sets up the
systemd user service and enables lingering. Then configure the `gdrive` remote with
`rclone config` if not done yet.

## Manual install

Requirements: `uv`, `aria2c` (`apt install aria2`), and an `rclone` remote named `gdrive`
configured for the user that runs the service (`rclone config`).

```
uv tool install .
opf-watch --help
```

## Run as a systemd user service

```
mkdir -p ~/.config/systemd/user
cp opf-watch.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now opf-watch
sudo loginctl enable-linger "$USER"
journalctl --user -u opf-watch -f
```

Options (download dir, rclone destination, start episode, ...) are flags in `ExecStart`.
After changing the code: `uv tool install --reinstall .`, then `systemctl --user restart opf-watch`.
