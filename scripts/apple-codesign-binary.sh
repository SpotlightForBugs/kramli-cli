#!/usr/bin/env bash
# Sign a macOS CLI binary with Developer ID (hardened runtime + timestamp).
# Expects APPLE_CERTIFICATE, APPLE_CERTIFICATE_PASSWORD, APPLE_SIGNING_IDENTITY.
set -euo pipefail

bin="${1:?usage: apple-codesign-binary.sh PATH/TO/kramli}"

if [[ -z "${APPLE_CERTIFICATE:-}" || -z "${APPLE_CERTIFICATE_PASSWORD:-}" || -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  echo "Skipping codesign: APPLE_CERTIFICATE secrets not set" >&2
  exit 0
fi

KEYCHAIN_PATH="${RUNNER_TEMP:-/tmp}/kramli-signing.keychain-db"
KEYCHAIN_PASSWORD="${KEYCHAIN_PASSWORD:-$(openssl rand -base64 32)}"
CERTIFICATE_PATH="${RUNNER_TEMP:-/tmp}/kramli-signing.p12"

echo "$APPLE_CERTIFICATE" | base64 --decode > "$CERTIFICATE_PATH"

security delete-keychain "$KEYCHAIN_PATH" >/dev/null 2>&1 || true
security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"
security set-keychain-settings -lut 21600 "$KEYCHAIN_PATH"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"
security import "$CERTIFICATE_PATH" -P "$APPLE_CERTIFICATE_PASSWORD" -A -t cert -f pkcs12 -k "$KEYCHAIN_PATH"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"
security list-keychain -d user -s "$KEYCHAIN_PATH"

codesign --force --options runtime --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$bin"
codesign --verify --verbose=2 "$bin"
spctl -a -t exec -vv "$bin" || true
