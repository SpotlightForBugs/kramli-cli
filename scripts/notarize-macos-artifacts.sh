#!/usr/bin/env bash
# Notarize signed macOS release archives produced by cargo-dist.
# Expects APPLE_NOTARY_KEY, APPLE_NOTARY_KEY_ID, APPLE_NOTARY_ISSUER.
set -euo pipefail

if [[ -z "${APPLE_NOTARY_KEY:-}" || -z "${APPLE_NOTARY_KEY_ID:-}" || -z "${APPLE_NOTARY_ISSUER:-}" ]]; then
  echo "Skipping notarization: APPLE_NOTARY_* secrets not set"
  exit 0
fi

KEY_PATH="$RUNNER_TEMP/notary-key.p8"
echo "$APPLE_NOTARY_KEY" | base64 --decode > "$KEY_PATH"
chmod 600 "$KEY_PATH"

WORKDIR="$RUNNER_TEMP/notarize"
mkdir -p "$WORKDIR"

shopt -s nullglob
for archive in target/distrib/kramli-*-apple-darwin.tar.xz; do
  name="$(basename "$archive" .tar.xz)"
  stage="$WORKDIR/$name"
  mkdir -p "$stage"
  tar -xJf "$archive" -C "$stage"

  bin="$stage/kramli"
  if [[ ! -f "$bin" ]]; then
    echo "No kramli binary in $archive" >&2
    exit 1
  fi

  zip_path="$WORKDIR/$name.zip"
  ditto -c -k --keepParent "$bin" "$zip_path"

  echo "Submitting $name for notarization..."
  xcrun notarytool submit "$zip_path" \
    --key "$KEY_PATH" \
    --key-id "$APPLE_NOTARY_KEY_ID" \
    --issuer "$APPLE_NOTARY_ISSUER" \
    --wait

  echo "Stapling $bin"
  xcrun stapler staple "$bin"

  echo "Repacking $archive"
  rm -f "$archive"
  tar -cJf "$archive" -C "$stage" kramli
done
