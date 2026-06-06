#!/usr/bin/env bash
# Patch every local cargo-dist manifest under target/distrib/ from *.sha256 sidecars.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
artifacts_dir="${1:-$root/target/distrib}"

shopt -s nullglob
for manifest in "$artifacts_dir"/*-dist-manifest.json; do
  case "$(basename "$manifest")" in
    global-dist-manifest.json | plan-dist-manifest.json) continue ;;
  esac
  python3 "$root/scripts/patch-dist-manifest-checksums.py" \
    --manifest "$manifest" \
    --artifacts-dir "$artifacts_dir"
done

if [[ -f dist-manifest.json ]]; then
  python3 "$root/scripts/patch-dist-manifest-checksums.py" \
    --manifest dist-manifest.json \
    --artifacts-dir "$artifacts_dir"
fi
