#!/usr/bin/env python3
"""Mirror a curated subset of Kenney audio from the asset server into assets/audio/.

Reads `tools/audio_sync.toml`, fetches each `<server.base_url>/<server.prefix>/<rel>`
into `assets/audio/<category>/<local>.ogg`, and skips files already present with
the expected size (looked up in the server's `index.json`).

Usage:
    python3 tools/audio_sync.py            # sync all categories
    python3 tools/audio_sync.py ui music   # sync only listed categories
"""
from __future__ import annotations

import argparse
import json
import sys
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

try:
    import tomllib  # py311+
except ModuleNotFoundError:
    import tomli as tomllib  # type: ignore[no-redef]

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST  = REPO_ROOT / "tools" / "audio_sync.toml"
DEST_ROOT = REPO_ROOT / "assets" / "audio"


def load_manifest() -> dict:
    with MANIFEST.open("rb") as f:
        return tomllib.load(f)


def fetch_index(base_url: str) -> dict[str, int]:
    """Return {server_path: size_bytes} for every file the asset server advertises."""
    url = base_url.rstrip("/") + "/index.json"
    with urllib.request.urlopen(url, timeout=30) as resp:
        data = json.load(resp)
    sizes: dict[str, int] = {}
    for pack in data.get("packs", []):
        for f in pack.get("files", []):
            sizes[f["path"]] = f["size"]
    return sizes


def download(url: str, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".part")
    with urllib.request.urlopen(url, timeout=60) as resp, tmp.open("wb") as out:
        while chunk := resp.read(64 * 1024):
            out.write(chunk)
    tmp.replace(dest)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("categories", nargs="*", help="Categories to sync (default: all)")
    ap.add_argument("-j", "--jobs", type=int, default=8, help="Parallel downloads")
    args = ap.parse_args()

    manifest = load_manifest()
    server = manifest["server"]
    base_url = server["base_url"].rstrip("/")
    prefix   = server["prefix"].strip("/")

    print(f"Fetching index from {base_url}/index.json ...", flush=True)
    sizes = fetch_index(base_url)

    categories = [k for k in manifest if k != "server"]
    if args.categories:
        unknown = set(args.categories) - set(categories)
        if unknown:
            print(f"Unknown categories: {sorted(unknown)}", file=sys.stderr)
            return 2
        categories = list(args.categories)

    jobs: list[tuple[str, str, Path, int | None]] = []
    for cat in categories:
        for local_name, rel in manifest[cat].items():
            server_path = f"{prefix}/{rel}"
            url = f"{base_url}/" + urllib.parse.quote(server_path)
            dest = DEST_ROOT / cat / local_name
            expected = sizes.get(server_path)
            jobs.append((cat, url, dest, expected))

    skipped = 0
    downloaded = 0
    failed: list[tuple[Path, str]] = []

    todo: list[tuple[str, Path]] = []
    for cat, url, dest, expected in jobs:
        if dest.exists() and expected is not None and dest.stat().st_size == expected:
            skipped += 1
            continue
        todo.append((url, dest))

    print(f"{skipped} already in sync; {len(todo)} to fetch", flush=True)

    if todo:
        with ThreadPoolExecutor(max_workers=args.jobs) as pool:
            futs = {pool.submit(download, url, dest): (url, dest) for url, dest in todo}
            for fut in as_completed(futs):
                url, dest = futs[fut]
                try:
                    fut.result()
                    downloaded += 1
                    rel = dest.relative_to(REPO_ROOT)
                    print(f"  + {rel}", flush=True)
                except Exception as e:  # noqa: BLE001
                    failed.append((dest, f"{type(e).__name__}: {e}"))

    print(f"\nDone. {downloaded} downloaded, {skipped} skipped, {len(failed)} failed.")
    if failed:
        for dest, err in failed:
            print(f"  ! {dest.relative_to(REPO_ROOT)}: {err}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
