#!/usr/bin/env bash
# Assemble a Debian package for rustcache.
# Usage: packaging/scripts/build-deb.sh <VERSION> <ARCH>
#   VERSION  e.g. 0.1.0
#   ARCH     amd64 | arm64
set -euo pipefail

VERSION="${1:?version required (e.g. 0.1.0)}"
ARCH="${2:?arch required (amd64|arm64)}"

case "$ARCH" in
amd64 | arm64) ;;
*)
	echo "unsupported arch: $ARCH (want amd64 or arm64)" >&2
	exit 1
	;;
esac

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$ROOT/target/release/rustcache"
OUT_DIR="$ROOT/dist"
DEB_NAME="rustcache_${VERSION}_${ARCH}.deb"

if [[ ! -x "$BIN" ]]; then
	echo "binary not found: $BIN (build with: cargo build --release --features embed-ui)" >&2
	exit 1
fi

mkdir -p "$OUT_DIR"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

PKG="$STAGE/rustcache_${VERSION}_${ARCH}"
mkdir -p \
	"$PKG/DEBIAN" \
	"$PKG/usr/bin" \
	"$PKG/etc/rustcache" \
	"$PKG/usr/lib/systemd/system" \
	"$PKG/var/lib/rustcache" \
	"$PKG/usr/share/doc/rustcache"

install -m0755 "$BIN" "$PKG/usr/bin/rustcache"
install -m0644 "$ROOT/packaging/etc/config.system.toml" "$PKG/etc/rustcache/config.toml"
install -m0644 "$ROOT/packaging/systemd/rustcache.service" "$PKG/usr/lib/systemd/system/rustcache.service"
cp "$ROOT/packaging/debian/conffiles" "$PKG/DEBIAN/conffiles"
install -m0755 "$ROOT/packaging/debian/postinst" "$PKG/DEBIAN/postinst"
install -m0755 "$ROOT/packaging/debian/prerm" "$PKG/DEBIAN/prerm"
install -m0755 "$ROOT/packaging/debian/postrm" "$PKG/DEBIAN/postrm"

# Minimal copyright file (Cargo.toml declares MIT)
cat >"$PKG/usr/share/doc/rustcache/copyright" <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: rustcache
Source: https://github.com/Traineratwot/RustCache

Files: *
Copyright: Traineratwot
License: MIT
EOF

sed -e "s/@VERSION@/${VERSION}/" -e "s/@ARCH@/${ARCH}/" \
	"$ROOT/packaging/debian/control.in" >"$PKG/DEBIAN/control"
SIZE="$(du -sk "$PKG" | cut -f1)"
echo "Installed-Size: $SIZE" >>"$PKG/DEBIAN/control"

dpkg-deb --root-owner-group --build "$PKG" "$OUT_DIR/$DEB_NAME"

echo "built $OUT_DIR/$DEB_NAME"
dpkg-deb --info "$OUT_DIR/$DEB_NAME"
dpkg-deb --contents "$OUT_DIR/$DEB_NAME"
