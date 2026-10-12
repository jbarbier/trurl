#!/usr/bin/env bash
# Build the Linux release tarball: dist/trurl-<version>-linux-x64.tar.gz
#
# Needs: the three llama-server builds in vendor/llama/linux (scripts/build_llama.py)
# and a release build of the terminal app. CI runs this inside Debian 11 so the
# result runs on any Linux with glibc 2.31 or newer (Ubuntu 20.04+, Debian 11+).
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(python3 -c 'import json;print(json.load(open("pins.json"))["app"]["version"])')
NAME="trurl-$VERSION-linux-x64"
OUT="dist/$NAME"

cargo build --release --locked -p trurl-tui
python3 scripts/check_pe_imports.py vendor/llama/linux

rm -rf "$OUT"
mkdir -p "$OUT/llama" "$OUT/licenses"
cp target/release/trurl "$OUT/trurl"
for v in avx2 avx compat; do
  mkdir -p "$OUT/llama/$v"
  cp "vendor/llama/linux/$v/llama-server" "$OUT/llama/$v/"
done
cp LICENSE "$OUT/licenses/Trurl-MIT.txt"
cp licenses/*.txt "$OUT/licenses/"
cp packaging/linux/install.sh "$OUT/install.sh"
cp packaging/linux/README.txt "$OUT/README.txt"
chmod +x "$OUT/trurl" "$OUT/install.sh" "$OUT"/llama/*/llama-server

tar -C dist -czf "dist/$NAME.tar.gz" "$NAME"
sha256sum "dist/$NAME.tar.gz"
echo "glibc needed: $(objdump -T "$OUT/trurl" "$OUT/llama/compat/llama-server" | grep -o 'GLIBC_[0-9.]*' | sort -V -u | tail -1)"
