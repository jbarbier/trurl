#!/usr/bin/env python3
"""Fail if a llama-server build needs a library a learner's computer may not have.

Windows (.exe): reads the PE import table directly (no tools needed). Allowed:
DLLs that ship with every Windows. Not allowed: the Visual C++ runtime
(MSVCP140, VCRUNTIME140, api-ms-win-crt-*, ucrtbase), OpenMP, OpenSSL, or any
ggml/llama DLL: our builds are single static executables.

Linux (ELF): reads DT_NEEDED with `readelf -d`. Allowed: glibc's own libraries.

  python3 scripts/check_pe_imports.py vendor/llama/windows
  python3 scripts/check_pe_imports.py vendor/llama/linux
"""

from __future__ import annotations

import re
import struct
import subprocess
import sys
from pathlib import Path

WINDOWS_OK = {
    "kernel32.dll", "advapi32.dll", "ws2_32.dll", "shell32.dll", "user32.dll", "ole32.dll",
    "oleaut32.dll", "crypt32.dll", "bcrypt.dll", "ntdll.dll", "psapi.dll", "shlwapi.dll",
    "iphlpapi.dll", "userenv.dll", "secur32.dll", "rpcrt4.dll", "version.dll", "dbghelp.dll",
}
LINUX_OK = re.compile(r"^(libc|libm|libpthread|libdl|librt|ld-linux-x86-64)\.so(\.\d+)*$")


def pe_imports(path: Path) -> list[str]:
    data = path.read_bytes()
    if data[:2] != b"MZ":
        raise ValueError("not a PE file")
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe:pe + 4] != b"PE\0\0":
        raise ValueError("bad PE signature")
    n_sections = struct.unpack_from("<H", data, pe + 6)[0]
    opt_size = struct.unpack_from("<H", data, pe + 20)[0]
    opt = pe + 24
    magic = struct.unpack_from("<H", data, opt)[0]
    dd = opt + (112 if magic == 0x20B else 96)  # data directories (PE32+ vs PE32)
    imp_rva, _ = struct.unpack_from("<II", data, dd + 8)  # entry 1: import table
    sections = []
    sec = opt + opt_size
    for i in range(n_sections):
        vsize, vaddr, rawsize, rawptr = struct.unpack_from("<IIII", data, sec + i * 40 + 8)
        sections.append((vaddr, max(vsize, rawsize), rawptr))

    def off(rva: int) -> int:
        for vaddr, size, rawptr in sections:
            if vaddr <= rva < vaddr + size:
                return rva - vaddr + rawptr
        raise ValueError(f"rva {rva:#x} outside sections")

    names = []
    if imp_rva == 0:
        return names
    p = off(imp_rva)
    while True:
        oft, _, _, name_rva, ft = struct.unpack_from("<IIIII", data, p)
        if name_rva == 0 and oft == 0 and ft == 0:
            break
        q = off(name_rva)
        names.append(data[q:data.index(b"\0", q)].decode("ascii", "replace"))
        p += 20
    return names


def elf_needed(path: Path) -> list[str]:
    out = subprocess.run(["readelf", "-d", str(path)], capture_output=True, text=True, check=True).stdout
    return re.findall(r"\(NEEDED\)\s+Shared library: \[([^\]]+)\]", out)


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    root = Path(sys.argv[1])
    bad = 0
    found = 0
    for exe in sorted(root.rglob("llama-server*")):
        if exe.suffix not in ("", ".exe") or not exe.is_file():
            continue
        found += 1
        if exe.read_bytes()[:2] == b"MZ":
            deps = pe_imports(exe)
            wrong = [d for d in deps if d.lower() not in WINDOWS_OK]
        else:
            deps = elf_needed(exe)
            wrong = [d for d in deps if not LINUX_OK.match(d)]
        status = "FAIL" if wrong else "ok  "
        print(f"{status} {exe}: {', '.join(deps)}")
        if wrong:
            print(f"     not allowed: {', '.join(wrong)}")
            bad += 1
    if found == 0:
        print(f"no llama-server found under {root}")
        return 1
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
