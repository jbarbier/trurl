#!/usr/bin/env python3
"""Voice gate for everything a learner or reader sees.

Checks the desktop UI (HTML and the strings in its JS), the learner-facing
strings in Rust (errors, terminal app), the installer messages and the docs:
  * no em dashes (U+2014)
  * "learner", never "student"
  * none of the banned words (see CLAUDE.md)

  python3 scripts/check_copy.py          # exit 1 and list every hit
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BANNED = [
    "delve", "crucial", "robust", "comprehensive", "nuanced", "multifaceted", "furthermore",
    "moreover", "pivotal", "landscape", "tapestry", "underscore", "foster", "showcase",
    "intricate", "vibrant", "fundamental", "significant", "interplay",
]
PHRASES = ["here's the kicker", "here's the thing", "plot twist", "let me break this down", "the bottom line", "make no mistake"]

STRING = re.compile(r'"((?:[^"\\\n]|\\.)*)"')
JS_STRING = re.compile(r'"((?:[^"\\\n]|\\.)*)"|\'((?:[^\'\\\n]|\\.)*)\'|`((?:[^`\\]|\\.)*)`', re.S)


def rust_strings(text: str) -> list[tuple[int, str]]:
    out = []
    for i, line in enumerate(text.splitlines(), 1):
        code = line.split("//", 1)[0] if '"' not in line.split("//", 1)[0] or "://" not in line else line
        for m in STRING.finditer(code):
            out.append((i, m.group(1)))
    return out


def js_strings(text: str) -> list[tuple[int, str]]:
    out = []
    for m in JS_STRING.finditer(text):
        s = next(g for g in m.groups() if g is not None)
        line = text.count("\n", 0, m.start()) + 1
        out.append((line, s))
    return out


def html_text(text: str) -> list[tuple[int, str]]:
    no_script = re.sub(r"<(script|style)[^>]*>.*?</\1>", lambda m: "\n" * m.group(0).count("\n"), text, flags=re.S | re.I)
    out = []
    for i, line in enumerate(no_script.splitlines(), 1):
        visible = re.sub(r"<[^>]+>", " ", line)
        attrs = re.findall(r'(?:aria-label|title|placeholder|alt)="([^"]*)"', line)
        out.append((i, visible + " " + " ".join(attrs)))
    return out


def check(path: Path, chunks: list[tuple[int, str]], problems: list[str]) -> None:
    for line, s in chunks:
        low = s.lower()
        if "—" in s:
            problems.append(f"{path.relative_to(ROOT)}:{line}: em dash: {s.strip()[:80]}")
        if re.search(r"\bstudents?\b", low) and 'never "student"' not in low:
            problems.append(f"{path.relative_to(ROOT)}:{line}: say 'learner': {s.strip()[:80]}")
        for w in BANNED:
            if re.search(rf"\b{w}\w*\b", low):
                problems.append(f"{path.relative_to(ROOT)}:{line}: banned word '{w}': {s.strip()[:80]}")
        for p in PHRASES:
            if p in low:
                problems.append(f"{path.relative_to(ROOT)}:{line}: banned phrase '{p}'")


def main() -> int:
    problems: list[str] = []
    ui = ROOT / "apps" / "desktop" / "ui"
    for p in sorted(ui.glob("*.html")):
        check(p, html_text(p.read_text(encoding="utf-8")), problems)
    for p in sorted(ui.glob("*.js")):
        check(p, js_strings(p.read_text(encoding="utf-8")), problems)
    rust = [ROOT / "crates/trurl-core/src/errors.rs"] + sorted((ROOT / "apps/tui/src").glob("*.rs"))
    for p in rust:
        check(p, rust_strings(p.read_text(encoding="utf-8")), problems)
    nsh = ROOT / "apps/desktop/src-tauri/installer.nsh"
    check(nsh, [(i, l) for i, l in enumerate(nsh.read_text().splitlines(), 1) if "MessageBox" in l or "DetailPrint" in l], problems)
    docs = [ROOT / "README.md", ROOT / "CLAUDE.md"] + sorted((ROOT / "docs").rglob("*.md"))
    for p in docs:
        if p.exists():
            check(p, list(enumerate(p.read_text(encoding="utf-8").splitlines(), 1)), problems)
    for line in problems:
        print(line)
    print(f"check_copy: {len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
