#!/usr/bin/env python3
"""Fail CI if release artifacts, sidecars, and embedded installer checksums disagree."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

from release_checksums import hash_file, is_release_archive, load_sidecars, read_sidecar


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--artifacts-dir",
        type=Path,
        default=Path("target/distrib"),
        help="Directory with archives, sidecars, and global installers",
    )
    return parser.parse_args()


def verify_files_match_sidecars(artifacts_dir: Path, sidecars: dict[str, str]) -> list[str]:
    errors: list[str] = []
    for name, expected in sidecars.items():
        if not is_release_archive(name):
            continue
        artifact = artifacts_dir / name
        if not artifact.is_file():
            errors.append(f"missing artifact file: {name}")
            continue
        actual = hash_file(artifact)
        if actual != expected:
            errors.append(
                f"{name}: sidecar/file mismatch (sidecar {expected}, file {actual})"
            )
    return errors


def verify_sha256_sum(artifacts_dir: Path, sidecars: dict[str, str]) -> list[str]:
    errors: list[str] = []
    sum_path = artifacts_dir / "sha256.sum"
    if not sum_path.is_file():
        return ["missing sha256.sum"]

    sum_entries: dict[str, str] = {}
    for line_no, line in enumerate(sum_path.read_text(encoding="utf-8").splitlines(), 1):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(maxsplit=1)
        if len(parts) != 2:
            errors.append(f"sha256.sum:{line_no}: malformed line")
            continue
        expected, name = parts[0], parts[1].lstrip("*")
        sum_entries[name] = expected

    for name, sidecar in sidecars.items():
        if name not in sum_entries:
            errors.append(f"sha256.sum: missing entry for {name}")
        elif sum_entries[name] != sidecar:
            errors.append(
                f"sha256.sum: {name} has {sum_entries[name]}, sidecar has {sidecar}"
            )
    return errors


def verify_shell_installer(installer: Path, sidecars: dict[str, str]) -> list[str]:
    errors: list[str] = []
    if not installer.is_file():
        return ["missing kramli-installer.sh"]

    text = installer.read_text(encoding="utf-8")
    embedded: dict[str, str] = {}
    for match in re.finditer(
        r'"((?:kramli-[^"]+\.(?:tar\.xz|zip)))"\)\s*\n(?:.*\n)*?\s*_checksum_value="([0-9a-f]{64})"',
        text,
    ):
        embedded[match.group(1)] = match.group(2)

    for name, expected in sidecars.items():
        if not is_release_archive(name):
            continue
        actual = embedded.get(name)
        if actual is None:
            errors.append(f"install.sh: missing checksum for {name}")
        elif actual != expected:
            errors.append(f"install.sh: {name} embeds {actual}, sidecar has {expected}")
    return errors


def verify_homebrew_formula(formula: Path, sidecars: dict[str, str]) -> list[str]:
    errors: list[str] = []
    if not formula.is_file():
        return ["missing kramli.rb"]

    text = formula.read_text(encoding="utf-8")
    for match in re.finditer(
        r'url "https://[^"]+/([^"/]+)"\s*\n\s*sha256 "([0-9a-f]{64})"',
        text,
    ):
        name, embedded = match.group(1), match.group(2)
        if not is_release_archive(name):
            continue
        expected = sidecars.get(name)
        if expected is None:
            errors.append(f"kramli.rb: checksum for unknown artifact {name}")
        elif embedded != expected:
            errors.append(
                f"kramli.rb: {name} embeds {embedded}, sidecar has {expected}"
            )
    return errors


def verify_ps1_installer(installer: Path, sidecars: dict[str, str]) -> list[str]:
    errors: list[str] = []
    if not installer.is_file():
        return ["missing kramli-installer.ps1"]

    text = installer.read_text(encoding="utf-8")
    if "checksum_sha256" not in text:
        errors.append(
            "kramli-installer.ps1: no checksum_sha256 entries (run patch-installer-ps1-checksums.py)"
        )
        return errors

    for match in re.finditer(
        r'@\{\s*\n\s*"artifact_name"\s*=\s*"((?:kramli-[^"]+\.(?:tar\.xz|zip)))"[\s\S]*?\}',
        text,
    ):
        block = match.group(0)
        name = match.group(1)
        checksum_match = re.search(r'"checksum_sha256"\s*=\s*"([0-9a-f]{64})"', block)
        if checksum_match is None:
            errors.append(f"install.ps1: platform block for {name} missing checksum_sha256")
            continue
        embedded = checksum_match.group(1)
        expected = sidecars.get(name)
        if expected is None:
            errors.append(f"install.ps1: checksum for unknown artifact {name}")
        elif embedded != expected:
            errors.append(
                f"install.ps1: {name} embeds {embedded}, sidecar has {expected}"
            )

    if "Get-FileHash -Path $dir_path -Algorithm SHA256" not in text:
        errors.append("kramli-installer.ps1: missing Get-FileHash verification block")
    return errors


def main() -> int:
    args = parse_args()
    artifacts_dir = args.artifacts_dir
    sidecars = load_sidecars(artifacts_dir)
    if not sidecars:
        print(f"error: no *.sha256 sidecars in {artifacts_dir}", file=sys.stderr)
        return 1

    errors: list[str] = []
    errors.extend(verify_files_match_sidecars(artifacts_dir, sidecars))
    errors.extend(verify_sha256_sum(artifacts_dir, sidecars))
    errors.extend(verify_shell_installer(artifacts_dir / "kramli-installer.sh", sidecars))
    errors.extend(verify_homebrew_formula(artifacts_dir / "kramli.rb", sidecars))
    errors.extend(verify_ps1_installer(artifacts_dir / "kramli-installer.ps1", sidecars))

    if errors:
        print("release artifact verification failed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    print(
        f"verified {len(sidecars)} sidecar(s), install.sh, install.ps1, kramli.rb, sha256.sum",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
