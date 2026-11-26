#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
TAG="v${VERSION}"
REPO="hadinatajenta/cottondata"

echo "==> Building macOS release bundle for ${TAG}..."
./scripts/bundle-macos.sh

ZIP_PATH="dist/CottonData-macos-arm64.zip"
DMG_PATH="dist/CottonData-macos-arm64.dmg"
SUMS_PATH="dist/SHA256SUMS.txt"

(cd dist && shasum -a 256 "$(basename "$ZIP_PATH")" "$(basename "$DMG_PATH")" > "$(basename "$SUMS_PATH")")
cat "$SUMS_PATH"

if ! command -v gh >/dev/null 2>&1; then
  echo ""
  echo "Artifacts ready in dist/:"
  echo "  - $ZIP_PATH"
  echo "  - $DMG_PATH"
  echo "  - $SUMS_PATH"
  echo ""
  echo "Install GitHub CLI (brew install gh && gh auth login) to upload automatically,"
  echo "or upload those files at: https://github.com/${REPO}/releases/new?tag=${TAG}"
  exit 0
fi

if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  echo "==> Updating existing GitHub release ${TAG}..."
  gh release upload "$TAG" "$ZIP_PATH" "$DMG_PATH" "$SUMS_PATH" --repo "$REPO" --clobber
else
  echo "==> Creating GitHub release ${TAG}..."
  gh release create "$TAG" "$ZIP_PATH" "$DMG_PATH" "$SUMS_PATH" \
    --repo "$REPO" \
    --title "CottonData ${TAG}" \
    --generate-notes
fi
