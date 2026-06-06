# kramli-cli

Command-line client for [Kramli](https://kramli.de) shopping lists and todos.

## Install

### Homebrew (macOS and Linux)

```bash
brew install SpotlightForBugs/tap/kramli
```

### Install script

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/SpotlightForBugs/kramli-cli/releases/latest/download/kramli-installer.sh | sh
```

### GitHub Releases

Download the archive for your platform from [Releases](https://github.com/SpotlightForBugs/kramli-cli/releases), extract `kramli`, and put it on your `PATH`.

## Quick start

```bash
kramli login
kramli lists list
kramli items list <LIST_ID> --open
```

Generate an API key at [kramli.de/settings#api-keys](https://kramli.de/settings#api-keys).

## Development

```bash
git config core.hooksPath .githooks   # once per clone — strips Cursor co-author trailers
cargo test
cargo run -- lists list
```

## Releases

Tags like `v0.1.0` trigger [GitHub Actions](.github/workflows/release.yml) via [cargo-dist](https://github.com/axodotdev/cargo-dist): builds for macOS (Intel + Apple Silicon), Linux (x86_64 + arm64), and Windows, publishes GitHub Releases with checksums and artifact attestations, and updates the [Homebrew tap](https://github.com/SpotlightForBugs/homebrew-tap).

### Maintainer secrets (`kramli-cli` repo)

| Secret | Purpose |
|--------|---------|
| `APPLE_CERTIFICATE` | Base64 `.p12` Developer ID Application cert |
| `APPLE_CERTIFICATE_PASSWORD` | `.p12` password |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Name (TEAMID)` |
| `APPLE_NOTARY_KEY` | Base64 App Store Connect API `.p8` key |
| `APPLE_NOTARY_KEY_ID` | API key ID |
| `APPLE_NOTARY_ISSUER` | Issuer UUID from App Store Connect |
| `HOMEBREW_TAP_TOKEN` | PAT with push access to `homebrew-tap` |

Signing and notarization run only on tag releases, not PR dry-runs.

After changing `dist-workspace.toml`, run `dist generate` and re-apply the `BEGIN CUSTOM` blocks in `.github/workflows/release.yml`.

## License

MIT
