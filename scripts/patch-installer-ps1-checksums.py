#!/usr/bin/env python3
"""Inject SHA-256 verification into cargo-dist's PowerShell installer (not built-in yet)."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

from release_checksums import load_sidecars


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--installer", type=Path, default=Path("target/distrib/kramli-installer.ps1"))
    parser.add_argument("--artifacts-dir", type=Path, default=Path("target/distrib"))
    return parser.parse_args()


VERIFY_BLOCK = """
  if ($info.ContainsKey("checksum_sha256") -and $info["checksum_sha256"]) {
    $expected = $info["checksum_sha256"].ToLower()
    $actual = (Get-FileHash -Path $dir_path -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $expected) {
      throw "checksum mismatch for ${artifact_name}: expected ${expected}, got ${actual}"
    }
  }
"""


def patch_platform_blocks(text: str, sidecars: dict[str, str]) -> tuple[str, int]:
    patched = 0

    def repl(match: re.Match[str]) -> str:
        nonlocal patched
        block = match.group(0)
        artifact_match = re.search(r'"artifact_name"\s*=\s*"([^"]+)"', block)
        if artifact_match is None:
            return block
        artifact_name = artifact_match.group(1)
        checksum = sidecars.get(artifact_name)
        if checksum is None:
            return block
        if '"checksum_sha256"' in block:
            block = re.sub(
                r'"checksum_sha256"\s*=\s*"[^"]*"',
                f'"checksum_sha256" = "{checksum}"',
                block,
            )
        elif '"zip_ext"' in block:
            block = block.replace(
                '"zip_ext" = ".zip"',
                f'"zip_ext" = ".zip"\n      "checksum_sha256" = "{checksum}"',
            )
            block = block.replace(
                '"zip_ext" = ".tar.xz"',
                f'"zip_ext" = ".tar.xz"\n      "checksum_sha256" = "{checksum}"',
            )
        else:
            return block
        patched += 1
        return block

    platform_pattern = re.compile(
        r'"\S+"\s*=\s*@\{\s*\n(?:.*\n)*?\s*\}',
        re.MULTILINE,
    )
    return platform_pattern.sub(repl, text), patched


def patch_download_verification(text: str) -> str:
    marker = "Invoke-DownloadFile -client $wc -url $url -path $dir_path"
    if marker not in text:
        raise ValueError("could not find Download() download call in installer.ps1")
    if "Get-FileHash -Path $dir_path -Algorithm SHA256" in text:
        return text
    return text.replace(marker, marker + VERIFY_BLOCK, 1)


def main() -> int:
    args = parse_args()
    sidecars = load_sidecars(args.artifacts_dir)
    if not sidecars:
        print(f"error: no sidecars in {args.artifacts_dir}", file=sys.stderr)
        return 1
    if not args.installer.is_file():
        print(f"error: missing {args.installer}", file=sys.stderr)
        return 1

    text = args.installer.read_text(encoding="utf-8")
    text, platform_count = patch_platform_blocks(text, sidecars)
    text = patch_download_verification(text)
    args.installer.write_text(text, encoding="utf-8")

    print(
        f"patched {args.installer} ({platform_count} platform block(s), download verify)",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
