#!/usr/bin/env python3
"""Check that Linux programs run on an old C library (glibc).

The README promises Ubuntu 20.04, Debian 11 and Linux Mint 20 or newer: glibc
2.31. A program built on a newer system silently needs a newer glibc and fails
to start there ("version `GLIBC_2.34' not found"). This reads every versioned
glibc symbol each program needs and fails if one is newer than the limit.

  python3 scripts/check_glibc.py dist/trurl-0.1.0-linux-x64/   # a folder or files
  python3 scripts/check_glibc.py --max 2.31 target/release/trurl

Needs `objdump` (binutils).
"""
from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

GLIBC = re.compile(r"\bGLIBC_(\d+)\.(\d+)(?:\.(\d+))?\b")


def is_elf(p: Path) -> bool:
    try:
        with p.open("rb") as f:
            return f.read(4) == b"\x7fELF"
    except OSError:
        return False


def needs(p: Path) -> tuple[int, ...]:
    out = subprocess.run(["objdump", "-T", str(p)], capture_output=True, text=True).stdout
    versions = [tuple(int(x) for x in m.groups() if x is not None) for m in GLIBC.finditer(out)]
    return max(versions, default=(0,))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("paths", nargs="+")
    ap.add_argument("--max", default="2.31", help="newest glibc allowed (default 2.31: Ubuntu 20.04, Debian 11)")
    a = ap.parse_args()
    limit = tuple(int(x) for x in a.max.split("."))
    files = []
    for p in map(Path, a.paths):
        files += [f for f in p.rglob("*") if f.is_file() and is_elf(f)] if p.is_dir() else [p]
    if not files:
        print("no ELF programs found")
        return 1
    bad = 0
    for f in sorted(files):
        v = needs(f)
        shown = ".".join(map(str, v)) if v != (0,) else "none"
        if v > limit:
            bad += 1
            print(f"FAIL {f}: needs glibc {shown}, newer than {a.max}")
        else:
            print(f"ok   {f}: needs glibc {shown}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
