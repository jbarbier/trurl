#!/usr/bin/env python3
"""Prove each llama-server build only uses instructions its CPUs have.

Disassembles vendor/llama/<os>/{compat,avx,avx2}/llama-server and counts the
instructions that would crash an older CPU with "illegal instruction":

  compat  must contain no AVX at all (no VEX-encoded ymm/xmm ops): runs on any x86-64
  avx     may use AVX, but no AVX2 integer ops, FMA or F16C: Sandy/Ivy Bridge
  avx2    may use AVX2 + FMA + F16C, but no AVX-512 (zmm registers)

Two modes:

  python3 scripts/check_cpu_isa.py vendor/llama/linux
      the finished executables (Linux: libstdc++ and libgcc are linked in and
      must follow the rule too)

  python3 scripts/check_cpu_isa.py --objects ~/.cache/trurl/build-windows-<commit>
      llama.cpp's own compiled libraries for each tier (Windows): the static
      Microsoft C runtime inside the .exe carries FMA/AVX versions of math and
      memcpy routines that it only uses after checking the CPU at run time, so
      on Windows the executables are proven by scripts/check_cpu_emulated.py
      (Intel SDE runs each build on an emulated older CPU) and the code we
      compile is proven here.

Needs `objdump` (binutils), or `llvm-objdump` via OBJDUMP=... on Windows.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path

PATTERNS = {
    "avx (ymm registers)": re.compile(r"%ymm\d|\bymm\d"),
    "avx-512 (zmm registers)": re.compile(r"%zmm\d|\bzmm\d|%k[1-7]\b"),
    "fma": re.compile(r"\tvfn?m(add|sub)"),
    "f16c": re.compile(r"\tvcvt(ph2ps|ps2ph)"),
    "avx2 integer": re.compile(r"\tvp(broadcast[bwdq]|perm[dq]|maddubsw|maddwd|sllv[dq]|srlv[dq]|gather)\S*\s.*ymm"),
    "vex (any avx encoding)": re.compile(r"\tv(mov|add|sub|mul|xor|and|or|pxor|pand|padd|broadcast|perm|shuf|cvt)\w*\s"),
}
FORBIDDEN = {
    "compat": ["avx (ymm registers)", "avx-512 (zmm registers)", "fma", "f16c", "avx2 integer", "vex (any avx encoding)"],
    "avx": ["avx-512 (zmm registers)", "fma", "f16c", "avx2 integer"],
    "avx2": ["avx-512 (zmm registers)"],
}


def scan(exe: Path) -> Counter:
    counts: Counter = Counter()
    tool = os.environ.get("OBJDUMP", "objdump")  # llvm-objdump works too (Windows CI)
    proc = subprocess.Popen([tool, "-d", "--no-show-raw-insn", str(exe)], stdout=subprocess.PIPE, text=True, errors="replace")
    assert proc.stdout
    for line in proc.stdout:
        if "\tv" not in line and "mm" not in line:
            continue
        for name, rx in PATTERNS.items():
            if rx.search(line):
                counts[name] += 1
    proc.wait()
    return counts


def objects(build_root: Path, variant: str) -> list[Path]:
    d = build_root / variant
    return sorted(p for p in d.rglob("*") if p.suffix.lower() in (".lib", ".a", ".obj", ".o") and p.is_file())


def main() -> int:
    args = [a for a in sys.argv[1:] if a != "--objects"]
    object_mode = "--objects" in sys.argv
    root = Path(args[0] if args else "vendor/llama/linux").expanduser()
    bad = 0
    for variant, forbidden in FORBIDDEN.items():
        if object_mode:
            files = objects(root, variant)
            if not files:
                print(f"skip {variant}: no build folder under {root}")
                continue
            c = Counter()
            for f in files:
                c.update(scan(f))
            exe = root / variant
        else:
            exe = next((p for p in [root / variant / "llama-server", root / variant / "llama-server.exe"] if p.exists()), None)
            if not exe:
                print(f"skip {variant}: not built")
                continue
            c = scan(exe)
        found = {k: c[k] for k in forbidden if c[k]}
        summary = ", ".join(f"{k}={c[k]}" for k in PATTERNS)
        if found:
            bad += 1
            print(f"FAIL {variant}: uses {found}  ({summary})")
        else:
            print(f"ok   {variant}: {summary}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
