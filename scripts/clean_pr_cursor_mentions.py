#!/usr/bin/env python3
"""Scrub Cursor/Bugbot noise from GitHub pull requests.

Removes cloud-agent PR body wrappers, agent links, and Bugbot upsell comments.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.request

CURSOR_AGENT_BLOCK_RE = re.compile(
    r"<!--\s*CURSOR_AGENT_PR_BODY_BEGIN\s*-->.*?<!--\s*CURSOR_AGENT_PR_BODY_END\s*-->",
    re.DOTALL | re.IGNORECASE,
)
CURSOR_AGENT_LINK_RE = re.compile(
    r"\n*---\n*\s*(?:\[.*?\]\()?https?://(?:www\.)?cursor\.com/agents/[^\s\)\]]+.*",
    re.IGNORECASE,
)
CURSOR_FOOTER_RE = re.compile(
    r"\n*(?:Made with \[?Cursor\]?|Powered by \[?Cursor\]?|"
    r"Open in \[?Cursor\]?|View (?:this )?run (?:in|on) \[?Cursor\]?)"
    r"[^\n]*\n*",
    re.IGNORECASE,
)
TRAILING_CURSOR_LINK_RE = re.compile(
    r"\n*https?://(?:www\.)?cursor\.com/[^\s\)\]]+\s*",
    re.IGNORECASE,
)
CURSOR_NOISE_AUTHORS = frozenset({"cursor", "cursoragent", "cursor-agent"})
BUGBOT_MARKERS = ("BUGBOT_", "cursor.com/dashboard/bugbot")


def scrub_pr_body(body: str | None) -> tuple[str, bool]:
    """Return cleaned PR body and whether it changed."""
    if not body:
        return body or "", False

    cleaned = body
    for pattern in (
        CURSOR_AGENT_BLOCK_RE,
        CURSOR_AGENT_LINK_RE,
        CURSOR_FOOTER_RE,
        TRAILING_CURSOR_LINK_RE,
    ):
        cleaned = pattern.sub("\n", cleaned)

    cleaned = re.sub(r"\n{3,}", "\n\n", cleaned).strip()
    changed = cleaned != body.strip()
    return cleaned, changed


def is_cursor_noise_comment(author_login: str, body: str | None) -> bool:
    login = (author_login or "").lower()
    text = body or ""
    if login in CURSOR_NOISE_AUTHORS:
        return True
    upper = text.upper()
    lower = text.lower()
    if any(marker in upper for marker in BUGBOT_MARKERS):
        return True
    if "bugbot" in lower and "cursor.com" in lower:
        return True
    return False


class GitHubClient:
    def __init__(self, token: str, repo: str) -> None:
        self.token = token
        self.repo = repo

    def _request(
        self,
        method: str,
        path: str,
        payload: dict | None = None,
    ) -> dict | list | None:
        url = f"https://api.github.com{path}"
        data = None
        headers = {
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {self.token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "kramli-cli-clean-pr-script",
        }
        if payload is not None:
            data = json.dumps(payload).encode("utf-8")
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(request) as response:
                raw = response.read().decode("utf-8")
                if not raw:
                    return None
                return json.loads(raw)
        except urllib.error.HTTPError as error:
            body = error.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"GitHub API {method} {path} failed: {error.code} {body}") from error

    def get_pull_request(self, number: int) -> dict:
        result = self._request("GET", f"/repos/{self.repo}/pulls/{number}")
        assert isinstance(result, dict)
        return result

    def update_pull_request_body(self, number: int, body: str) -> None:
        self._request("PATCH", f"/repos/{self.repo}/pulls/{number}", {"body": body})

    def list_issue_comments(self, number: int) -> list[dict]:
        result = self._request("GET", f"/repos/{self.repo}/issues/{number}/comments")
        assert isinstance(result, list)
        return result

    def delete_issue_comment(self, comment_id: int) -> None:
        self._request("DELETE", f"/repos/{self.repo}/issues/comments/{comment_id}")

    def list_open_pull_requests(self) -> list[dict]:
        result = self._request("GET", f"/repos/{self.repo}/pulls?state=open&per_page=100")
        assert isinstance(result, list)
        return result


def clean_pull_request(client: GitHubClient, number: int, dry_run: bool = False) -> dict:
    pr = client.get_pull_request(number)
    body = pr.get("body") or ""
    cleaned_body, body_changed = scrub_pr_body(body)

    comments = client.list_issue_comments(number)
    noisy_comments = [
        comment
        for comment in comments
        if is_cursor_noise_comment(
            (comment.get("user") or {}).get("login", ""),
            comment.get("body"),
        )
    ]

    if body_changed and not dry_run:
        client.update_pull_request_body(number, cleaned_body)

    deleted_comment_ids: list[int] = []
    if not dry_run:
        for comment in noisy_comments:
            comment_id = comment["id"]
            client.delete_issue_comment(comment_id)
            deleted_comment_ids.append(comment_id)

    return {
        "number": number,
        "body_changed": body_changed,
        "deleted_comments": deleted_comment_ids or [c["id"] for c in noisy_comments],
        "dry_run": dry_run,
    }


def run_self_test() -> None:
    body = (
        "Real summary\n\n"
        "<!-- CURSOR_AGENT_PR_BODY_BEGIN -->\n"
        "Agent junk\n"
        "<!-- CURSOR_AGENT_PR_BODY_END -->\n\n"
        "---\n"
        "[View run](https://cursor.com/agents/abc123)\n"
    )
    cleaned, changed = scrub_pr_body(body)
    assert changed
    assert "CURSOR_AGENT" not in cleaned
    assert "cursor.com" not in cleaned
    assert "Real summary" in cleaned

    assert is_cursor_noise_comment(
        "cursor",
        "Enable Bugbot in the Cursor dashboard",
    )
    assert not is_cursor_noise_comment("deepsource-io", "Rust review passed")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--pr",
        type=int,
        action="append",
        dest="prs",
        help="Pull request number to clean (repeatable)",
    )
    parser.add_argument(
        "--all-open",
        action="store_true",
        help="Clean every open pull request",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Report needed changes without editing GitHub",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="Run built-in scrubbing checks and exit",
    )
    args = parser.parse_args()

    if args.self_test:
        run_self_test()
        print("self-test: ok")
        return 0

    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    repo = os.environ.get("GITHUB_REPOSITORY")
    if not token or not repo:
        print("GITHUB_TOKEN and GITHUB_REPOSITORY are required", file=sys.stderr)
        return 2

    client = GitHubClient(token=token, repo=repo)
    pr_numbers = list(args.prs or [])
    if args.all_open:
        pr_numbers.extend(pr["number"] for pr in client.list_open_pull_requests())
    if not pr_numbers:
        print("No pull requests selected. Use --pr or --all-open.", file=sys.stderr)
        return 2

    results = []
    for number in sorted(set(pr_numbers)):
        results.append(clean_pull_request(client, number, dry_run=args.check))

    print(json.dumps(results, indent=2))
    if args.check and any(r["body_changed"] or r["deleted_comments"] for r in results):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
