#!/bin/sh
# Assemble Jenova-<version>-x86_64.spk from a built jenova-nas binary.
# Usage: ./build-spk.sh path/to/jenova-nas [outdir]
set -eu
BIN="$1"
OUT="${2:-dist}"
HERE="$(cd "$(dirname "$0")" && pwd)"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$HERE/Cargo.toml" | head -1)"
BUILD="${BUILD_NUMBER:-0001}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# package.tgz: the program and the DSM web UI
mkdir -p "$WORK/pkg/bin" "$WORK/pkg/ui"
install -m 755 "$BIN" "$WORK/pkg/bin/jenova-nas"
cp -R "$HERE/ui/." "$WORK/pkg/ui/"
chmod 755 "$WORK/pkg/ui/api.cgi"
(cd "$WORK/pkg" && tar --owner=0 --group=0 -czf "$WORK/package.tgz" bin ui)

# spk layout
mkdir -p "$WORK/spk"
sed "s/@VERSION@/$VERSION-$BUILD/" "$HERE/package/INFO.in" > "$WORK/spk/INFO"
echo "extractsize=\"$(du -sk "$WORK/pkg" | cut -f1)\"" >> "$WORK/spk/INFO"
cp "$WORK/package.tgz" "$WORK/spk/"
cp -R "$HERE/package/scripts" "$HERE/package/conf" "$WORK/spk/"
cp "$HERE/package/PACKAGE_ICON.PNG" "$HERE/package/PACKAGE_ICON_256.PNG" "$WORK/spk/"
chmod 755 "$WORK/spk/scripts/"*

mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
NAME="Jenova-$VERSION-x86_64.spk"
(cd "$WORK/spk" && tar --owner=0 --group=0 -cf "$OUT/$NAME" INFO package.tgz scripts conf PACKAGE_ICON.PNG PACKAGE_ICON_256.PNG)
echo "$OUT/$NAME"
