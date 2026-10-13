#!/usr/bin/env python3
"""Print the CHANGELOG.md section for one version (the release notes).

  python3 scripts/release_notes.py 0.2.0 > notes.md
"""

import re
import sys
from pathlib import Path

version = sys.argv[1].lstrip("v")
text = (Path(__file__).resolve().parent.parent / "CHANGELOG.md").read_text()
m = re.search(rf"^## \[?{re.escape(version)}\]?[^\n]*\n(.*?)(?=^## |\Z)", text, re.S | re.M)
if not m:
    sys.exit(f"CHANGELOG.md has no section for {version}")
print(m.group(1).strip())
