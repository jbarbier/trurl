#!/usr/bin/env bash
# UI end to end on Linux: the desktop UI in Chromium, served by the bridge,
# backed by the real core, the real llama-server and real models.
#   1. full suite, online (Hugging Face search and a real 1 MB download)
#   2. offline suite, with the network blocked for the bridge and llama-server
# Screenshots: target/e2e/{full,offline}/
set -euo pipefail
cd "$(dirname "$0")/../../.."

MODELS="${TRURL_TEST_MODELS:-$HOME/.cache/trurl/models}"
python3 scripts/fetch_models.py --dest "$MODELS" --only gemma-3-1b-it,qwen3-0.6b
cargo build -p trurl-bridge
(cd apps/bridge/e2e && npm ci --no-audit --no-fund >/dev/null && npx playwright install chromium >/dev/null)
mkdir -p target
gcc -shared -fPIC -O2 -o target/netblock.so scripts/netblock.c -ldl

run_suite() { # suite port preload
  local suite=$1 port=$2 preload=$3
  # A fixed, readable folder: screenshots show its path (the Models screen).
  local home="/tmp/trurl-$suite"
  rm -rf "$home" && mkdir -p "$home"
  mkdir -p "$home/models"
  ln "$MODELS/gemma-3-1b-it-Q4_K_M.gguf" "$home/models/" 2>/dev/null || cp "$MODELS/gemma-3-1b-it-Q4_K_M.gguf" "$home/models/"
  ln "$MODELS/Qwen_Qwen3-0.6B-Q4_K_M.gguf" "$home/models/" 2>/dev/null || cp "$MODELS/Qwen_Qwen3-0.6B-Q4_K_M.gguf" "$home/models/"
  mkdir -p "target/e2e/$suite"
  TRURL_HOME="$home" LD_PRELOAD="$preload" NETBLOCK_LOG="$home/net.log" target/debug/trurl-bridge --port "$port" 2> "target/e2e/$suite/bridge.log" &
  local pid=$!
  for _ in $(seq 1 50); do curl -s "http://127.0.0.1:$port/" >/dev/null && break; sleep 0.2; done
  local status=0
  (cd apps/bridge/e2e && node run.mjs --url "http://127.0.0.1:$port/" --suite "$suite" --out "../../../target/e2e/$suite") || status=$?
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  # Keep what the model server printed, next to the screenshots.
  cp -r "$home/logs" "target/e2e/$suite/" 2>/dev/null || true
  rm -rf "$home"
  return $status
}

run_suite full 7861 ""
run_suite offline 7862 "$PWD/target/netblock.so"
