#!/usr/bin/env python3

"""Sync the project version in Cargo.toml and Cargo.lock.

Usage:
  python3 scripts/set_release_version.py --tag v0.1.8
  python3 scripts/set_release_version.py --version 0.1.8
  python3 scripts/set_release_version.py --tag v0.1.8 --check
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")


def _normalize_version(raw: str) -> str:
    version = raw.strip()
    if version.startswith("v"):
        version = version[1:]
    if not SEMVER_RE.match(version):
        raise ValueError(
            "Invalid version format. Use x.y.z (optionally with -pre and/or +build)."
        )
    return version


def _update_cargo_toml(path: Path, version: str) -> tuple[str, str, str]:
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    in_package = False
    old_version: str | None = None

    for idx, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            in_package = stripped == "[package]"
            continue
        if in_package and stripped.startswith("version"):
            match = re.match(r'(\s*version\s*=\s*")([^"]+)("\s*)$', line.rstrip("\n"))
            if match is None:
                raise ValueError("Could not parse [package].version in Cargo.toml")
            old_version = match.group(2)
            lines[idx] = f"{match.group(1)}{version}{match.group(3)}\n"
            break

    if old_version is None:
        raise ValueError("Could not find [package].version in Cargo.toml")

    return old_version, version, "".join(lines)


def _update_cargo_lock(path: Path, version: str) -> tuple[str, str, str]:
    text = path.read_text(encoding="utf-8")
    pattern = re.compile(
        r'(\[\[package\]\]\nname = "kramli"\nversion = ")([^"]+)(")',
        flags=re.MULTILINE,
    )
    match = pattern.search(text)
    if match is None:
        raise ValueError('Could not find [[package]] name = "kramli" in Cargo.lock')

    old_version = match.group(2)
    updated = pattern.sub(rf"\g<1>{version}\g<3>", text, count=1)
    return old_version, version, updated


def main() -> int:
    parser = argparse.ArgumentParser(description="Set release version in Cargo.toml and Cargo.lock")
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--version", help="Version to set (for example: 0.1.8)")
    source.add_argument("--tag", help="Tag to parse (for example: v0.1.8)")
    parser.add_argument(
        "--check",
        action="store_true",
        help="Check only. Exit non-zero when files do not match the target version.",
    )
    args = parser.parse_args()

    try:
        target_version = _normalize_version(args.version or args.tag or "")
    except ValueError as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return 2

    root = Path(__file__).resolve().parents[1]
    cargo_toml = root / "Cargo.toml"
    cargo_lock = root / "Cargo.lock"

    try:
        toml_old, _, toml_updated = _update_cargo_toml(cargo_toml, target_version)
        lock_old, _, lock_updated = _update_cargo_lock(cargo_lock, target_version)
    except ValueError as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return 2

    in_sync = toml_old == target_version and lock_old == target_version
    if args.check:
        if in_sync:
            print(f"OK: version is already {target_version} in Cargo.toml and Cargo.lock")
            return 0
        print(
            "Version mismatch detected: "
            f"Cargo.toml={toml_old}, Cargo.lock={lock_old}, expected={target_version}",
            file=sys.stderr,
        )
        return 1

    cargo_toml.write_text(toml_updated, encoding="utf-8")
    cargo_lock.write_text(lock_updated, encoding="utf-8")
    print(f"Updated Cargo.toml: {toml_old} -> {target_version}")
    print(f"Updated Cargo.lock: {lock_old} -> {target_version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
