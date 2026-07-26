# AGENTS.md

Repository-specific instructions for agents and contributors working in `kramli-cli`.

## Release version policy

Before pushing a release tag, the project version must already match that tag.

- Source of truth files:
  - `Cargo.toml` -> `[package].version`
  - `Cargo.lock` -> `[[package]] name = "kramli"` -> `version`
- Why this matters:
  - `cargo-dist` plans releases from the crate version.
  - If tag and crate version differ, release workflows fail in the `dist host --steps=create --tag=...` step.

## Decision: automate the version update

Use the release version script instead of manual edits.

- Set version from a tag:
  - `python3 scripts/set_release_version.py --tag v0.1.8`
- Or set directly:
  - `python3 scripts/set_release_version.py --version 0.1.8`
- Check-only mode (no file changes):
  - `python3 scripts/set_release_version.py --tag v0.1.8 --check`

The script updates both `Cargo.toml` and `Cargo.lock` together and prevents common mismatch errors.

## Pre-tag release checklist

1. Sync version files:
   - `python3 scripts/set_release_version.py --tag vX.Y.Z`
2. Run quality gates:
   - `cargo test`
   - `cargo clippy -- -D warnings`
3. Commit with an open-source quality message (Conventional Commits).
4. Push commit to `main`.
5. Create and push annotated tag:
   - `git tag -a vX.Y.Z -m "vX.Y.Z"`
   - `git push origin vX.Y.Z`

## Commit message quality

- Use Conventional Commits (`feat:`, `fix:`, `chore:`, `docs:`, etc.).
- Keep subject line clear and specific.
- For release/process changes, include a short body with rationale and impact.

## Pull request hygiene

Keep pull requests free of Cursor/Bugbot branding:

- Do **not** add `Co-authored-by: Cursor <cursoragent@cursor.com>` trailers. The `.githooks/commit-msg` hook strips them locally, but **Cursor does not run repo hooks** unless you manually run `git config core.hooksPath .githooks` and Cursor happens to honor it (cloud agents generally do not).
- Use `work/<topic>-<suffix>` branch names, not `cursor/...`.
- Cloud-agent PR bodies must not include `CURSOR_AGENT_PR_BODY_*` wrappers, `cursor.com/agents/...` links, or Bugbot upsell text.

The `clean-pr` GitHub Actions workflow enforces this server-side:

- **`fix-commits` job** — rewrites any `cursoragent@cursor.com` commits on the PR branch (author taken from a human `Co-authored-by` trailer or the PR opener), strips Cursor co-author lines, and force-pushes the branch.
- **`clean` job** — scrubs PR descriptions and deletes Bugbot comments from the `cursor` bot.

Fork PRs may not be rewriteable by `GITHUB_TOKEN` (same-repo branches only).

## Test coverage and bug discovery

When working toward test coverage, writing tests, or running parallel subagents on a module:

### Fix bad behavior — do not work around it

If tests reveal incorrect behavior, panics, deadlocks, hangs, or logic errors in **production code**, **patch the production code**. That is in scope.

- Do **not** weaken assertions, skip branches, or add test-only hacks just to make coverage green while leaving the bug in place.
- A test that **fails because it asserts correct behavior** against a broken implementation is valid work: fix the implementation, then the test passes.
- Small, documented **test hooks** in the module under test (for example `set_test_*_err`) are fine when they are the only practical way to exercise an error path — keep them minimal and local to that module.

### Stay in scope

- Change only production code **directly related** to the behavior you are testing or the task you were assigned.
- Do **not** refactor, reformat, rename, or “clean up” unrelated modules, dependencies, CI, or tooling.
- Do **not** edit files outside your assigned area “while you are here.”
- Subagents assigned to one module must not modify other modules except shared test infrastructure explicitly called out in the task (for example `test_env.rs`, `kramli-test-macros`).

### Test conventions in this repo

- Use `#[kramli_test_macros::test]` and `#[kramli_test_macros::tokio_test]` — not bare `#[test]` / `#[tokio::test]` (timeouts apply to all tests).
- `#[ignore]` is OK on macro-wrapped tests (for example pseudo-terminal subprocess cases); never use plain `#[test]` for ignored tests.
- Name tests descriptively (`*_covers_*_branches`, `*_returns_*`, etc.) — no `cov_*` prefixes.
- Never nest `with_env_vars_async`; use `with_env_vars_async_unlocked` inside an outer env block.
- Do not use `/tmp/` paths in tests (DeepSource RS-S1003).
- Run tests with `cargo test -- --test-threads=1`. Do not pipe test output through `tail` (errors appear at the top; buffering can hide failures).

### Subagent handoff

When orchestrating parallel agents, include in each prompt:

1. **Scope:** exact file(s) or module(s) they may change.
2. **Permission to fix bugs** found while testing those modules.
3. **Prohibition** on out-of-scope edits.
4. **Timeout:** each agent runs only its own test(s), max 20s per test (`KRAMLI_TEST_TIMEOUT_SECS`).
