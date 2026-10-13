#!/usr/bin/env python3
"""Build the folder you copy onto USB sticks: installer + models, no internet needed
on the learners' computers. See docs/USB-BUNDLE.md.

  python3 scripts/make_usb_bundle.py                       # latest release from GitHub
  python3 scripts/make_usb_bundle.py --installer path/to/Trurl_0.1.0_x64-offline-setup.exe
  python3 scripts/make_usb_bundle.py --no-windows --linux-tarball dist/trurl-0.1.0-linux-x64.tar.gz
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
import urllib.error
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
    try:
        with urllib.request.urlopen(url, timeout=120) as r, open(tmp, "wb") as f:
            shutil.copyfileobj(r, f)
    except urllib.error.HTTPError as e:
        if e.code == 404:
            sys.exit(
                f"Not found: {url}\n"
                "That version is not published on GitHub Releases yet. Publish the release, "
                "or pass --installer with an installer you already have (CI builds one: "
                "Actions > CI > windows-installer-and-screenshots)."
            )
        raise
    tmp.replace(dest)


def release_sums(v: str) -> dict[str, str]:
    """SHA256SUMS.txt of the GitHub release: what every file on the stick must match."""
    url = f"https://github.com/{REPO}/releases/download/v{v}/SHA256SUMS.txt"
    with urllib.request.urlopen(url, timeout=60) as r:
        text = r.read(1 << 20).decode()
    sums = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) == 2:
            sums[parts[1].lstrip("*")] = parts[0].lower()
    return sums


def check(path: Path, sums: dict[str, str]) -> None:
    """Every learner who installs from this stick runs this file: it must be the
    one the release workflow built and signed, bit for bit."""
    want = sums.get(path.name)
    if not want:
        sys.exit(f"{path.name} is not listed in the release's SHA256SUMS.txt")
    got = sha256(path)
    if got != want:
        path.unlink()
        sys.exit(f"{path.name} does not match the release (sha256 {got}, expected {want}). Deleted it; run again.")
    print(f"ok   {path.name} matches the release")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--installer", help="offline setup .exe (default: download from the GitHub release)")
    ap.add_argument("--version", default=PINS["app"]["version"])
    ap.add_argument("--only", default="", help="comma-separated model ids from pins.json")
    ap.add_argument("--extra-model", action="append", default=[], help="another .gguf to include")
    ap.add_argument("--linux", action="store_true", help="also include the Linux tarball from the release")
    ap.add_argument("--linux-tarball", help="include this Linux tarball (your own build) instead of the release's")
    ap.add_argument("--no-windows", action="store_true", help="no Windows installer (a stick for Linux computers only)")
    ap.add_argument("--models-cache", default=str(Path.home() / ".cache" / "trurl" / "models"))
    ap.add_argument("--out", default=str(ROOT / "dist"))
    a = ap.parse_args()

    v = a.version
    out = Path(a.out) / f"Trurl-{v}-USB"
    (out / "models").mkdir(parents=True, exist_ok=True)

    exe_name = f"Trurl_{v}_x64-offline-setup.exe"
    need_release = (not a.no_windows and not a.installer) or (a.linux and not a.linux_tarball)
    release = release_sums(v) if need_release else None
    if a.no_windows:
        pass
    elif a.installer:
        shutil.copy2(a.installer, out / exe_name)
        print(f"note {exe_name} is your own build: not checked against a release")
    else:
        if not (out / exe_name).exists():
            download(f"https://github.com/{REPO}/releases/download/v{v}/{exe_name}", out / exe_name)
        check(out / exe_name, release)

    if a.linux or a.linux_tarball:
        (out / "linux").mkdir(exist_ok=True)
        tgz = f"trurl-{v}-linux-x64.tar.gz"
        if a.linux_tarball:
            shutil.copy2(a.linux_tarball, out / "linux" / tgz)
            print(f"note {tgz} is your own build: not checked against a release")
        else:
            if not (out / "linux" / tgz).exists():
                download(f"https://github.com/{REPO}/releases/download/v{v}/{tgz}", out / "linux" / tgz)
            check(out / "linux" / tgz, release)

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
    # The read-me lists the models really on this stick.
    def gb(n: int) -> str:
        return f"{n / 1e9:.1f} GB" if n >= 1e9 else f"{round(n / 1e6)} MB"

    on_stick = sorted((out / "models").glob("*.gguf"))
    names = {m["file"]: m.get("name", m["file"]) for m in PINS["models"]}
    listing = ", ".join(f"{names.get(f.name, f.name)} ({gb(f.stat().st_size)})" for f in on_stick)
    readme = (ROOT / "docs" / "USB-READ-ME-FIRST.txt").read_text()
    marker = "  models/             "
    lines = [(marker + listing) if l.startswith(marker) else l for l in readme.splitlines()]
    (out / "READ-ME-FIRST.txt").write_text("\n".join(lines) + "\n")

    sums = []
    for p in sorted(out.rglob("*")):
        if p.is_file() and p.name != "SHA256SUMS.txt":
            sums.append(f"{sha256(p)}  {p.relative_to(out).as_posix()}")
    (out / "SHA256SUMS.txt").write_text("\n".join(sums) + "\n")
    total = sum(p.stat().st_size for p in out.rglob("*") if p.is_file())
    print(f"\n{out}\n{total / 1e9:.2f} GB. Copy the whole folder onto each USB stick (FAT32 or exFAT both work: the largest file is under 4 GB).")


if __name__ == "__main__":
    main()
