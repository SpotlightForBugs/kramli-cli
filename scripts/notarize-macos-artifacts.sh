#!/usr/bin/env bash
# Sign, notarize, and staple macOS release archives from cargo-dist.
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

  bin="$stage/$name/kramli"
  if [[ ! -f "$bin" ]]; then
    bin="$(find "$stage" -maxdepth 2 -type f -name kramli | head -1)"
  fi
  if [[ ! -f "$bin" ]]; then
    echo "No kramli binary in $archive" >&2
    exit 1
  fi

  bash scripts/apple-codesign-binary.sh "$bin"

  zip_path="$WORKDIR/$name.zip"
  rm -f "$zip_path"
  ditto -c -k --keepParent "$bin" "$zip_path"

  echo "Submitting $name for notarization..."
  submit_log="$WORKDIR/$name-notary.log"
  set +e
  xcrun notarytool submit "$zip_path" \
    --key "$KEY_PATH" \
    --key-id "$APPLE_NOTARY_KEY_ID" \
    --issuer "$APPLE_NOTARY_ISSUER" \
    --wait 2>&1 | tee "$submit_log"
  submit_status="${PIPESTATUS[0]}"
  set -e

  submission_id="$(sed -n 's/^  id: //p' "$submit_log" | tail -1)"
  if [[ "$submit_status" -ne 0 ]] || grep -q "status: Invalid" "$submit_log"; then
    echo "Notarization rejected for $name" >&2
    if [[ -n "$submission_id" ]]; then
      xcrun notarytool log "$submission_id" \
        --key "$KEY_PATH" \
        --key-id "$APPLE_NOTARY_KEY_ID" \
        --issuer "$APPLE_NOTARY_ISSUER" >&2 || true
    fi
    exit 1
  fi

  echo "Notarization accepted for $name (skipping staple; bare CLI binaries use online validation)"

  echo "Repacking $archive"
  rm -f "$archive"
  tar -cJf "$archive" -C "$stage" "$name"

  archive_name="$(basename "$archive")"
  shasum -a 256 "$archive" | awk -v f="$archive_name" '{print $1 " *" f}' > "${archive}.sha256"
done
