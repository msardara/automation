"""Wait for new OPF Italia One Piece torrents, download them, upload to Google Drive.

Needs `aria2c` and `rclone` on PATH:  apt install aria2
"""

import argparse
import json
import logging
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

URL_TEMPLATE = "https://www.opfitalia.net/tor/{episode}fhd.torrent"
USER_AGENT = "Mozilla/5.0 (X11; Linux x86_64) opf-watch"

log = logging.getLogger("opf-watch")


def parse_args():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--start", type=int, default=1181, help="first episode to look for (ignored once state file exists)")
    p.add_argument("--download-dir", type=Path, default=Path("/mnt/storagebox/onepiece"))
    p.add_argument("--rclone-dest", default="gdrive:One Piece/16 - Wano (891-)")
    p.add_argument("--state-file", type=Path, default=Path.home() / ".opf-watch" / "state.json")
    p.add_argument("--interval", type=int, default=600, help="seconds between checks when the next torrent is not out")
    p.add_argument("--seed-time", type=int, default=0, help="minutes to seed after download")
    p.add_argument("--delete-after-upload", action="store_true", help="remove local files once uploaded")
    p.add_argument("--once", action="store_true", help="check once and exit if nothing is available")
    return p.parse_args()


def load_next_episode(state_file, default):
    try:
        return json.loads(state_file.read_text())["next_episode"]
    except (FileNotFoundError, KeyError, ValueError):
        return default


def save_next_episode(state_file, episode):
    state_file.parent.mkdir(parents=True, exist_ok=True)
    state_file.write_text(json.dumps({"next_episode": episode}))


def fetch_torrent(episode, dest_dir):
    """Download the .torrent if published. Returns its path, or None if not available yet."""
    url = URL_TEMPLATE.format(episode=episode)
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            data = resp.read()
    except urllib.error.HTTPError as e:
        if e.code in (403, 404):
            return None
        raise
    # a missing file may be served as an HTML page with status 200
    if not data.startswith(b"d") or b"4:info" not in data:
        return None
    dest_dir.mkdir(parents=True, exist_ok=True)
    path = dest_dir / f"{episode}fhd.torrent"
    path.write_bytes(data)
    return path


def torrent_files(torrent):
    """Relative paths of the files inside the torrent, via `aria2c --show-files`."""
    out = subprocess.run(
        ["aria2c", "--show-files=true", str(torrent)],
        check=True, capture_output=True, text=True,
    ).stdout
    paths = [m.group(1) for m in re.finditer(r"^\s*\d+\|(.+)$", out, re.MULTILINE)]
    return [p.removeprefix("./") for p in paths]


def download(torrent, download_dir, seed_time):
    download_dir.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            "aria2c",
            f"--dir={download_dir}",
            f"--seed-time={seed_time}",
            "--check-integrity=true",
            "--auto-file-renaming=false",
            "--allow-overwrite=true",
            "--file-allocation=none",
            "--summary-interval=60",
            "--console-log-level=warn",
            str(torrent),
        ],
        check=True,
    )


def upload(files, download_dir, dest, delete_after):
    for rel in files:
        src = download_dir / rel
        # keep the torrent's folder layout if it has one
        target = dest if "/" not in rel else f"{dest}/{Path(rel).parent}"
        log.info("uploading %s -> %s", src, target)
        subprocess.run(["rclone", "copy", str(src), target, "-P"], check=True)
        if delete_after:
            src.unlink()


def main():
    args = parse_args()
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    episode = load_next_episode(args.state_file, args.start)
    torrent_dir = args.state_file.parent / "torrents"

    while True:
        try:
            torrent = fetch_torrent(episode, torrent_dir)
            if torrent is None:
                log.info("episode %d not available yet", episode)
                if args.once:
                    return 0
                time.sleep(args.interval)
                continue

            log.info("episode %d available, downloading", episode)
            files = torrent_files(torrent)
            download(torrent, args.download_dir, args.seed_time)
            upload(files, args.download_dir, args.rclone_dest, args.delete_after_upload)
            episode += 1
            save_next_episode(args.state_file, episode)
            log.info("episode %d done", episode - 1)
        except (urllib.error.URLError, TimeoutError) as e:
            log.warning("network error: %s", e)
            time.sleep(args.interval)
        except subprocess.CalledProcessError as e:
            # state is not advanced, so the same episode is retried (aria2c resumes)
            log.error("%s failed (exit %d), retrying in %ds", e.cmd[0], e.returncode, args.interval)
            time.sleep(args.interval)


def cli():
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
