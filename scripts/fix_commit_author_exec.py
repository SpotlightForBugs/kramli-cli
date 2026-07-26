#!/usr/bin/env python3
"""Amend the current commit during rebase when Cursor is the author.

Used as: git rebase --root --exec "python3 scripts/fix_commit_author_exec.py"
"""

from __future__ import annotations

import os
import re
import subprocess
import sys

CURSOR_AUTHOR_EMAIL = "cursoragent@cursor.com"
CURSOR_COAUTHOR_LINE_RE = re.compile(
    r"^co-authored-by:.*(<cursoragent@cursor\.com>|cursor\s*<cursoragent@cursor\.com>)",
    re.IGNORECASE,
)
COAUTHOR_RE = re.compile(
    r"^Co-authored-by:\s*(.+?)\s*<([^>]+)>",
    re.IGNORECASE | re.MULTILINE,
)


def scrub_commit_message(message: str) -> str:
    kept = [
        line
        for line in message.splitlines()
        if not CURSOR_COAUTHOR_LINE_RE.match(line.strip())
    ]
    while kept and not kept[-1].strip():
        kept.pop()
    if not kept:
        return ""
    return "\n".join(kept) + "\n"


def pick_human_author(message: str, fallback_name: str, fallback_email: str) -> tuple[str, str]:
    for match in COAUTHOR_RE.finditer(message):
        name = match.group(1).strip()
        email = match.group(2).strip().lower()
        if email == CURSOR_AUTHOR_EMAIL.lower():
            continue
        if "cursoragent" in email:
            continue
        return name, email
    return fallback_name, fallback_email


def main() -> int:
    author_email = subprocess.check_output(
        ["git", "log", "-1", "--format=%ae"], text=True
    ).strip()
    if author_email.lower() != CURSOR_AUTHOR_EMAIL.lower():
        return 0

    fallback_name = os.environ.get("FALLBACK_AUTHOR_NAME", "").strip()
    fallback_email = os.environ.get("FALLBACK_AUTHOR_EMAIL", "").strip()
    if not fallback_name or not fallback_email:
        print("FALLBACK_AUTHOR_NAME and FALLBACK_AUTHOR_EMAIL are required", file=sys.stderr)
        return 1

    message = subprocess.check_output(["git", "log", "-1", "--format=%B"], text=True)
    new_name, new_email = pick_human_author(message, fallback_name, fallback_email)
    new_message = scrub_commit_message(message)

    env = os.environ.copy()
    env["GIT_COMMITTER_NAME"] = new_name
    env["GIT_COMMITTER_EMAIL"] = new_email
    subprocess.run(
        [
            "git",
            "commit",
            "--amend",
            f"--author={new_name} <{new_email}>",
            "-m",
            new_message,
        ],
        check=True,
        env=env,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
