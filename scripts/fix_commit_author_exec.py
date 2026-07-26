#!/usr/bin/env python3
"""Amend the current commit during rebase when Cursor is the author.

Only reassigns authorship when the commit message already names a human
Co-authored-by trailer. Sole cursoragent commits are left unchanged.
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


def find_human_coauthor(message: str) -> tuple[str, str] | None:
    for match in COAUTHOR_RE.finditer(message):
        name = match.group(1).strip()
        email = match.group(2).strip().lower()
        if email == CURSOR_AUTHOR_EMAIL.lower():
            continue
        if "cursoragent" in email:
            continue
        return name, email
    return None


def run_self_test() -> None:
    message = (
        "feat: example\n\n"
        "Co-authored-by: Johannes Häusler <mail@spotlightforbugs.eu>\n"
    )
    assert find_human_coauthor(message) == (
        "Johannes Häusler",
        "mail@spotlightforbugs.eu",
    )
    assert find_human_coauthor("feat: cursor only\n") is None


def main() -> int:
    author_email = subprocess.check_output(
        ["git", "log", "-1", "--format=%ae"], text=True
    ).strip()
    if author_email.lower() != CURSOR_AUTHOR_EMAIL.lower():
        return 0

    message = subprocess.check_output(["git", "log", "-1", "--format=%B"], text=True)
    human = find_human_coauthor(message)
    if human is None:
        return 0

    new_name, new_email = human
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
    if len(sys.argv) > 1 and sys.argv[1] == "--self-test":
        run_self_test()
        print("self-test: ok")
        raise SystemExit(0)
    raise SystemExit(main())
