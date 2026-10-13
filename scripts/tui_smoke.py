#!/usr/bin/env python3
"""The terminal app, end to end, as a learner uses it: the real `trurl` in tmux,
a real model, network blocked. Run by CI on every pull request.

Checks: it loads the model and chats offline; F6 copies the answer into the
notes file; a second `trurl` is refused; and when Trurl is killed (a crash),
its llama-server goes with it.

  python3 scripts/tui_smoke.py --models ~/.cache/trurl/models [--netblock target/netblock.so]
"""
from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SESSION = "trurl-smoke"
MODEL = "gemma-3-1b-it-Q4_K_M.gguf"


def tmux(*args: str) -> str:
    return subprocess.run(["tmux", *args], capture_output=True, text=True).stdout


def screen() -> str:
    return tmux("capture-pane", "-p", "-t", SESSION)


def wait_for(text: str, secs: float) -> str:
    t0 = time.time()
    while time.time() - t0 < secs:
        s = screen()
        if text in s:
            return s
        time.sleep(0.5)
    raise SystemExit(f"FAIL: never saw {text!r} on screen within {secs:.0f}s. Last screen:\n{screen()}")


def ours(home: Path) -> list[int]:
    """Processes started for this test (TRURL_HOME is our folder), and only those:
    another Trurl on the same computer is never touched."""
    found = []
    for d in Path("/proc").iterdir():
        if not d.name.isdigit():
            continue
        try:
            env = (d / "environ").read_bytes().split(b"\0")
        except OSError:
            continue
        if f"TRURL_HOME={home}".encode() in env:
            found.append(int(d.name))
    return found


def cmdline(pid: int) -> str:
    try:
        return Path(f"/proc/{pid}/cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")
    except OSError:
        return ""


def step(msg: str) -> None:
    print(f"ok   {msg}", flush=True)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--models", required=True)
    ap.add_argument("--trurl", default=str(ROOT / "target/release/trurl"))
    ap.add_argument("--netblock", help="LD_PRELOAD library that blocks the network (scripts/netblock.c)")
    a = ap.parse_args()

    home = Path(tempfile.mkdtemp(prefix="trurl-smoke-"))
    (home / "models").mkdir()
    shutil.copy(Path(a.models).expanduser() / MODEL, home / "models" / MODEL)
    (home / "Documents").mkdir()
    env = {
        "TRURL_HOME": str(home),
        "HOME": str(home),
        "TRURL_LLAMA_DIR": str(ROOT / "vendor/llama/linux"),
        "LD_PRELOAD": a.netblock or "",
        "NETBLOCK_LOG": str(home / "net.log"),
        "TERM": "xterm-256color",
    }
    cmd = " ".join(f"{k}='{v}'" for k, v in env.items()) + f" '{a.trurl}'"
    tmux("kill-session", "-t", SESSION)
    tmux("new-session", "-d", "-s", SESSION, "-x", "120", "-y", "34", cmd)
    try:
        wait_for("Ready", 300)
        step("the model loads")

        tmux("send-keys", "-t", SESSION, "What is the capital of Ghana? Answer in one short sentence.", "Enter")
        s = wait_for("Accra", 300)
        wait_for("Ready", 300)
        step("it answers offline (Accra)")

        tmux("send-keys", "-t", SESSION, "F6")
        notes = home / "Documents" / "Trurl notes.txt"
        for _ in range(20):
            if notes.exists():
                break
            time.sleep(0.2)
        text = notes.read_text() if notes.exists() else ""
        if "Accra" not in text or "Question: What is the capital of Ghana?" not in text:
            raise SystemExit(f"FAIL: F6 did not save the answer to {notes}: {text!r}")
        step("F6 saves the answer with its question in the notes file")

        second = subprocess.run([a.trurl], env={**os.environ, **env}, capture_output=True, text=True, timeout=30)
        if second.returncode == 0 or "already open" not in second.stderr:
            raise SystemExit(f"FAIL: a second trurl was not refused: {second.returncode} {second.stderr!r}")
        step("a second trurl is refused, in plain words")

        mine = ours(home)
        servers = [p for p in mine if "llama-server" in cmdline(p)]
        if not servers:
            raise SystemExit("FAIL: no llama-server found for this Trurl")
        trurl = [p for p in mine if cmdline(p).startswith(a.trurl)]
        for p in trurl:
            os.kill(p, 9)  # a crash: no chance to clean up
        for _ in range(50):
            if not any(Path(f"/proc/{p}").exists() for p in servers):
                break
            time.sleep(0.2)
        alive = [p for p in servers if Path(f"/proc/{p}").exists()]
        if alive:
            raise SystemExit(f"FAIL: llama-server {alive} still runs after Trurl was killed")
        step("killing Trurl (a crash) also stops its llama-server")

        log = home / "net.log"
        if a.netblock and log.exists() and log.read_text().strip():
            raise SystemExit(f"FAIL: something tried to reach the network:\n{log.read_text()}")
        if a.netblock:
            step("nothing tried to reach the network")
        print("tui smoke: all passed")
        return 0
    finally:
        tmux("kill-session", "-t", SESSION)
        for p in ours(home):
            try:
                os.kill(p, 9)
            except OSError:
                pass
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
