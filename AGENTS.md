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

- Do **not** add `Co-authored-by: Cursor <cursoragent@cursor.com>` trailers (`.githooks/commit-msg` strips them locally; run `git config core.hooksPath .githooks` after clone).
- Use `work/<topic>-<suffix>` branch names, not `cursor/...`.
- Cloud-agent PR bodies must not include `CURSOR_AGENT_PR_BODY_*` wrappers, `cursor.com/agents/...` links, or Bugbot upsell text.

The `clean-pr` GitHub Actions workflow scrubs those artifacts from PR descriptions and deletes Bugbot comments posted by the `cursor` bot. It cannot rewrite commit authorship on already-pushed commits.
