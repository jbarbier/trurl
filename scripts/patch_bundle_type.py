#!/usr/bin/env python3
"""Mark trurl.exe as "installed by NSIS", the way `tauri bundle` does, but before
the file is code-signed.

Tauri's updater reads a 27-byte marker in the main executable to know which
installer to download. `tauri bundle` writes that marker itself, which would
break a signature already on the file, and then puts the unmarked file back.
So the release builds with `tauri build --no-bundle`, runs this, signs
trurl.exe, then bundles with `tauri bundle --no-binary-patching`.

Same rule as tauri-bundler 2.x (`patch_binary` in crates/tauri-bundler/src/bundle.rs):
replace the first `__TAURI_BUNDLE_TYPE_VAR_UNK` with the bundle type's marker.
A file already marked for that type is left as it is.

  python scripts/patch_bundle_type.py target/release/trurl.exe nsis
  python scripts/patch_bundle_type.py --self-test
"""

from __future__ import annotations

import argparse
import sys
import tempfile
from pathlib import Path

UNKNOWN = b"__TAURI_BUNDLE_TYPE_VAR_UNK"
MARKERS = {
    "nsis": b"__TAURI_BUNDLE_TYPE_VAR_NSS",
    "msi": b"__TAURI_BUNDLE_TYPE_VAR_MSI",
}


def patch(data: bytes, bundle: str) -> bytes:
    marker = MARKERS[bundle]
    i = data.find(UNKNOWN)
    if i == -1:
        if marker in data:
            return data
        raise ValueError("no bundle type marker in this file: is it a Tauri 2 app built for release?")
    return data[:i] + marker + data[i + len(UNKNOWN):]


def self_test() -> int:
    exe = b"MZ...code..." + UNKNOWN + b"...more..." + UNKNOWN
    once = patch(exe, "nsis")
    assert len(once) == len(exe)
    assert once.count(MARKERS["nsis"]) == 1 and once.count(UNKNOWN) == 1, "only the first marker, like Tauri"
    assert once.index(MARKERS["nsis"]) == exe.index(UNKNOWN)
    marked = b"MZ" + MARKERS["nsis"]
    assert patch(marked, "nsis") == marked, "already marked: unchanged"
    try:
        patch(b"MZ no marker", "nsis")
        raise AssertionError("a file without a marker must fail")
    except ValueError:
        pass
    with tempfile.TemporaryDirectory() as d:
        p = Path(d) / "trurl.exe"
        p.write_bytes(exe)
        assert main([str(p), "nsis"]) == 0
        assert p.read_bytes() == once
    print("patch_bundle_type: self-test passed")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("exe", nargs="?")
    ap.add_argument("bundle", nargs="?", choices=sorted(MARKERS))
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args(argv)
    if a.self_test:
        return self_test()
    if not a.exe or not a.bundle:
        ap.error("give the executable and the bundle type")
    path = Path(a.exe)
    data = path.read_bytes()
    try:
        out = patch(data, a.bundle)
    except ValueError as e:
        print(f"patch_bundle_type: {path}: {e}", file=sys.stderr)
        return 1
    if out != data:
        path.write_bytes(out)
    print(f"{path}: marked as installed by {a.bundle}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
