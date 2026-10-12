#!/usr/bin/env python3
"""Set Trurl's version everywhere it lives, in one go.

  python3 scripts/bump_version.py 0.2.0

Updates pins.json (app.version), Cargo.toml ([workspace.package] version),
apps/desktop/src-tauri/tauri.conf.json (version) and Cargo.lock (the
workspace crates). scripts/check_pins.py fails if these ever disagree.
Remember the release notes: a "## 0.2.0" section in CHANGELOG.md.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKSPACE_CRATES = ["trurl-core", "trurl-tui", "trurl-bridge", "trurl-desktop"]


def main() -> int:
    if len(sys.argv) != 2 or not re.fullmatch(r"\d+\.\d+\.\d+", sys.argv[1]):
        print(__doc__)
        return 2
    new = sys.argv[1]

    pins_path = ROOT / "pins.json"
    pins = json.loads(pins_path.read_text())
    old = pins["app"]["version"]
    pins["app"]["version"] = new
    pins_path.write_text(json.dumps(pins, indent=2) + "\n")

    cargo = ROOT / "Cargo.toml"
    text = cargo.read_text()
    text, n = re.subn(r'(\[workspace\.package\][^\[]*?\nversion = )"[^"]+"', rf'\g<1>"{new}"', text, count=1)
    if n != 1:
        sys.exit("could not find [workspace.package] version in Cargo.toml")
    cargo.write_text(text)

    conf_path = ROOT / "apps/desktop/src-tauri/tauri.conf.json"
    conf = json.loads(conf_path.read_text())
    conf["version"] = new
    conf_path.write_text(json.dumps(conf, indent=2) + "\n")

    lock = ROOT / "Cargo.lock"
    if lock.exists():
        text = lock.read_text()
        for name in WORKSPACE_CRATES:
            text = re.sub(rf'(name = "{re.escape(name)}"\nversion = )"[^"]+"', rf'\g<1>"{new}"', text)
        lock.write_text(text)

    changelog = (ROOT / "CHANGELOG.md").read_text()
    if f"## {new}" not in changelog:
        print(f"note: add a '## {new}' section to CHANGELOG.md (the update prompt shows it)")
    print(f"version {old} -> {new}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
