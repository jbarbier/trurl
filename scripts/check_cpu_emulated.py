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

Each build answers twice: with the f32 model as given, and with a Q4_0 copy this
script writes next to the logs. The copy matters: the SSSE3 and AVX code for
Q4_0 (the reason the ssse3 build exists) only runs on a quantized model.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import socket
import struct
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


# GGUF value types with a fixed size (struct format); 8 is a string, 9 an array.
GGUF_FIXED = {0: "B", 1: "b", 2: "H", 3: "h", 4: "I", 5: "i", 6: "f", 7: "?", 10: "Q", 11: "q", 12: "d"}
GGML_F32, GGML_Q4_0 = 0, 2


def quantize_q4_0(values: tuple) -> bytes:
    """ggml's reference Q4_0 (quantize_row_q4_0_ref): blocks of 32, one f16 scale."""
    out = bytearray()
    for b in range(0, len(values), 32):
        x = values[b:b + 32]
        mx = max(x, key=abs)
        d = mx / -8
        inv = 1 / d if d else 0.0
        q = [min(15, int(v * inv + 8.5)) for v in x]
        out += struct.pack("<e", d)
        out += bytes(q[j] | (q[16 + j] << 4) for j in range(16))
    return bytes(out)


def write_q4_0_copy(src: Path, dst: Path) -> int:
    """Copy an f32 GGUF, storing every 2-D f32 weight whose rows are a multiple
    of 32 as Q4_0. Returns how many tensors were quantized."""
    data = src.read_bytes()
    pos = 0

    def take(fmt: str):
        nonlocal pos
        v = struct.unpack_from("<" + fmt, data, pos)
        pos += struct.calcsize("<" + fmt)
        return v[0]

    def skip_value(t: int):
        nonlocal pos
        if t == 8:
            n = take("Q")  # read before adding: `pos += take()` would load pos first
            pos += n
        elif t == 9:
            it, n = take("I"), take("Q")
            for _ in range(n):
                skip_value(it)
        else:
            pos += struct.calcsize("<" + GGUF_FIXED[t])

    if data[:4] != b"GGUF" or struct.unpack_from("<I", data, 4)[0] != 3:
        sys.exit(f"{src}: not a GGUF version 3 file")
    pos = 8
    n_tensors, n_kv = take("Q"), take("Q")
    align = 32
    for _ in range(n_kv):
        key = data[pos + 8:pos + 8 + struct.unpack_from("<Q", data, pos)[0]].decode()
        pos += 8 + len(key.encode())
        t = take("I")
        if key == "general.alignment":
            align = struct.unpack_from("<I", data, pos)[0]
        skip_value(t)
    head = data[:pos]
    infos = []
    for _ in range(n_tensors):
        name = data[pos + 8:pos + 8 + struct.unpack_from("<Q", data, pos)[0]]
        pos += 8 + len(name)
        dims = [take("Q") for _ in range(take("I"))]
        infos.append((name, dims, take("I"), take("Q")))
    start = (pos + align - 1) // align * align

    tensors, n_quant = [], 0
    for name, dims, typ, off in infos:
        count = 1
        for n in dims:
            count *= n
        if typ != GGML_F32:
            sys.exit(f"{src}: expected an f32 model, {name.decode()} has type {typ}")
        raw = data[start + off:start + off + 4 * count]
        if len(dims) == 2 and dims[0] % 32 == 0 and not name.endswith(b"norm.weight"):
            raw, typ = quantize_q4_0(struct.unpack(f"<{count}f", raw)), GGML_Q4_0
            n_quant += 1
        tensors.append((name, dims, typ, raw))

    out = bytearray(head)
    off = 0
    for name, dims, typ, raw in tensors:
        out += struct.pack("<Q", len(name)) + name + struct.pack("<I", len(dims))
        out += b"".join(struct.pack("<Q", n) for n in dims) + struct.pack("<IQ", typ, off)
        off = (off + len(raw) + align - 1) // align * align
    out += bytes(-len(out) % align)
    for _, _, _, raw in tensors:
        out += raw + bytes(-len(raw) % align)
    dst.write_bytes(bytes(out))
    return n_quant


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
            if WINDOWS:
                # Killing sde.exe leaves the llama-server.exe it launched running: kill the tree.
                subprocess.run(["taskkill", "/T", "/F", "/PID", str(proc.pid)], capture_output=True)
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
    model = Path(a.model).expanduser()
    q4 = logs / (model.stem + "-q4_0.gguf")
    n = write_q4_0_copy(model, q4)
    print(f"wrote {q4.name}: {n} tensors in Q4_0", flush=True)
    bad = 0
    for v in a.variants.split(","):
        exe = next((p for p in [root / v / "llama-server.exe", root / v / "llama-server"] if p.exists()), None)
        if not exe:
            print(f"FAIL {v}: no llama-server under {root / v}")
            bad += 1
            continue
        for kind, m in (("f32", model), ("q4_0", q4)):
            log = logs / f"{v}-{kind}.log"
            ok, msg = run(sde, CHIPS[v], exe, m, log, a.budget)
            print(f"{'ok  ' if ok else 'FAIL'} {v} on {CHIPS[v][1:]}, {kind}: {msg}", flush=True)
            if not ok:
                bad += 1
                tail = log.read_text(errors="replace").splitlines()[-15:]
                print("     " + "\n     ".join(tail))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
