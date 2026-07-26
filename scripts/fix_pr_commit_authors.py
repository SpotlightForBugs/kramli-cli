#!/usr/bin/env python3
"""Rewrite cursoragent-authored commits on a checked-out PR branch."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

CURSOR_AUTHOR_EMAIL = "cursoragent@cursor.com"
REPO_ROOT = Path(__file__).resolve().parents[1]
EXEC_HELPER = REPO_ROOT / "scripts" / "fix_commit_author_exec.py"


def github_request(token: str, path: str) -> dict | list:
    request = urllib.request.Request(
        f"https://api.github.com{path}",
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "kramli-cli-fix-pr-authors",
        },
    )
    try:
        with urllib.request.urlopen(request) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"GitHub API GET {path} failed: {error.code} {body}") from error


def resolve_fallback_author(token: str, repo: str, pr_number: int) -> tuple[str, str]:
    commits = github_request(token, f"/repos/{repo}/pulls/{pr_number}/commits")
    if isinstance(commits, list):
        for commit in commits:
            for author in commit.get("authors") or []:
                email = (author.get("email") or "").strip().lower()
                login = (author.get("login") or "").strip()
                name = (author.get("name") or login or "").strip()
                if not email or email == CURSOR_AUTHOR_EMAIL.lower():
                    continue
                if "cursoragent" in email or login.lower() in {"cursor", "cursoragent"}:
                    continue
                return name, email

    pr = github_request(token, f"/repos/{repo}/pulls/{pr_number}")
    assert isinstance(pr, dict)
    user = pr.get("user") or {}
    login = user.get("login", "github-actions[bot]")
    user_id = user.get("id")
    if user_id:
        return login, f"{user_id}+{login}@users.noreply.github.com"
    return login, f"{login}@users.noreply.github.com"


def resolve_head_ref(token: str, repo: str, pr_number: int) -> str:
    pr = github_request(token, f"/repos/{repo}/pulls/{pr_number}")
    assert isinstance(pr, dict)
    head_ref = pr.get("head", {}).get("ref")
    if not head_ref:
        raise RuntimeError(f"Could not resolve head ref for PR #{pr_number}")
    return head_ref


def branch_has_cursor_authors() -> bool:
    output = subprocess.check_output(
        ["git", "log", "--format=%ae", "HEAD"], text=True, cwd=REPO_ROOT
    )
    return any(
        email.strip().lower() == CURSOR_AUTHOR_EMAIL.lower()
        for email in output.splitlines()
    )


def rewrite_branch(fallback_name: str, fallback_email: str) -> bool:
    if not branch_has_cursor_authors():
        return False

    env = os.environ.copy()
    env["FALLBACK_AUTHOR_NAME"] = fallback_name
    env["FALLBACK_AUTHOR_EMAIL"] = fallback_email
    env["GIT_SEQUENCE_EDITOR"] = "true"
    subprocess.run(
        [
            "git",
            "rebase",
            "--root",
            "--exec",
            f"python3 {EXEC_HELPER}",
        ],
        cwd=REPO_ROOT,
        env=env,
        check=True,
    )
    return True


def push_branch(remote_ref: str) -> None:
    subprocess.run(
        ["git", "push", "origin", f"HEAD:{remote_ref}", "--force-with-lease"],
        cwd=REPO_ROOT,
        check=True,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pr", type=int, required=True, help="Pull request number")
    parser.add_argument(
        "--push",
        action="store_true",
        help="Force-push rewritten branch back to the PR head ref",
    )
    parser.add_argument(
        "--push-ref",
        help="Override remote ref to update, e.g. refs/heads/work/my-branch",
    )
    parser.add_argument("--check", action="store_true", help="Report only; do not rewrite")
    args = parser.parse_args()

    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    repo = os.environ.get("GITHUB_REPOSITORY")
    if not token or not repo:
        print("GITHUB_TOKEN and GITHUB_REPOSITORY are required", file=sys.stderr)
        return 2

    fallback_name, fallback_email = resolve_fallback_author(token, repo, args.pr)
    needs_fix = branch_has_cursor_authors()
    result = {
        "pr": args.pr,
        "needs_fix": needs_fix,
        "fallback_author": f"{fallback_name} <{fallback_email}>",
        "rewritten": False,
        "pushed": False,
    }

    if not needs_fix:
        print(json.dumps(result, indent=2))
        return 0

    if args.check:
        print(json.dumps(result, indent=2))
        return 1

    result["rewritten"] = rewrite_branch(fallback_name, fallback_email)
    push_ref = args.push_ref
    if args.push and result["rewritten"] and not push_ref:
        push_ref = f"refs/heads/{resolve_head_ref(token, repo, args.pr)}"
    if push_ref and result["rewritten"]:
        push_branch(push_ref)
        result["pushed"] = True
        result["push_ref"] = push_ref

    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
