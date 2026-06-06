"""Shared helpers for release artifact SHA-256 sidecars."""

from __future__ import annotations

import hashlib
import re
from pathlib import Path

ARCHIVE_SUFFIXES = (".tar.xz", ".zip", ".tar.gz")


def read_sidecar(path: Path) -> str:
    line = path.read_text(encoding="utf-8").strip()
    if not line:
        raise ValueError(f"empty checksum file: {path}")
    checksum = line.split()[0]
    if len(checksum) != 64 or not re.fullmatch(r"[0-9a-f]+", checksum):
        raise ValueError(f"invalid sha256 in {path}: {checksum!r}")
    return checksum


def hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_sidecars(artifacts_dir: Path) -> dict[str, str]:
    sidecars: dict[str, str] = {}
    for sidecar in sorted(artifacts_dir.glob("*.sha256")):
        name = sidecar.name[: -len(".sha256")]
        sidecars[name] = read_sidecar(sidecar)
    return sidecars


def is_release_archive(name: str) -> bool:
    return name.startswith("kramli-") and name.endswith(ARCHIVE_SUFFIXES)
