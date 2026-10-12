#!/usr/bin/env python3
"""Build the folder you copy onto USB sticks: installer + models, no internet needed
on the learners' computers. See docs/USB-BUNDLE.md.

  python3 scripts/make_usb_bundle.py                       # latest release from GitHub
  python3 scripts/make_usb_bundle.py --installer path/to/Trurl_0.1.0_x64-offline-setup.exe
  python3 scripts/make_usb_bundle.py --only gemma-3-1b-it,qwen3-0.6b   # smaller stick
  python3 scripts/make_usb_bundle.py --extra-model ~/Downloads/phi-4-mini-Q4_K_M.gguf

Result (dist/Trurl-<version>-USB/):
  Trurl_<v>_x64-offline-setup.exe   double-click on Windows 10/11
  models/*.gguf                      copied to each computer by the installer
  linux/trurl-<v>-linux-x64.tar.gz   for Linux computers (optional, --linux)
  LICENSES/                          Gemma Terms of Use and every other license
  READ-ME-FIRST.txt                  for the teacher
  SHA256SUMS.txt                     to check a copy
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PINS = json.loads((ROOT / "pins.json").read_text())
REPO = "jbarbier/trurl"


def sha256(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 22), b""):
            h.update(b)
    return h.hexdigest()


def download(url: str, dest: Path) -> None:
    print(f"downloading {url}")
    tmp = dest.with_suffix(dest.suffix + ".part")
    with urllib.request.urlopen(url, timeout=120) as r, open(tmp, "wb") as f:
        shutil.copyfileobj(r, f)
    tmp.replace(dest)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--installer", help="offline setup .exe (default: download from the GitHub release)")
    ap.add_argument("--version", default=PINS["app"]["version"])
    ap.add_argument("--only", default="", help="comma-separated model ids from pins.json")
    ap.add_argument("--extra-model", action="append", default=[], help="another .gguf to include")
    ap.add_argument("--linux", action="store_true", help="also include the Linux tarball")
    ap.add_argument("--models-cache", default=str(Path.home() / ".cache" / "trurl" / "models"))
    ap.add_argument("--out", default=str(ROOT / "dist"))
    a = ap.parse_args()

    v = a.version
    out = Path(a.out) / f"Trurl-{v}-USB"
    (out / "models").mkdir(parents=True, exist_ok=True)

    exe_name = f"Trurl_{v}_x64-offline-setup.exe"
    if a.installer:
        shutil.copy2(a.installer, out / exe_name)
    elif not (out / exe_name).exists():
        download(f"https://github.com/{REPO}/releases/download/v{v}/{exe_name}", out / exe_name)

    if a.linux:
        (out / "linux").mkdir(exist_ok=True)
        tgz = f"trurl-{v}-linux-x64.tar.gz"
        if not (out / "linux" / tgz).exists():
            download(f"https://github.com/{REPO}/releases/download/v{v}/{tgz}", out / "linux" / tgz)

    # Models: fetch into the cache once (resumable, hash-checked), then copy.
    ids = [m["id"] for m in PINS["models"]] if not a.only else [x.strip() for x in a.only.split(",")]
    subprocess.run([sys.executable, str(ROOT / "scripts/fetch_models.py"), "--dest", a.models_cache, "--only", ",".join(ids)], check=True)
    for m in PINS["models"]:
        if m["id"] not in ids:
            continue
        src = Path(a.models_cache) / m["file"]
        dst = out / "models" / m["file"]
        if not dst.exists() or dst.stat().st_size != m["size"]:
            print(f"copying {m['file']}")
            shutil.copy2(src, dst)
    for extra in a.extra_model:
        p = Path(extra).expanduser()
        if not p.name.lower().endswith(".gguf"):
            sys.exit(f"not a .gguf file: {p}")
        shutil.copy2(p, out / "models" / p.name)

    shutil.copytree(ROOT / "licenses", out / "LICENSES", dirs_exist_ok=True)
    shutil.copy2(ROOT / "LICENSE", out / "LICENSES" / "Trurl-MIT.txt")
    shutil.copy2(ROOT / "docs" / "USB-READ-ME-FIRST.txt", out / "READ-ME-FIRST.txt")

    sums = []
    for p in sorted(out.rglob("*")):
        if p.is_file() and p.name != "SHA256SUMS.txt":
            sums.append(f"{sha256(p)}  {p.relative_to(out).as_posix()}")
    (out / "SHA256SUMS.txt").write_text("\n".join(sums) + "\n")
    total = sum(p.stat().st_size for p in out.rglob("*") if p.is_file())
    print(f"\n{out}\n{total / 1e9:.2f} GB. Copy the whole folder onto each USB stick (FAT32 or exFAT both work: the largest file is under 4 GB).")


if __name__ == "__main__":
    main()
