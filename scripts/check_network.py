#!/usr/bin/env python3
"""Every web address in Trurl's code must be on this list.

Trurl may reach exactly two places on the internet: Hugging Face (only when the
learner opens the browser) and GitHub Releases (the update check). Adding a
host here is a product decision (CLAUDE.md, product rule 1), not a drive-by.
The live evals prove the same thing at runtime: scripts/run_evals.sh blocks the
network and fails if anything even tries.

  python3 scripts/check_network.py
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# host -> why it is allowed
ALLOWED = {
    "huggingface.co": "Hugging Face browser and downloads (opt-in, only when online)",
    "github.com": "update manifest on GitHub Releases, and the project link in About",
    "127.0.0.1": "Trurl's own llama-server",
    "localhost": "Tauri's internal protocol",
    "ipc.localhost": "Tauri's internal protocol",
    "schema.tauri.app": "JSON schema reference in config files, never fetched by the app",
    "ai.google.dev": "the Gemma notice text required by the Gemma Terms of Use (text only)",
    "www.w3.org": "the SVG namespace identifier in the UI code, never fetched",
}
# Hugging Face redirects downloads to its CDNs. Trurl never stores or names these
# URLs; they appear only as redirect targets at runtime.
CDN_NOTE = "cdn-lfs*.hf.co, cas-bridge.xethub.hf.co (redirect targets of huggingface.co downloads)"

SCAN = [
    ("crates", "*.rs"),
    ("apps/tui/src", "*.rs"),
    ("apps/bridge/src", "*.rs"),
    ("apps/desktop/src-tauri/src", "*.rs"),
    ("apps/desktop/src-tauri", "*.json"),
    ("apps/desktop/src-tauri/capabilities", "*.json"),
    ("apps/desktop/ui", "*.js"),
    ("apps/desktop/ui", "*.html"),
    ("apps/desktop/ui", "*.css"),
]
SKIP = {"apps/desktop/ui/vendor", "apps/desktop/src-tauri/gen"}
URL = re.compile(r"\b(?:https?|wss?)://([A-Za-z0-9.\-]+)")


def main() -> int:
    bad = []
    seen: dict[str, int] = {}
    for d, pat in SCAN:
        for p in sorted((ROOT / d).rglob(pat)):
            rel = p.relative_to(ROOT).as_posix()
            if any(rel.startswith(s) for s in SKIP) or "/tests/" in rel or rel.endswith("mock.js"):
                continue
            for i, line in enumerate(p.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
                for m in URL.finditer(line):
                    host = m.group(1).lower()
                    seen[host] = seen.get(host, 0) + 1
                    if host not in ALLOWED:
                        bad.append(f"{rel}:{i}: {host} is not on the allowlist")
    for host, n in sorted(seen.items()):
        print(f"{'ok ' if host in ALLOWED else 'BAD'} {host} ({n}x): {ALLOWED.get(host, 'NOT ALLOWED')}")
    print(f"note: {CDN_NOTE}")
    for b in bad:
        print(b)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
