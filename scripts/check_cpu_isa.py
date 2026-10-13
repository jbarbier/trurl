#!/usr/bin/env python3
"""Prove each llama-server build only uses instructions its CPUs have.

Disassembles vendor/llama/<os>/{compat,ssse3,avx,avx2}/llama-server and counts the
instructions that would crash an older CPU with "illegal instruction":

  compat  SSE2 only: no SSSE3, SSE4, POPCNT or AVX: runs on any x86-64 (Pentium 4)
  ssse3   must use SSSE3, but no SSE4.1/4.2, POPCNT or AVX: Core 2 (Merom), Atom
  avx     must use AVX, but no AVX2 integer ops, FMA or F16C: Sandy/Ivy Bridge
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

  python3 scripts/check_cpu_isa.py --self-test
      compiles one small function per instruction set with gcc and checks that
      each pattern below catches its own set and nothing else (run by gates.sh)

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
    # SSE4.1, SSE4.2 and POPCNT (Penryn and Nehalem, 2008): absent on a Core 2 Merom
    # and an Atom N270. VEX forms are caught by "vex" below.
    "sse4/popcnt": re.compile(
        r"\t(pmulld|pmuldq|pblendvb|blendvp[sd]|blendp[sd]|pblendw|ptest|pmin(sb|sd|uw|ud)|pmax(sb|sd|uw|ud)|round[ps][sd]"
        r"|pinsr[bdq]|pextr[bdq]|pmov[sz]x\w+|insertps|extractps|dpp[sd]|mpsadbw|phminposuw|packusdw"
        r"|pcmpeqq|movntdqa|pcmpgtq|crc32\w*|pcmp[ei]str[im]|popcnt)\b"
    ),
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
    "compat": ["ssse3", "sse4/popcnt", "avx (ymm registers)", "avx-512 (zmm registers)", "fma", "f16c", "avx2 integer", "vex (any avx encoding)"],
    "ssse3": ["sse4/popcnt", "avx (ymm registers)", "avx-512 (zmm registers)", "fma", "f16c", "avx2 integer", "vex (any avx encoding)"],
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
        if "\tv" not in line and "mm" not in line and "popcnt" not in line and "crc32" not in line:
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


# One function per instruction set, the flags that allow it, and the pattern
# kinds it must trigger. Everything else must stay at zero.
SELF_TEST = [
    ("sse2", "-msse2", "#include <emmintrin.h>\n__m128i f(__m128i a, __m128i b) { return _mm_min_epu8(_mm_add_epi16(a, b), b); }", set()),
    ("ssse3", "-mssse3", "#include <tmmintrin.h>\n__m128i f(__m128i a, __m128i b) { return _mm_maddubs_epi16(_mm_shuffle_epi8(a, b), b); }", {"ssse3"}),
    ("sse4.1", "-msse4.1", "#include <smmintrin.h>\n__m128i f(__m128i a, __m128i b) { return _mm_mullo_epi32(a, b); }", {"sse4/popcnt"}),
    ("sse4.2", "-msse4.2", "#include <nmmintrin.h>\nunsigned f(unsigned a, unsigned b) { return _mm_crc32_u32(a, b); }", {"sse4/popcnt"}),
    ("popcnt", "-mpopcnt", "int f(unsigned a) { return __builtin_popcount(a); }", {"sse4/popcnt"}),
    ("avx", "-mavx", "#include <immintrin.h>\n__m256 f(__m256 a, __m256 b) { return _mm256_add_ps(a, b); }", {"avx (ymm registers)", "vex (any avx encoding)"}),
    ("avx2", "-mavx2", "#include <immintrin.h>\n__m256i f(__m256i a, __m256i b) { return _mm256_maddubs_epi16(a, b); }", {"ssse3", "avx (ymm registers)", "avx2 integer"}),
    ("fma", "-mavx -mfma", "#include <immintrin.h>\n__m256 f(__m256 a, __m256 b, __m256 c) { return _mm256_fmadd_ps(a, b, c); }", {"avx (ymm registers)", "fma"}),
    ("f16c", "-mavx -mf16c", "#include <immintrin.h>\n__m256 f(__m128i a) { return _mm256_cvtph_ps(a); }", {"avx (ymm registers)", "f16c", "vex (any avx encoding)"}),
    ("avx512", "-mavx512f", "#include <immintrin.h>\n__m512 f(__m512 a, __m512 b) { return _mm512_add_ps(a, b); }", {"avx-512 (zmm registers)", "vex (any avx encoding)"}),
]


def self_test() -> int:
    import tempfile

    bad = 0
    with tempfile.TemporaryDirectory() as tmp:
        for name, flags, code, expect in SELF_TEST:
            src, obj = Path(tmp, name + ".c"), Path(tmp, name + ".o")
            src.write_text(code + "\n")
            subprocess.run(["gcc", "-O2", "-c", *flags.split(), str(src), "-o", str(obj)], check=True)
            got = {k for k, n in scan(obj).items() if n}
            # The "vex" kind is a catch-all for AVX encodings: allowed, not required.
            missing = expect - got - {"vex (any avx encoding)"}
            extra = got - expect - ({"vex (any avx encoding)"} if "avx" in flags else set())
            if missing or extra:
                bad += 1
                print(f"FAIL self-test {name}: missed {sorted(missing)}, wrongly found {sorted(extra)}")
            else:
                print(f"ok   self-test {name}: {sorted(got)}")
    return 1 if bad else 0


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()
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
        elif not missing:
            print(f"ok   {variant}: {summary}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
