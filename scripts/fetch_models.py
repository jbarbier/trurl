#!/usr/bin/env python3
"""Download the pinned models from pins.json and check their SHA-256.

Resumable: an interrupted run continues where it stopped (HTTP Range).
Existing files with the right size and hash are skipped.

Usage:
  python3 scripts/fetch_models.py --dest DIR              # every pinned model
  python3 scripts/fetch_models.py --dest DIR --only gemma-3-1b-it,qwen3-0.6b
  python3 scripts/fetch_models.py --dest DIR --link-from OTHER_DIR   # reuse files already on disk
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PINS = json.loads((ROOT / "pins.json").read_text())


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 22), b""):
            h.update(chunk)
    return h.hexdigest()


def download(url: str, part: Path, size: int) -> None:
    for attempt in range(8):
        have = part.stat().st_size if part.exists() else 0
        if have >= size:
            return
        req = urllib.request.Request(url, headers={"Range": f"bytes={have}-", "User-Agent": "trurl-fetch/1"})
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                mode = "ab" if r.status == 206 else "wb"
                with open(part, mode) as f:
                    t0, done = time.time(), (have if mode == "ab" else 0)
                    while True:
                        b = r.read(1 << 20)
                        if not b:
                            break
                        f.write(b)
                        done += len(b)
                        if time.time() - t0 > 5:
                            print(f"  {part.name}: {done * 100 // size}%", flush=True)
                            t0 = time.time()
            return
        except (urllib.error.URLError, TimeoutError, ConnectionError) as e:
            wait = min(60, 2 ** attempt)
            print(f"  {e}; retrying in {wait}s", flush=True)
            time.sleep(wait)
    sys.exit(f"giving up on {url}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dest", required=True)
    ap.add_argument("--only", default="")
    ap.add_argument("--link-from", default="")
    args = ap.parse_args()
    dest = Path(args.dest).expanduser()
    dest.mkdir(parents=True, exist_ok=True)
    only = {x.strip() for x in args.only.split(",") if x.strip()}
    ok = True
    for m in PINS["models"]:
        if only and m["id"] not in only:
            continue
        target = dest / m["file"]
        if not target.exists() and args.link_from:
            src = Path(args.link_from).expanduser() / m["file"]
            if src.exists() and src.stat().st_size == m["size"]:
                try:
                    os.link(src, target)
                except OSError:
                    pass
        if target.exists() and target.stat().st_size == m["size"] and sha256(target) == m["sha256"]:
            print(f"ok   {m['file']}")
            continue
        url = f"https://huggingface.co/{m['repo']}/resolve/{m['revision']}/{m['file']}"
        print(f"get  {m['file']} ({m['size'] // 1_000_000} MB)", flush=True)
        part = target.with_suffix(".gguf.part")
        download(url, part, m["size"])
        got = sha256(part)
        if got != m["sha256"]:
            part.unlink()
            print(f"BAD  {m['file']}: sha256 {got} != {m['sha256']}", file=sys.stderr)
            ok = False
            continue
        part.replace(target)
        print(f"ok   {m['file']}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
