#!/usr/bin/env python3
"""Screenshots of the terminal app, for the README, from the real program.

Runs `trurl` inside tmux (120x34), plays a short scripted session against the
real core and real models, captures each screen with its colours, renders it as
HTML and photographs it with Chromium (the e2e Playwright install).

  python3 scripts/tui_screens.py --models ~/.cache/trurl/models --out docs/img
"""

from __future__ import annotations

import argparse
import html
import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
COLS, ROWS = 120, 34
# The 16 basic colours, as the Windows Terminal "Campbell" scheme draws them.
PALETTE = ["#0C0C0C", "#C50F1F", "#13A10E", "#C19C00", "#0037DA", "#881798", "#3A96DD", "#CCCCCC",
           "#767676", "#E74856", "#16C60C", "#F9F1A5", "#3B78FF", "#B4009E", "#61D6D6", "#F2F2F2"]
BG, FG = "#0C0C0C", "#CCCCCC"


def tmux(*args: str) -> str:
    return subprocess.run(["tmux", "-L", "trurl-shots", *args], capture_output=True, text=True, check=True).stdout


def ansi_to_html(text: str) -> str:
    out = []
    fg, bg, bold, italic, rev = FG, None, False, False, False
    pos = 0
    for m in re.finditer(r"\x1b\[([0-9;]*)m", text):
        out.append(span(text[pos:m.start()], fg, bg, bold, italic, rev))
        pos = m.end()
        codes = [int(c) if c else 0 for c in m.group(1).split(";")]
        i = 0
        while i < len(codes):
            c = codes[i]
            if c == 0:
                fg, bg, bold, italic, rev = FG, None, False, False, False
            elif c == 1:
                bold = True
            elif c == 3:
                italic = True
            elif c == 7:
                rev = True
            elif c == 22:
                bold = False
            elif c == 23:
                italic = False
            elif c == 27:
                rev = False
            elif 30 <= c <= 37:
                fg = PALETTE[c - 30]
            elif 90 <= c <= 97:
                fg = PALETTE[c - 90 + 8]
            elif c == 39:
                fg = FG
            elif 40 <= c <= 47:
                bg = PALETTE[c - 40]
            elif 100 <= c <= 107:
                bg = PALETTE[c - 100 + 8]
            elif c == 49:
                bg = None
            elif c in (38, 48) and i + 2 < len(codes) and codes[i + 1] == 5:
                col = PALETTE[codes[i + 2]] if codes[i + 2] < 16 else FG
                if c == 38:
                    fg = col
                else:
                    bg = col
                i += 2
            i += 1
    out.append(span(text[pos:], fg, bg, bold, italic, rev))
    return "".join(out)


def span(t: str, fg, bg, bold, italic, rev) -> str:
    if not t:
        return ""
    f, b = (bg or BG, fg) if rev else (fg, bg)
    style = f"color:{f};" + (f"background:{b};" if b else "") + ("font-weight:700;" if bold else "") + ("font-style:italic;" if italic else "")
    return f'<span style="{style}">{html.escape(t)}</span>'


def capture(name: str, out: Path, pages: list) -> None:
    raw = tmux("capture-pane", "-p", "-e", "-t", "shots")
    body = ansi_to_html(raw)
    doc = (f'<html><body style="margin:0;background:{BG}"><div style="padding:14px 16px;background:{BG};'
           f"font:15px/1.32 'DejaVu Sans Mono','Cascadia Mono',Consolas,monospace;color:{FG};white-space:pre;"
           f'display:inline-block">{body}</div></body></html>')
    p = out / f"{name}.html"
    p.write_text(doc)
    pages.append((name, p))


def send(*keys: str, wait: float = 0.6) -> None:
    for k in keys:
        if k.startswith("TEXT:"):
            tmux("send-keys", "-t", "shots", "-l", k[5:])
        else:
            tmux("send-keys", "-t", "shots", k)
    time.sleep(wait)


def wait_for(text: str, timeout: float = 300) -> None:
    t0 = time.time()
    while time.time() - t0 < timeout:
        if text in tmux("capture-pane", "-p", "-t", "shots"):
            return
        time.sleep(0.5)
    raise SystemExit(f"timed out waiting for {text!r}")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--models", required=True)
    ap.add_argument("--out", default=str(ROOT / "docs" / "img"))
    ap.add_argument("--bin", default=str(ROOT / "target" / "release" / "trurl"))
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    home = Path(tempfile.mkdtemp(prefix="trurl-shots-"))
    (home / "models").mkdir()
    for f in ["gemma-3-1b-it-Q4_K_M.gguf", "Qwen_Qwen3-0.6B-Q4_K_M.gguf"]:
        os.link(Path(a.models).expanduser() / f, home / "models" / f)
    env = f"TRURL_HOME={home} TRURL_LLAMA_DIR={ROOT / 'vendor' / 'llama' / 'linux'} TERM=xterm-256color"
    subprocess.run(["tmux", "-L", "trurl-shots", "kill-server"], capture_output=True)
    tmux("new-session", "-d", "-s", "shots", "-x", str(COLS), "-y", str(ROWS), f"env {env} {a.bin}")
    pages: list = []
    tmpdir = Path(tempfile.mkdtemp(prefix="trurl-shots-html-"))
    try:
        wait_for("Ready")
        send("TEXT:Explain what a language model is, in two short sentences.", "Enter")
        wait_for("Ready", 300)
        time.sleep(1)
        send("F2", wait=1)
        capture("tui-models", tmpdir, pages)
        send("Down", "Enter", wait=1)
        wait_for("Ready", 300)
        send("TEXT:What is 17 + 25? Reply with the number.", "Enter", wait=4)
        capture("tui-thinking", tmpdir, pages)
        wait_for("Thought for", 600)
        wait_for("Ready", 600)
        capture("tui-chat", tmpdir, pages)
        send("F3", wait=1)
        capture("tui-settings", tmpdir, pages)
        send("Escape", "F1", wait=1)
        capture("tui-about", tmpdir, pages)
    finally:
        subprocess.run(["tmux", "-L", "trurl-shots", "kill-server"], capture_output=True)
    # Photograph the HTML pages.
    e2e = ROOT / "apps" / "bridge" / "e2e"
    script = e2e / ".tui-shot.mjs"
    script.write_text(
        "import { chromium } from 'playwright';\n"
        f"const pages = {json.dumps([[n, str(p)] for n, p in pages])};\n"
        "const b = await chromium.launch(); const pg = await b.newPage({ deviceScaleFactor: 1 });\n"
        f"for (const [n, p] of pages) {{ await pg.goto('file://' + p); const el = await pg.$('div'); await el.screenshot({{ path: {json.dumps(str(out))} + '/' + n + '.png' }}); }}\n"
        "await b.close();\n"
    )
    try:
        subprocess.run(["node", str(script)], cwd=e2e, check=True)
    finally:
        script.unlink()
        shutil.rmtree(home, ignore_errors=True)
    print("wrote", ", ".join(f"{n}.png" for n, _ in pages))


if __name__ == "__main__":
    main()
