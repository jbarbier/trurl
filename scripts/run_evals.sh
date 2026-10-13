#!/usr/bin/env bash
# Evals: the real core, the real pinned llama-server and real models, with the
# network cut off (Linux). Fails if any test fails OR if anything on the chat
# path even tries to reach the network.
#
#   bash scripts/run_evals.sh               # all live evals
#   bash scripts/run_evals.sh reasoning     # only tests whose name contains "reasoning"
#
# Needs: a C compiler (for the network shim), the llama-server builds
# (python3 scripts/build_llama.py) and ~1.3 GB for two models, fetched once.
set -euo pipefail
cd "$(dirname "$0")/.."

MODELS="${TRURL_TEST_MODELS:-$HOME/.cache/trurl/models}"
FILTER="${1:-}"

python3 scripts/fetch_models.py --dest "$MODELS" --only gemma-3-1b-it,qwen3-0.6b
# A 1 MB base model with no chat template (course session 1.1.2), pinned by commit and hash.
python3 scripts/fetch_models.py --dest "$MODELS" --test-models
if [ ! -x vendor/llama/linux/ssse3/llama-server ]; then
  python3 scripts/build_llama.py
fi

mkdir -p target
gcc -shared -fPIC -O2 -o target/netblock.so scripts/netblock.c -ldl

# Build first (cargo itself may need the network), then run the test binary offline.
BIN=$(cargo test -p trurl-core --test live --no-run --message-format=json 2>/dev/null \
  | python3 -c 'import json,sys
for l in sys.stdin:
    try: m=json.loads(l)
    except ValueError: continue
    if m.get("reason")=="compiler-artifact" and m.get("target",{}).get("name")=="live" and m.get("executable"): print(m["executable"])' | tail -1)
[ -n "$BIN" ] || { echo "could not build the live tests"; exit 1; }

NETLOG=$(mktemp)
echo "Running evals with the network blocked (attempts are logged to $NETLOG)"
set +e
NETBLOCK_LOG="$NETLOG" LD_PRELOAD="$PWD/target/netblock.so" TRURL_TEST_MODELS="$MODELS" \
  "$BIN" --ignored --test-threads 1 $FILTER
STATUS=$?
set -e

if [ -s "$NETLOG" ]; then
  echo "FAIL: something tried to reach the network during the evals:"
  cat "$NETLOG"
  exit 1
fi
echo "Network: no attempt to reach anything outside this computer."
exit $STATUS
