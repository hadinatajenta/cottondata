#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
TAG="v${VERSION}"
REPO="hadinatajenta/cottondata"

echo "==> Building macOS release bundle for ${TAG}..."
./scripts/bundle-macos.sh

SUMS_PATH="dist/SHA256SUMS.txt"
shopt -s nullglob
ASSETS=(
  dist/CottonData-macos-*.zip
  dist/CottonData-macos-*.dmg
  dist/CottonData-windows-*.zip
  dist/CottonData-setup-windows-*.exe
  dist/CottonData-linux-*.tar.gz
  dist/cottondata_*.deb
)
shopt -u nullglob

if [[ "${#ASSETS[@]}" -eq 0 ]]; then
  echo "No release assets found in dist/" >&2
  exit 1
fi

(
  cd dist
  rm -f SHA256SUMS.txt
  for asset in "${ASSETS[@]}"; do
    shasum -a 256 "$(basename "$asset")" >> SHA256SUMS.txt
  done
)
cat "$SUMS_PATH"

if ! command -v gh >/dev/null 2>&1; then
  echo ""
  echo "Artifacts ready in dist/:"
  for asset in "${ASSETS[@]}"; do
    echo "  - $asset"
  done
  echo "  - $SUMS_PATH"
  echo ""
  echo "Install GitHub CLI (brew install gh && gh auth login) to upload automatically,"
  echo "or upload those files at: https://github.com/${REPO}/releases/new?tag=${TAG}"
  exit 0
fi

NOTES_FILE="$(mktemp)"
trap 'rm -f "$NOTES_FILE"' EXIT
if [[ -f CHANGELOG.md ]]; then
  awk -v ver="$VERSION" '
    index($0, "## [" ver "]") == 1 { found=1; next }
    found && /^## \[/ { exit }
    found { print }
  ' CHANGELOG.md > "$NOTES_FILE"
fi

if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  echo "==> Updating existing GitHub release ${TAG}..."
  gh release upload "$TAG" "${ASSETS[@]}" "$SUMS_PATH" --repo "$REPO" --clobber
  if [[ -s "$NOTES_FILE" ]] && grep -q '[^[:space:]]' "$NOTES_FILE"; then
    gh release edit "$TAG" --repo "$REPO" --title "CottonData ${TAG}" --notes-file "$NOTES_FILE"
  fi
else
  echo "==> Creating GitHub release ${TAG}..."
  if [[ -s "$NOTES_FILE" ]] && grep -q '[^[:space:]]' "$NOTES_FILE"; then
    gh release create "$TAG" "${ASSETS[@]}" "$SUMS_PATH" \
      --repo "$REPO" \
      --title "CottonData ${TAG}" \
      --notes-file "$NOTES_FILE"
  else
    gh release create "$TAG" "${ASSETS[@]}" "$SUMS_PATH" \
      --repo "$REPO" \
      --title "CottonData ${TAG}" \
      --generate-notes
  fi
fi
