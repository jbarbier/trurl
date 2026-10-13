#!/usr/bin/env python3
"""Build the pinned llama-server, four times, from source.

Why from source instead of the official release zips:
  * The official Windows build needs the Visual C++ runtime (MSVCP140.dll),
    which many offline Windows installs do not have.
  * The official Linux build needs glibc 2.34 and OpenSSL 3 (Ubuntu 22.04+).
Our builds are single static executables: no DLLs, no C++ runtime, no OpenSSL,
no OpenMP. Nothing to install, nothing to go missing.

Four CPU tiers, picked by Trurl at startup (crates/trurl-core/src/hardware.rs):
  avx2    Haswell (2013) and newer: AVX2 + FMA + F16C + BMI2
  avx     Sandy Bridge / Ivy Bridge (2011-2012): AVX only
  ssse3   Core 2, first Core i3/i5/i7, Atom, AMD Bobcat (2006-2010): SSSE3, no AVX
  compat  any x86-64 CPU (SSE2)

The build never touches the network except to fetch the pinned source tarball,
whose SHA-256 is checked against pins.json. The web UI download that llama.cpp
does by default is switched off.

Usage:
  python3 scripts/build_llama.py                 # all variants for this OS
  python3 scripts/build_llama.py --variants compat
  python3 scripts/build_llama.py --jobs 2 --cmake /path/to/cmake
Output: vendor/llama/<os>/<variant>/llama-server[.exe] and vendor/llama/<os>/BUILD.json
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PINS = json.loads((ROOT / "pins.json").read_text())

COMMON = [
    "-DCMAKE_BUILD_TYPE=Release",
    "-DBUILD_SHARED_LIBS=OFF",
    "-DGGML_BACKEND_DL=OFF",
    "-DGGML_CPU_ALL_VARIANTS=OFF",
    "-DGGML_NATIVE=OFF",
    "-DGGML_OPENMP=OFF",
    "-DLLAMA_OPENSSL=OFF",
    "-DLLAMA_BUILD_TESTS=OFF",
    "-DLLAMA_BUILD_EXAMPLES=OFF",
    "-DLLAMA_BUILD_TOOLS=ON",
    "-DLLAMA_BUILD_SERVER=ON",
    "-DLLAMA_BUILD_UI=OFF",
    "-DLLAMA_USE_PREBUILT_UI=OFF",
    "-DLLAMA_BUILD_IS_DEV=OFF",
    "-DGGML_AVX512=OFF",
    "-DGGML_AVX_VNNI=OFF",
]

OFF_ALL = ["-DGGML_SSE42=OFF", "-DGGML_AVX=OFF", "-DGGML_AVX2=OFF", "-DGGML_BMI2=OFF", "-DGGML_FMA=OFF", "-DGGML_F16C=OFF"]

# Extra compiler flags per variant and OS, passed as CFLAGS/CXXFLAGS (CMake adds
# them to its own defaults). MSVC compiles SSSE3 code on x64 without a switch,
# but ggml only uses it when __SSSE3__ is defined.
# MA_DR_FLAC_NO_SSE41: with MSVC, miniaudio (audio input for multimodal models,
# never used by Trurl) compiles SSE4.1 versions of its FLAC decoder, picked after
# a CPUID check. Safe, but the tiers below SSE4.1 should not contain them at all,
# so check_cpu_isa.py can stay strict.
VARIANT_CFLAGS = {
    "ssse3": {"linux": "-mssse3", "windows": "/D__SSE3__ /D__SSSE3__ /DMA_DR_FLAC_NO_SSE41"},
    "compat": {"windows": "/DMA_DR_FLAC_NO_SSE41"},
}

VARIANTS = {
    "avx2": ["-DGGML_SSE42=ON", "-DGGML_AVX=ON", "-DGGML_AVX2=ON", "-DGGML_BMI2=ON", "-DGGML_FMA=ON", "-DGGML_F16C=ON"],
    "avx": ["-DGGML_SSE42=ON", "-DGGML_AVX=ON", "-DGGML_AVX2=OFF", "-DGGML_BMI2=OFF", "-DGGML_FMA=OFF", "-DGGML_F16C=OFF"],
    # Core 2 and the first Core i3/i5/i7 (2006-2010), Atom, AMD Bobcat: SSSE3, no AVX.
    # ggml has no switch for it: the compiler flag comes from VARIANT_CFLAGS.
    "ssse3": OFF_ALL,
    "compat": OFF_ALL,
}


def host_os() -> str:
    return {"Windows": "windows", "Linux": "linux"}.get(platform.system(), platform.system().lower())


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fetch_source(cache: Path) -> Path:
    src = PINS["llama_cpp"]["source"]
    cache.mkdir(parents=True, exist_ok=True)
    tgz = cache / f"llama.cpp-{PINS['llama_cpp']['commit']}.tar.gz"
    if not tgz.exists() or sha256(tgz) != src["sha256"]:
        print(f"downloading {src['url']}", flush=True)
        tmp = tgz.with_suffix(".part")
        with urllib.request.urlopen(src["url"], timeout=120) as r, open(tmp, "wb") as f:
            shutil.copyfileobj(r, f)
        tmp.replace(tgz)
    got = sha256(tgz)
    if got != src["sha256"]:
        sys.exit(f"source tarball sha256 mismatch: {got} != {src['sha256']}")
    out = cache / f"llama.cpp-{PINS['llama_cpp']['commit']}"
    if not out.exists():
        with tarfile.open(tgz) as t:
            # The checksum is already verified; the "data" filter is extra safety on
            # Pythons that have it (the Debian 11 release builder may not).
            t.extractall(cache, **({"filter": "data"} if hasattr(tarfile, "data_filter") else {}))
    if not (out / "CMakeLists.txt").exists():
        sys.exit(f"unexpected tarball layout: {out}")
    return out


def build(src: Path, build_root: Path, variant: str, cmake: str, jobs: int, os_name: str) -> Path:
    bdir = build_root / variant
    stamp = [f"-DLLAMA_BUILD_NUMBER={PINS['llama_cpp']['build'].lstrip('b')}", f"-DLLAMA_BUILD_COMMIT={PINS['llama_cpp']['commit'][:9]}"]
    args = [cmake, "-S", str(src), "-B", str(bdir)] + COMMON + stamp + VARIANTS[variant]
    if os_name == "windows":
        # Static C runtime: no MSVCP140.dll / VCRUNTIME140.dll needed on the learner's PC.
        args += ["-A", "x64", "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded", "-DCMAKE_POLICY_DEFAULT_CMP0091=NEW"]
    else:
        gen = shutil.which("ninja") or str(Path(cmake).parent.parent.parent / "ninja")
        if Path(gen).exists():
            args += ["-G", "Ninja", f"-DCMAKE_MAKE_PROGRAM={gen}"]
        args += ["-DCMAKE_EXE_LINKER_FLAGS=-static-libstdc++ -static-libgcc"]
    env = dict(os.environ)
    extra = VARIANT_CFLAGS.get(variant, {}).get(os_name)
    # CMake reads CFLAGS only when it first configures a folder. If the flags
    # changed since (or the folder predates this record), forget the cached
    # configuration, or the new flags would be silently ignored.
    record = bdir / "trurl-cflags.txt"
    if (bdir / "CMakeCache.txt").exists() and (not record.exists() or record.read_text() != (extra or "")):
        print(f"+ {variant}: compiler flags changed, configuring again", flush=True)
        (bdir / "CMakeCache.txt").unlink()
    if extra:
        env["CFLAGS"] = (env.get("CFLAGS", "") + " " + extra).strip()
        env["CXXFLAGS"] = (env.get("CXXFLAGS", "") + " " + extra).strip()
        print(f"+ CFLAGS={env['CFLAGS']!r}", flush=True)
    print("+", " ".join(args), flush=True)
    subprocess.run(args, check=True, env=env)
    record.write_text(extra or "")
    subprocess.run([cmake, "--build", str(bdir), "--config", "Release", "--target", "llama-server", "-j", str(jobs)], check=True)
    exe = "llama-server.exe" if os_name == "windows" else "llama-server"
    for cand in [bdir / "bin" / "Release" / exe, bdir / "bin" / exe]:
        if cand.exists():
            return cand
    sys.exit(f"built, but {exe} not found under {bdir}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--variants", default="avx2,avx,ssse3,compat")
    ap.add_argument("--cmake", default=shutil.which("cmake") or "cmake")
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 2)
    ap.add_argument("--cache", default=str(Path.home() / ".cache" / "trurl"))
    ap.add_argument("--out", default=str(ROOT / "vendor" / "llama"))
    args = ap.parse_args()

    os_name = host_os()
    src = fetch_source(Path(args.cache))
    build_root = Path(args.cache) / f"build-{os_name}-{PINS['llama_cpp']['commit'][:12]}"
    out_root = Path(args.out) / os_name
    manifest_path = out_root / "BUILD.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {}
    manifest.update({"commit": PINS["llama_cpp"]["commit"], "release": PINS["llama_cpp"]["release"], "os": os_name})
    manifest.setdefault("variants", {})
    for v in [x.strip() for x in args.variants.split(",") if x.strip()]:
        if v not in VARIANTS:
            sys.exit(f"unknown variant {v}")
        exe = build(src, build_root, v, args.cmake, args.jobs, os_name)
        dest = out_root / v
        dest.mkdir(parents=True, exist_ok=True)
        target = dest / exe.name
        shutil.copy2(exe, target)
        manifest["variants"][v] = {
            "flags": VARIANTS[v],
            "cflags": VARIANT_CFLAGS.get(v, {}).get(os_name, ""),
            "sha256": sha256(target),
            "size": target.stat().st_size,
        }
        print(f"{v}: {target} ({target.stat().st_size // 1024} KB)", flush=True)
    shutil.copy2(src / "LICENSE", out_root / "LICENSE-llama.cpp.txt")
    manifest["common_flags"] = COMMON
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"wrote {manifest_path}")


if __name__ == "__main__":
    main()
