<p align="center">
  <img src=".github/assets/logo.svg" alt="Kramli" width="96" height="96">
</p>

<h1 align="center">kramli-cli</h1>

<p align="center">
  Command-line client for <a href="https://kramli.de">Kramli</a> — shopping lists, todos, and shared lists from your terminal.
</p>

<p align="center">
  <img src=".github/assets/og-image.jpg" alt="Kramli app preview" width="640">
</p>

<p align="center">
  <img src=".github/assets/cli-screenshot.png" alt="kramli CLI showing lists and items in a terminal" width="800">
</p>

## Install

**Homebrew (macOS and Linux)**

```bash
brew install SpotlightForBugs/tap/kramli
brew trust --formula spotlightforbugs/tap/kramli
```

**Install script (macOS, Linux, Windows via WSL)**

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/SpotlightForBugs/kramli-cli/releases/latest/download/kramli-installer.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://github.com/SpotlightForBugs/kramli-cli/releases/latest/download/kramli-installer.ps1 | iex
```

Or download a build for your platform from [Releases](https://github.com/SpotlightForBugs/kramli-cli/releases), extract `kramli`, and put it on your `PATH`.

## Quick start

```bash
kramli login
kramli lists list
kramli items list <LIST_ID> --open
kramli items add <LIST_ID> "Milch" --priority high
kramli search "dark mode"
```

Create an API key at [kramli.de/settings#api-keys](https://kramli.de/settings#api-keys) if you prefer `kramli login --api-key`.

## Localization

- Default/fallback language is English (`en`).
- Locale priority: `KRAMLI_LANG` -> profile language (`/api/profile.lang`) -> `LC_ALL` -> `LC_MESSAGES` -> `LANG` -> `en`.
- Supported language codes: `en`, `de`, `fr`, `es`, `it`, `nl`, `pl`, `pt`, `ru`, `tr`, `uk`, `ar`, `ja`, `ko`, `zh`.
- Icon style for terminal output: `KRAMLI_ICON_STYLE=label` (default), `emoji`, or `raw`.

Examples:

```bash
KRAMLI_LANG=de kramli status
KRAMLI_LANG=fr kramli lists list
KRAMLI_LANG=pt_BR kramli profile
KRAMLI_ICON_STYLE=emoji kramli lists list
```

## License

MIT
