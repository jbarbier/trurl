#!/usr/bin/env python3
"""Write latest.json, the update manifest both Trurl apps read from GitHub Releases.

  python3 scripts/make_manifest.py --version 0.2.0 --notes-file notes.md \\
      --windows Trurl_0.2.0_x64-setup.exe --linux trurl-0.2.0-linux-x64.tar.gz \\
      --out latest.json

Each file must have its minisign signature next to it (<file>.sig), made with
the updater key (`tauri signer sign`). The desktop app reads `windows-x86_64`,
the terminal app reads `linux-x86_64`.
"""

from __future__ import annotations

import argparse
import datetime
import json
from pathlib import Path

REPO = "jbarbier/trurl"


def entry(path: Path, version: str) -> dict:
    sig = Path(str(path) + ".sig")
    if not sig.exists():
        raise SystemExit(f"missing signature {sig}")
    return {
        "signature": sig.read_text().strip(),
        "url": f"https://github.com/{REPO}/releases/download/v{version}/{path.name}",
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True)
    ap.add_argument("--notes-file", required=True)
    ap.add_argument("--windows")
    ap.add_argument("--linux")
    ap.add_argument("--out", default="latest.json")
    a = ap.parse_args()
    platforms = {}
    if a.windows:
        platforms["windows-x86_64"] = entry(Path(a.windows), a.version)
    if a.linux:
        platforms["linux-x86_64"] = entry(Path(a.linux), a.version)
    if not platforms:
        raise SystemExit("give --windows and/or --linux")
    manifest = {
        "version": a.version,
        "notes": Path(a.notes_file).read_text().strip(),
        "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": platforms,
    }
    Path(a.out).write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"wrote {a.out}: {', '.join(platforms)}")


if __name__ == "__main__":
    main()
