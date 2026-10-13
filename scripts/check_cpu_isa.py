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
    # SSSE3-only instructions (pmaddubsw, pshufb, palignr, phadd/phsub, psign, pabs, pmulhrsw).
    "ssse3": re.compile(r"\t(v?pmaddubsw|v?pshufb|v?palignr|v?phadd[wd]|v?phsub[wd]|v?psign[bwd]|v?pabs[bwd]|v?pmulhrsw)\b"),
    "avx (ymm registers)": re.compile(r"%ymm\d|\bymm\d"),
    "avx-512 (zmm registers)": re.compile(r"%zmm\d|\bzmm\d|%k[1-7]\b"),
    "fma": re.compile(r"\tvfn?m(add|sub)"),
    "f16c": re.compile(r"\tvcvt(ph2ps|ps2ph)"),
    "avx2 integer": re.compile(r"\tvp(broadcast[bwdq]|perm[dq]|maddubsw|maddwd|sllv[dq]|srlv[dq]|gather)\S*\s.*ymm"),
    "vex (any avx encoding)": re.compile(r"\tv(mov|add|sub|mul|xor|and|or|pxor|pand|padd|broadcast|perm|shuf|cvt)\w*\s"),
}
# What each tier must contain: a build whose flags were silently dropped would
# pass every "forbidden" check and still be slow. These catch that.
REQUIRED = {
    "ssse3": ["ssse3"],
    "avx": ["avx (ymm registers)"],
    "avx2": ["fma", "avx2 integer"],
}

FORBIDDEN = {
    "compat": ["ssse3", "avx (ymm registers)", "avx-512 (zmm registers)", "fma", "f16c", "avx2 integer", "vex (any avx encoding)"],
    "ssse3": ["avx (ymm registers)", "avx-512 (zmm registers)", "fma", "f16c", "avx2 integer", "vex (any avx encoding)"],
    "avx": ["avx-512 (zmm registers)", "fma", "f16c", "avx2 integer"],
    "avx2": ["avx-512 (zmm registers)"],
}


SYMBOL = re.compile(r"^[0-9a-fA-F]+ <(.+)>:\s*$")


# C runtime routines that Microsoft's compiler expands inline with an AVX path
# chosen at run time (`__isa_available`), seen in llama.cpp's objects.
CRT_DISPATCHED = {"wmemcmp", "memcmp", "wmemchr", "memchr", "wcslen", "strlen", "memcpy", "memmove", "memset"}


def is_std(symbol: str) -> bool:
    """A function of the C/C++ runtime library (MSVC-mangled or demangled)."""
    return (
        "@std@@" in symbol
        or symbol.startswith("std::")
        or symbol.startswith("__std_")
        or symbol in CRT_DISPATCHED
    )


def scan(exe: Path, std_hits: Counter | None = None, other_hits: Counter | None = None) -> Counter:
    """Count each kind of instruction. With `std_hits`, instructions inside C++
    standard library functions go there instead (see main)."""
    counts: Counter = Counter()
    tool = os.environ.get("OBJDUMP", "objdump")  # llvm-objdump works too (Windows CI)
    proc = subprocess.Popen([tool, "-d", "--no-show-raw-insn", str(exe)], stdout=subprocess.PIPE, text=True, errors="replace")
    assert proc.stdout
    symbol = ""
    for line in proc.stdout:
        m = SYMBOL.match(line)
        if m:
            symbol = m.group(1)
            continue
        if "\tv" not in line and "mm" not in line:
            continue
        for name, rx in PATTERNS.items():
            if rx.search(line):
                if std_hits is not None and is_std(symbol):
                    std_hits[(symbol, name)] += 1
                else:
                    counts[name] += 1
                    if other_hits is not None:
                        other_hits[(symbol, name)] += 1
    proc.wait()
    return counts


def objects(build_root: Path, variant: str) -> list[Path]:
    """llama.cpp's compiled code for one tier: the object files (the .lib/.a
    archives only repeat them). CMake's own compiler probes are not ours."""
    d = build_root / variant
    return sorted(
        p
        for p in d.rglob("*")
        if p.suffix.lower() in (".obj", ".o") and p.is_file() and "CompilerId" not in str(p) and "CMakeScratch" not in str(p)
    )


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
            per_file = {}
            # Microsoft's C and C++ runtime (inlined from its headers into every
            # file that uses it) has AVX versions of a few routines and picks them
            # only after asking the CPU (`__isa_available`). They are listed by
            # name, not failed: check_cpu_emulated.py runs the whole .exe on an
            # old CPU. llama.cpp's own code never gets this pass.
            std_hits: Counter = Counter()
            other_hits: Counter = Counter()
            for f in files:
                fc = scan(f, std_hits, other_hits)
                c.update(fc)
                hits = sum(fc[k] for k in forbidden)
                if hits:
                    per_file[f] = (hits, dict((k, fc[k]) for k in forbidden if fc[k]))
            exe = root / variant
        else:
            exe = next((p for p in [root / variant / "llama-server", root / variant / "llama-server.exe"] if p.exists()), None)
            if not exe:
                print(f"skip {variant}: not built")
                continue
            c = scan(exe)
        found = {k: c[k] for k in forbidden if c[k]}
        missing = [k for k in REQUIRED.get(variant, []) if not c[k]]
        summary = ", ".join(f"{k}={c[k]}" for k in PATTERNS)
        if missing:
            bad += 1
            print(f"FAIL {variant}: none of its own instructions ({', '.join(missing)}): was it built with its flags?  ({summary})")
            continue
        if object_mode and std_hits:
            names = sorted({sym for (sym, kind) in std_hits if kind in forbidden})
            if names:
                print(f"note {variant}: C/C++ runtime code with run-time CPU checks: {', '.join(names[:6])}")
        if found:
            bad += 1
            print(f"FAIL {variant}: uses {found}  ({summary})")
            if object_mode:
                for f, (n, what) in sorted(per_file.items(), key=lambda kv: -kv[1][0])[:15]:
                    print(f"       {n:5d}  {f.relative_to(root)}  {what}")
                funcs = Counter()
                for (sym, kind), n in other_hits.items():
                    if kind in forbidden:
                        funcs[sym] += n
                for sym, n in funcs.most_common(10):
                    print(f"       {n:5d}  in {sym}")
        else:
            print(f"ok   {variant}: {summary}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
