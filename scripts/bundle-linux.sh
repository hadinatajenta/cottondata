#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="CottonData"
WORKSPACE_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -n 1)"
VERSION="${VERSION:-$WORKSPACE_VERSION}"
ARCH_LABEL="${ARCH_LABEL:-x64}"
DEB_ARCH="${DEB_ARCH:-amd64}"

DIST_DIR="$ROOT_DIR/dist"
STAGE_DIR="$DIST_DIR/cottondata-linux-${ARCH_LABEL}"

cd "$ROOT_DIR"
cargo build --release -p cottondata-desktop -p cottondata-cli

rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR/share/applications" "$STAGE_DIR/share/icons/hicolor/256x256/apps" "$STAGE_DIR/share/icons/hicolor/512x512/apps"

cp "$ROOT_DIR/target/release/CottonData" "$STAGE_DIR/CottonData"
cp "$ROOT_DIR/target/release/cottondata-cli" "$STAGE_DIR/cottondata-cli"
chmod +x "$STAGE_DIR/CottonData" "$STAGE_DIR/cottondata-cli"
cp "$ROOT_DIR/assets/cottondata.desktop" "$STAGE_DIR/share/applications/cottondata.desktop"
cp "$ROOT_DIR/assets/AppIcon-256.png" "$STAGE_DIR/share/icons/hicolor/256x256/apps/cottondata.png"
cp "$ROOT_DIR/assets/AppIcon-512.png" "$STAGE_DIR/share/icons/hicolor/512x512/apps/cottondata.png"
cp "$ROOT_DIR/README.md" "$ROOT_DIR/LICENSE" "$STAGE_DIR/"

cat > "$STAGE_DIR/install-user.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" "$HOME/.local/share/icons/hicolor/256x256/apps" "$HOME/.local/share/icons/hicolor/512x512/apps"
install -m 755 "$HERE/CottonData" "$HOME/.local/bin/CottonData"
install -m 755 "$HERE/cottondata-cli" "$HOME/.local/bin/cottondata-cli"
install -m 644 "$HERE/share/applications/cottondata.desktop" "$HOME/.local/share/applications/cottondata.desktop"
install -m 644 "$HERE/share/icons/hicolor/256x256/apps/cottondata.png" "$HOME/.local/share/icons/hicolor/256x256/apps/cottondata.png"
install -m 644 "$HERE/share/icons/hicolor/512x512/apps/cottondata.png" "$HOME/.local/share/icons/hicolor/512x512/apps/cottondata.png"
echo "Installed CottonData to ~/.local/bin"
EOF
chmod +x "$STAGE_DIR/install-user.sh"

TAR_PATH="$DIST_DIR/${APP_NAME}-linux-${ARCH_LABEL}.tar.gz"
tar -czf "$TAR_PATH" -C "$DIST_DIR" "cottondata-linux-${ARCH_LABEL}"

if command -v dpkg-deb >/dev/null 2>&1; then
  DEB_ROOT="$(mktemp -d)"
  trap 'rm -rf "$DEB_ROOT"' EXIT
  mkdir -p "$DEB_ROOT/DEBIAN" "$DEB_ROOT/usr/bin" "$DEB_ROOT/usr/share/applications" "$DEB_ROOT/usr/share/icons/hicolor/256x256/apps" "$DEB_ROOT/usr/share/icons/hicolor/512x512/apps"
  install -m 755 "$STAGE_DIR/CottonData" "$DEB_ROOT/usr/bin/CottonData"
  install -m 755 "$STAGE_DIR/cottondata-cli" "$DEB_ROOT/usr/bin/cottondata-cli"
  install -m 644 "$ROOT_DIR/assets/cottondata.desktop" "$DEB_ROOT/usr/share/applications/cottondata.desktop"
  install -m 644 "$ROOT_DIR/assets/AppIcon-256.png" "$DEB_ROOT/usr/share/icons/hicolor/256x256/apps/cottondata.png"
  install -m 644 "$ROOT_DIR/assets/AppIcon-512.png" "$DEB_ROOT/usr/share/icons/hicolor/512x512/apps/cottondata.png"
  cat > "$DEB_ROOT/DEBIAN/control" <<CTRL
Package: cottondata
Version: ${VERSION}
Section: database
Priority: optional
Architecture: ${DEB_ARCH}
Maintainer: Hadinata <hadinata.jenta@sharingvision.co.id>
Description: Featherweight native Rust MySQL client for up to 10 servers
 CottonData is a low-RAM MySQL-only desktop & CLI client written in pure Rust.
CTRL
  DEB_PATH="$DIST_DIR/cottondata_${VERSION}_${DEB_ARCH}.deb"
  dpkg-deb --build "$DEB_ROOT" "$DEB_PATH" >/dev/null
  ls -lh "$TAR_PATH" "$DEB_PATH"
else
  ls -lh "$TAR_PATH"
fi
