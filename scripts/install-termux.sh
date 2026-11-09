#!/data/data/com.termux/files/usr/bin/bash
# Installs the latest OxideCode arm64 build into Termux and exposes it both as
# `opencode` (the name the Android integration calls) and `oxidecode`.
set -euo pipefail

REPO="${OXIDECODE_REPO:-its-zain-dev/oxidecode}"
TAG="${1:-latest}"
ASSET="opencode-termux-aarch64"
BIN_DIR="${PREFIX:-/data/data/com.termux/files/usr}/bin"

if [ "$(uname -m)" != "aarch64" ]; then
  echo "OxideCode currently ships an arm64 binary; this machine is $(uname -m)." >&2
  exit 1
fi

if [ "$TAG" = "latest" ]; then
  URL="https://github.com/$REPO/releases/latest/download/$ASSET"
else
  URL="https://github.com/$REPO/releases/download/$TAG/$ASSET"
fi

echo "Downloading $URL"
tmp="$(mktemp "${TMPDIR:-$PREFIX/tmp}/oxidecode.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
curl -fL --retry 3 --progress-bar "$URL" -o "$tmp"
chmod 755 "$tmp"

# Install the requested name first so an existing official opencode is replaced
# atomically, then link the long name to the same file.
install -m 755 "$tmp" "$BIN_DIR/opencode"
ln -sf opencode "$BIN_DIR/oxidecode"

echo
"$BIN_DIR/opencode" --version
echo "Installed: $BIN_DIR/opencode (and $BIN_DIR/oxidecode)"
echo "Start it with: opencode"
