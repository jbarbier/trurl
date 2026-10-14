#!/usr/bin/env bash
# The fast gates, as CI's "Gates" job runs them: deterministic, offline, a few
# minutes. Run before every commit; stops at the first failure.
#   bash scripts/gates.sh
set -euo pipefail
cd "$(dirname "$0")/.."
command -v cargo >/dev/null || export PATH="$HOME/.cargo/bin:$PATH"   # rustup installs it there
step() { printf '\n=== %s\n' "$*"; }

step "format";       cargo fmt --all --check
step "clippy";       cargo clippy --locked --all-targets -- -D warnings
step "tests";        cargo test --locked --quiet
step "pins";         python3 scripts/check_pins.py
step "copy";         python3 scripts/check_copy.py
step "network";      python3 scripts/check_network.py
step "ui math";      node apps/desktop/ui/plainmath.test.cjs
step "isa patterns"; python3 scripts/check_cpu_isa.py --self-test
step "bundle marker"; python3 scripts/patch_bundle_type.py --self-test
echo
echo "gates: all passed"
