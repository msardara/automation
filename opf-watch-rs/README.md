# opf-watch-rs

Rust version of `opf-watch`. Polls opfitalia.net for the next One Piece FHD torrent,
downloads it with [librqbit](https://crates.io/crates/librqbit) and uploads it to Google
Drive with [google-drive3](https://crates.io/crates/google-drive3) (resumable, chunked,
MD5-verified). No `aria2c` or `rclone` needed.

## Build

Needs a Rust toolchain, a C compiler and `cmake` (for `aws-lc-sys`):

```
sudo apt install build-essential cmake pkg-config
cargo install --path .
```

## Google Drive authorization (once)

1. In Google Cloud Console create a project, enable the **Google Drive API**, configure the
   OAuth consent screen and create an **OAuth client ID** of type *Desktop app*.
   Publish the consent screen to *In production*, otherwise refresh tokens expire after 7 days.
2. Save the client file as `~/.opf-watch/client_secret.json`.
3. Authorize. A browser is needed for the redirect to `localhost:8085`:
   - on a machine with a browser: `opf-watch-rs auth`, then copy `~/.opf-watch/token.json`
     to the same path on the server, or
   - on the server over SSH: `ssh -L 8085:localhost:8085 server`, run `opf-watch-rs auth`,
     open the printed URL in your local browser.

## Run

```
opf-watch-rs --start 1181
```

Files go under `My Drive/One Piece/16 - Wano (891-)` by default (`--drive-folder`). The next
episode number is stored in `~/.opf-watch/state.json`. See `opf-watch-rs --help`.

## systemd user service

```
mkdir -p ~/.config/systemd/user
cp opf-watch-rs.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now opf-watch-rs
sudo loginctl enable-linger "$USER"
journalctl --user -u opf-watch-rs -f
```
