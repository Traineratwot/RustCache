#!/usr/bin/env bash
# Assemble a Debian package for rustcache-client (GUI).
# Usage: packaging/scripts/build-deb-client.sh <VERSION> <ARCH>
#   VERSION  e.g. 0.1.0
#   ARCH     amd64 | arm64
set -euo pipefail

VERSION="${1:?version required (e.g. 0.1.0)}"
ARCH="${2:?arch required (amd64|arm64)}"

case "$ARCH" in
amd64 | arm64) ;;
*)
	echo "unsupported arch: $ARCH (want amd64|arm64)" >&2
	exit 1
	;;
esac

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$ROOT/target/release/rustcache-client"
OUT_DIR="$ROOT/dist"
DEB_NAME="rustcache-client_${VERSION}_${ARCH}.deb"

if [[ ! -x "$BIN" ]]; then
	echo "binary not found: $BIN (build with: cargo build -p rustcache-client --release --features gui)" >&2
	exit 1
fi

mkdir -p "$OUT_DIR"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

PKG="$STAGE/rustcache-client_${VERSION}_${ARCH}"
mkdir -p \
	"$PKG/DEBIAN" \
	"$PKG/usr/bin" \
	"$PKG/usr/share/applications" \
	"$PKG/usr/share/doc/rustcache-client" \
	"$PKG/usr/share/icons/hicolor/128x128/apps" \
	"$PKG/usr/share/icons/hicolor/256x256/apps" \
	"$PKG/usr/share/icons/hicolor/512x512/apps"

install -m0755 "$BIN" "$PKG/usr/bin/rustcache-client"
install -m0644 "$ROOT/packaging/linux/rustcache-client.desktop" \
	"$PKG/usr/share/applications/rustcache-client.desktop"
install -m0644 "$ROOT/docs/media/logo-128.png" \
	"$PKG/usr/share/icons/hicolor/128x128/apps/rustcache-client.png"
install -m0644 "$ROOT/docs/media/logo-256.png" \
	"$PKG/usr/share/icons/hicolor/256x256/apps/rustcache-client.png"
install -m0644 "$ROOT/docs/media/logo-512.png" \
	"$PKG/usr/share/icons/hicolor/512x512/apps/rustcache-client.png"

cat >"$PKG/usr/share/doc/rustcache-client/copyright" <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: rustcache-client
Source: https://github.com/Traineratwot/RustCache

Files: *
Copyright: Traineratwot
License: MIT
EOF

sed -e "s/@VERSION@/${VERSION}/" -e "s/@ARCH@/${ARCH}/" \
	"$ROOT/packaging/debian/control-client.in" >"$PKG/DEBIAN/control"
SIZE="$(du -sk "$PKG" | cut -f1)"
echo "Installed-Size: $SIZE" >>"$PKG/DEBIAN/control"

dpkg-deb --root-owner-group --build "$PKG" "$OUT_DIR/$DEB_NAME"

echo "built $OUT_DIR/$DEB_NAME"
dpkg-deb --info "$OUT_DIR/$DEB_NAME"
