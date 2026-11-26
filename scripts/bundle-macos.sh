#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="CottonData"
WORKSPACE_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -n 1)"
VERSION="${VERSION:-$WORKSPACE_VERSION}"
TARGET="${TARGET:-$(rustc -vV | awk '/host:/ {print $2}')}"
ARCH_LABEL="arm64"
if [[ "$TARGET" == x86_64-* ]]; then
  ARCH_LABEL="x64"
fi

DIST_DIR="$ROOT_DIR/dist"
BUNDLE_DIR="$DIST_DIR/${APP_NAME}.app"
CONTENTS="$BUNDLE_DIR/Contents"
MACOS_DIR="$CONTENTS/MacOS"
RES_DIR="$CONTENTS/Resources"

cd "$ROOT_DIR"
cargo build --release --target "$TARGET" -p cottondata-desktop -p cottondata-cli

rm -rf "$BUNDLE_DIR"
mkdir -p "$MACOS_DIR" "$RES_DIR"

cp "$ROOT_DIR/target/$TARGET/release/CottonData" "$MACOS_DIR/CottonData"
cp "$ROOT_DIR/target/$TARGET/release/cottondata-cli" "$MACOS_DIR/cottondata-cli"
chmod +x "$MACOS_DIR/CottonData" "$MACOS_DIR/cottondata-cli"
cp "$ROOT_DIR/assets/AppIcon.icns" "$RES_DIR/AppIcon.icns"
printf 'APPL????' > "$CONTENTS/PkgInfo"

cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>CottonData</string>
    <key>CFBundleExecutable</key>
    <string>CottonData</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleIdentifier</key>
    <string>dev.cottondata.app</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>CottonData</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>
            <string>SQL Script</string>
            <key>CFBundleTypeExtensions</key>
            <array>
                <string>sql</string>
            </array>
            <key>CFBundleTypeRole</key>
            <string>Editor</string>
        </dict>
    </array>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$BUNDLE_DIR"

ZIP_PATH="$DIST_DIR/${APP_NAME}-macos-${ARCH_LABEL}.zip"
rm -f "$ZIP_PATH"
ditto -c -k --sequesterRsrc --keepParent "$BUNDLE_DIR" "$ZIP_PATH"

DMG_STAGE="$(mktemp -d)"
trap 'rm -rf "$DMG_STAGE"' EXIT
cp -R "$BUNDLE_DIR" "$DMG_STAGE/"
ln -s /Applications "$DMG_STAGE/Applications"

DMG_PATH="$DIST_DIR/${APP_NAME}-macos-${ARCH_LABEL}.dmg"
rm -f "$DMG_PATH"
hdiutil create -volname "$APP_NAME" -srcfolder "$DMG_STAGE" -ov -format UDZO "$DMG_PATH" >/dev/null

shasum -a 256 "$ZIP_PATH" "$DMG_PATH"
du -sh "$BUNDLE_DIR" "$ZIP_PATH" "$DMG_PATH"
