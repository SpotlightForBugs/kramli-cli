mod api;
mod cli;
mod config;
mod i18n;
mod mcp;
mod models;
mod output;
mod telemetry;
mod tui;

use clap::Parser;
use cli::{Cli, Commands};
use config::Config;
use dialoguer::Confirm;
use i18n::tr;
use std::io::IsTerminal;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Err(error) = ensure_first_run_preferences(&cli) {
        eprintln!("\x1b[31m{}\x1b[0m {error}", tr("main-error-prefix"));
        return ExitCode::FAILURE;
    }

    // Crash/error telemetry is enabled only after explicit env or saved user
    // consent. PII stays disabled and every event is scrubbed before it leaves
    // the machine.
    let guard = init_telemetry();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("\x1b[31m{}\x1b[0m {error}", tr("main-error-prefix"));
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = runtime.block_on(cli::run(cli)) {
        if guard.is_some() {
            sentry::capture_message("cli.command.failed", sentry::Level::Error);
        }
        eprintln!("\x1b[31m{}\x1b[0m {e}", tr("main-error-prefix"));
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn init_telemetry() -> Option<sentry::ClientInitGuard> {
    telemetry::is_enabled().then(|| {
        sentry::init((
            "https://9435ede2d0d8eceedf3b3e0eb5cb6aff@o4509985277018112.ingest.de.sentry.io/4510966154002512",
            sentry::ClientOptions {
                release: sentry::release_name!(),
                send_default_pii: false,
                attach_stacktrace: false,
                max_breadcrumbs: 0,
                default_integrations: true,
                auto_session_tracking: false,
                traces_sample_rate: telemetry::traces_sample_rate(),
                before_send: Some(std::sync::Arc::new(telemetry::scrub_event)),
                ..Default::default()
            },
        ))
    })
}

fn ensure_first_run_preferences(cli: &Cli) -> Result<(), String> {
    if !should_prompt_first_run_preferences(cli)
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
    {
        return Ok(());
    }

    let mut cfg = Config::load();
    let mut changed = false;

    if !cfg.telemetry_preference_set() {
        println!("{}", tr("main-consent-telemetry-title"));
        println!("{}", tr("main-consent-telemetry-body"));
        println!("{}", tr("main-consent-telemetry-change"));
        let enabled = Confirm::new()
            .with_prompt(tr("main-consent-telemetry-prompt"))
            .default(true)
            .interact()
            .map_err(|error| error.to_string())?;
        cfg.set_telemetry_enabled(enabled);
        changed = true;
    }

    if !cfg.bootstrap_icons_preference_set() {
        println!();
        println!("{}", tr("main-consent-bootstrap-body"));
        let enabled = Confirm::new()
            .with_prompt(tr("main-consent-bootstrap-prompt"))
            .default(true)
            .interact()
            .map_err(|error| error.to_string())?;
        cfg.set_bootstrap_icons_enabled(enabled);
        changed = true;
    }

    if changed {
        cfg.save()?;
    }
    Ok(())
}

fn should_prompt_first_run_preferences(cli: &Cli) -> bool {
    if cli.json {
        return false;
    }
    if cli.interactive {
        return true;
    }
    matches!(
        cli.command.as_ref(),
        Some(command)
            if !matches!(
                command,
                Commands::Batch { .. } | Commands::Completions { .. } | Commands::Mcp
            )
    )
}
