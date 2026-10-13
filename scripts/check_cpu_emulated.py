#!/usr/bin/env python3
"""Run each llama-server build on an emulated older CPU and make it answer.

check_cpu_isa.py reads the machine code. This script runs it: Intel's Software
Development Emulator (SDE) pretends to be an older processor, answers CPUID like
that processor would, and stops the program the moment it executes an
instruction that processor does not have. A build that answers here will not
crash with "illegal instruction" on a learner's old laptop.

  compat  -> Pentium 4 Prescott (2004, SSE3, no SSSE3, no AVX)
  ssse3   -> Core 2 Merom      (2006, SSSE3, no SSE4.2, no AVX)
  avx     -> Sandy Bridge      (2011, AVX, no FMA, no AVX2)
  avx2    -> Haswell           (2013, AVX2 + FMA + F16C + BMI2)

This matters most on Windows: the static Microsoft C runtime inside each .exe
carries AVX and FMA versions of some routines and picks one by asking the CPU
at run time, so reading the code alone cannot tell a real problem from a
correctly guarded one. Running it under the emulator can.

  python3 scripts/check_cpu_emulated.py vendor/llama/linux --model tiny.gguf

The emulator (pinned in pins.json, sha256 checked) is downloaded once into
~/.cache/trurl. Emulation is slow, so use a tiny model (stories260K, see
`scripts/fetch_models.py --test-models`).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import socket
import subprocess
import sys
import tarfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PINS = json.loads((ROOT / "pins.json").read_text())
CHIPS = {"compat": "-p4p", "ssse3": "-mrm", "avx": "-snb", "avx2": "-hsw"}
WINDOWS = platform.system() == "Windows"


def sha256(p: Path) -> str:
    h = hashlib.sha256()
    with p.open("rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def fetch_sde(cache: Path) -> Path:
    pin = PINS["build_tools"]["sde"]
    key = "windows" if WINDOWS else "linux"
    url, want = pin[f"{key}_url"], pin[f"{key}_sha256"].lower()
    out = cache / f"sde-{pin['version']}-{key}"
    exe = out / ("sde.exe" if WINDOWS else "sde64")
    found = next(out.rglob(exe.name), None) if out.exists() else None
    if found:
        return found
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / url.rsplit("/", 1)[1]
    if not archive.exists() or sha256(archive) != want:
        print(f"downloading Intel SDE {pin['version']}", flush=True)
        req = urllib.request.Request(url, headers={"User-Agent": "trurl-build"})
        with urllib.request.urlopen(req, timeout=120) as r, archive.open("wb") as f:
            while b := r.read(1 << 20):
                f.write(b)
    got = sha256(archive)
    if got != want:
        sys.exit(f"Intel SDE checksum mismatch: got {got}, pinned {want}")
    with tarfile.open(archive) as t:
        t.extractall(out, **({"filter": "data"} if hasattr(tarfile, "data_filter") else {}))  # checksum verified above
    found = next(out.rglob(exe.name), None)
    if not found:
        sys.exit(f"{exe.name} not found in {archive}")
    if not WINDOWS:
        for name in ("sde64", "sde", "pin", "pinbin"):
            for f in out.rglob(name):
                f.chmod(0o755)
        for f in out.rglob("*"):
            if f.is_file() and f.parent.name in ("bin", "intel64", "ia32") and not f.suffix:
                f.chmod(0o755)
    return found


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def get(url: str, data: dict | None = None, timeout: float = 5) -> dict:
    body = json.dumps(data).encode() if data is not None else None
    req = urllib.request.Request(url, data=body, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def run(sde: Path, chip: str, exe: Path, model: Path, log: Path, budget: float) -> tuple[bool, str]:
    port = free_port()
    cmd = [str(sde), chip, "--", str(exe), "-m", str(model), "--port", str(port), "--host", "127.0.0.1",
           "-c", "256", "-t", "1", "--no-webui"]
    t0 = time.time()
    with log.open("w") as lf:
        proc = subprocess.Popen(cmd, stdout=lf, stderr=subprocess.STDOUT)
        try:
            while True:
                if proc.poll() is not None:
                    return False, f"exited with code {proc.returncode} before it was ready"
                if time.time() - t0 > budget:
                    return False, f"not ready after {budget:.0f}s"
                try:
                    if get(f"http://127.0.0.1:{port}/health").get("status") == "ok":
                        break
                except Exception:
                    pass
                time.sleep(1)
            r = get(f"http://127.0.0.1:{port}/completion", {"prompt": "Once upon a time", "n_predict": 8, "temperature": 0},
                    timeout=budget)
            text = r.get("content", "")
            if r.get("tokens_predicted", 0) < 1:
                return False, "answered with no tokens"
            return True, f"answered {text!r} in {time.time() - t0:.0f}s"
        except Exception as e:
            return False, f"{type(e).__name__}: {e}"
        finally:
            proc.kill()
            proc.wait()


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("root", nargs="?", default=str(ROOT / "vendor/llama" / ("windows" if WINDOWS else "linux")))
    ap.add_argument("--model", required=True, help="a tiny GGUF, e.g. stories260K.gguf")
    ap.add_argument("--variants", default="compat,ssse3,avx,avx2")
    ap.add_argument("--cache", default=str(Path.home() / ".cache" / "trurl"))
    ap.add_argument("--budget", type=float, default=600, help="seconds allowed per build")
    a = ap.parse_args()

    sde = fetch_sde(Path(a.cache).expanduser())
    root = Path(a.root).expanduser()
    logs = ROOT / "target" / "sde"
    logs.mkdir(parents=True, exist_ok=True)
    bad = 0
    for v in a.variants.split(","):
        exe = next((p for p in [root / v / "llama-server.exe", root / v / "llama-server"] if p.exists()), None)
        if not exe:
            print(f"FAIL {v}: no llama-server under {root / v}")
            bad += 1
            continue
        log = logs / f"{v}.log"
        ok, msg = run(sde, CHIPS[v], exe, Path(a.model).expanduser(), log, a.budget)
        print(f"{'ok  ' if ok else 'FAIL'} {v} on {CHIPS[v][1:]}: {msg}", flush=True)
        if not ok:
            bad += 1
            tail = log.read_text(errors="replace").splitlines()[-15:]
            print("     " + "\n     ".join(tail))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
