#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd)"
PACKAGE_NAME="hardware-workbench"
APP_NAME="hardware-workbench-app"
DEB_ARCH="${DEB_ARCH:-amd64}"

if [[ "$DEB_ARCH" != "amd64" ]]; then
    echo "Ubuntu V1 currently supports amd64 only (got: $DEB_ARCH)" >&2
    exit 2
fi

if [[ $# -gt 1 ]]; then
    echo "Usage: $0 [version]" >&2
    exit 2
fi

if [[ $# -eq 1 ]]; then
    VERSION="$1"
else
    PACKAGE_ID="$(cargo pkgid --manifest-path "$REPO_ROOT/Cargo.toml" -p "$APP_NAME")"
    VERSION="${PACKAGE_ID##*@}"
fi

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]; then
    echo "Invalid Debian package version: $VERSION" >&2
    exit 2
fi

command -v cargo >/dev/null
command -v dpkg-deb >/dev/null

echo "Building $APP_NAME v$VERSION..."
cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p "$APP_NAME" --release

RELEASE_BINARY="$REPO_ROOT/target/release/$APP_NAME"
if [[ ! -x "$RELEASE_BINARY" ]]; then
    echo "Release binary not found or not executable: $RELEASE_BINARY" >&2
    exit 1
fi

DIST_DIR="$REPO_ROOT/dist"
STAGE_DIR="$DIST_DIR/${PACKAGE_NAME}-deb-root"
OUTPUT_PATH="$DIST_DIR/${PACKAGE_NAME}_${VERSION}_${DEB_ARCH}.deb"

rm -rf -- "$STAGE_DIR"
rm -f -- "$OUTPUT_PATH"

install -Dm755 "$RELEASE_BINARY" \
    "$STAGE_DIR/usr/lib/hardware-workbench/$APP_NAME"
install -Dm644 "$REPO_ROOT/assets/JetBrainsMonoNerdFontMono-Regular.ttf" \
    "$STAGE_DIR/usr/lib/hardware-workbench/assets/JetBrainsMonoNerdFontMono-Regular.ttf"
install -Dm644 "$REPO_ROOT/assets/NotoSansSC-VF.ttf" \
    "$STAGE_DIR/usr/lib/hardware-workbench/assets/NotoSansSC-VF.ttf"
install -Dm644 "$REPO_ROOT/assets/app-icon-256.png" \
    "$STAGE_DIR/usr/share/icons/hicolor/256x256/apps/hardware-workbench.png"
install -Dm644 "$SCRIPT_DIR/hardware-workbench.desktop" \
    "$STAGE_DIR/usr/share/applications/hardware-workbench.desktop"
install -Dm644 "$REPO_ROOT/LICENSE" \
    "$STAGE_DIR/usr/share/doc/$PACKAGE_NAME/copyright"
install -Dm644 "$REPO_ROOT/README.md" \
    "$STAGE_DIR/usr/share/doc/$PACKAGE_NAME/README.md"

ln -s "/usr/lib/hardware-workbench/$APP_NAME" \
    "$STAGE_DIR/usr/bin/$APP_NAME"

mkdir -p "$STAGE_DIR/DEBIAN"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@ARCH@/$DEB_ARCH/g" \
    "$SCRIPT_DIR/control.in" > "$STAGE_DIR/DEBIAN/control"

dpkg-deb --build --root-owner-group "$STAGE_DIR" "$OUTPUT_PATH" >/dev/null
rm -rf -- "$STAGE_DIR"

echo "Debian package created: $OUTPUT_PATH"
