#!/usr/bin/env python3
"""Pins gate: the build never changes unless a maintainer changes it.

Fails if:
  * the app version differs between pins.json, Cargo.toml and tauri.conf.json;
  * the identifier or product name drifted (the uninstaller and the models
    folder location depend on them);
  * a direct Rust dependency is not pinned with "=x.y.z";
  * a vendored file (JS, font) does not match its SHA-256;
  * the README's pinned-versions table disagrees with pins.json;
  * a generated license file is stale (scripts/gen_licenses.py --check).

  python3 scripts/check_pins.py
"""

from __future__ import annotations

import hashlib
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    errors: list[str] = []
    pins = json.loads((ROOT / "pins.json").read_text())
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text())
    tauri = json.loads((ROOT / "apps/desktop/src-tauri/tauri.conf.json").read_text())

    v = pins["app"]["version"]
    if cargo["workspace"]["package"]["version"] != v:
        errors.append(f"Cargo.toml version {cargo['workspace']['package']['version']} != pins.json {v}")
    if tauri["version"] != v:
        errors.append(f"tauri.conf.json version {tauri['version']} != pins.json {v}")
    if tauri["identifier"] != pins["app"]["identifier"]:
        errors.append("tauri.conf.json identifier differs from pins.json")
    if tauri["productName"] != "Trurl":
        errors.append("productName must stay 'Trurl': the install folder and uninstaller depend on it")
    if tauri["identifier"].lower() in ("cyberiadlabs", "trurl"):
        errors.append("identifier must not equal a data folder name (the uninstaller deletes %LOCALAPPDATA%\\<identifier>)")

    toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    if toolchain != pins["rust"]:
        errors.append(f"rust-toolchain.toml {toolchain} != pins.json rust {pins['rust']}")

    def check_deps(table: dict, where: str) -> None:
        for name, spec in table.items():
            if isinstance(spec, dict) and (spec.get("path") or spec.get("workspace")):
                continue
            ver = spec if isinstance(spec, str) else spec.get("version", "")
            if not re.fullmatch(r"=\d+\.\d+\.\d+", ver):
                errors.append(f"{where}: {name} = {ver!r} is not pinned (use \"=x.y.z\")")

    check_deps(cargo["workspace"]["dependencies"], "Cargo.toml [workspace.dependencies]")
    for toml in ROOT.glob("**/Cargo.toml"):
        if "target" in toml.parts or toml == ROOT / "Cargo.toml":
            continue
        t = tomllib.loads(toml.read_text())
        for section in ("dependencies", "build-dependencies"):
            check_deps(t.get(section, {}), f"{toml.relative_to(ROOT)} [{section}]")
        for tgt in t.get("target", {}).values():
            check_deps(tgt.get("dependencies", {}), f"{toml.relative_to(ROOT)} [target dependencies]")

    for item in pins["vendored"]:
        p = ROOT / item["path"]
        if not p.exists():
            errors.append(f"missing vendored file {item['path']}")
            continue
        got = hashlib.sha256(p.read_bytes()).hexdigest()
        if got != item["sha256"]:
            errors.append(f"{item['path']}: sha256 {got} != pins.json {item['sha256']}")

    readme = (ROOT / "README.md").read_text() if (ROOT / "README.md").exists() else ""
    expected = [
        pins["llama_cpp"]["release"],
        pins["llama_cpp"]["build"],
        pins["llama_cpp"]["commit"][:12],
        f"Rust {pins['rust']}",
    ] + [m["file"] for m in pins["models"]] + [m["sha256"][:16] for m in pins["models"]]
    deps = cargo["workspace"]["dependencies"]
    desktop = tomllib.loads((ROOT / "apps/desktop/src-tauri/Cargo.toml").read_text())["dependencies"]
    expected.append("tauri " + desktop["tauri"]["version"].lstrip("="))
    expected.append("ratatui " + deps["ratatui"]["version"].lstrip("="))
    for item in pins["vendored"]:
        if item["version"][0].isdigit():
            expected.append(f"{item['package']} {item['version']}")
    for e in expected:
        if e not in readme:
            errors.append(f"README.md pinned-versions table is missing or out of date: {e!r}")

    r = subprocess.run([sys.executable, str(ROOT / "scripts/gen_licenses.py"), "--check"], capture_output=True, text=True)
    if r.returncode != 0:
        errors.append("generated licenses are stale:\n" + r.stdout.strip())

    for e in errors:
        print("FAIL", e)
    print(f"check_pins: {len(errors)} problem(s)")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
