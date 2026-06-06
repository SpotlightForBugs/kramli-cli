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
cargo test
cargo run -- lists list
```

## Releases

Tags like `v0.1.0` trigger [GitHub Actions](.github/workflows/release.yml) via [cargo-dist](https://github.com/axodotdev/cargo-dist): builds for macOS (Intel + Apple Silicon), Linux (x86_64 + arm64), and Windows, publishes GitHub Releases with checksums and artifact attestations, and updates the Homebrew tap.

See [docs/RELEASE.md](docs/RELEASE.md) for maintainer setup (signing, notarization, secrets).

## License

MIT
