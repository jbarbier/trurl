#!/bin/sh
# Install Trurl for the current user. No root needed.
#
#   sh install.sh            install or update
#   sh install.sh --remove   uninstall (asks before deleting models)
#
# App:    ~/.local/share/trurl/app/      (replaced on update)
# Models: ~/.local/share/trurl/models/   (never touched by install or update)
# Command: ~/.local/bin/trurl
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
DATA="${XDG_DATA_HOME:-$HOME/.local/share}/trurl"
APP="$DATA/app"
BIN="$HOME/.local/bin"

if [ "${1:-}" = "--remove" ]; then
  rm -rf "$APP" "$BIN/trurl"
  echo "Trurl removed."
  if [ -d "$DATA/models" ]; then
    printf "Also delete your models in %s? [y/N] " "$DATA/models"
    read -r answer || answer=n
    case "$answer" in
      y|Y) rm -rf "$DATA"; echo "Models deleted." ;;
      *) echo "Models kept." ;;
    esac
  fi
  exit 0
fi

mkdir -p "$DATA/models" "$BIN"
rm -rf "$APP.new"
mkdir -p "$APP.new"
cp -R "$HERE/trurl" "$HERE/llama" "$HERE/licenses" "$APP.new/"
cp "$HERE/install.sh" "$APP.new/install.sh"
echo "Trurl app folder. Replaced as a whole on update; keep your files elsewhere." > "$APP.new/.trurl-app"
[ -f "$HERE/README.txt" ] && cp "$HERE/README.txt" "$APP.new/"
chmod +x "$APP.new/trurl" "$APP.new"/llama/*/llama-server
rm -rf "$APP.old"
[ -d "$APP" ] && mv "$APP" "$APP.old"
mv "$APP.new" "$APP"
rm -rf "$APP.old"
ln -sf "$APP/trurl" "$BIN/trurl"

# Models next to this installer (the USB package) are copied in, never moved.
for dir in "$HERE/models" "$HERE/../models" "$HERE/../../models"; do
  [ -d "$dir" ] || continue
  for f in "$dir"/*.gguf; do
    [ -f "$f" ] || continue
    name=$(basename "$f")
    if [ -f "$DATA/models/$name" ] && [ "$(wc -c < "$f")" = "$(wc -c < "$DATA/models/$name")" ]; then
      echo "Already there: $name"
      continue
    fi
    echo "Copying $name"
    cp "$f" "$DATA/models/$name.part" && mv "$DATA/models/$name.part" "$DATA/models/$name"
  done
done

echo
echo "Trurl is installed. Start it with:  trurl"
case ":$PATH:" in
  *":$BIN:"*) ;;
  *) echo "(If 'trurl' is not found, open a new terminal, or run $BIN/trurl)" ;;
esac
