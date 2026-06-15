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
use cli::Cli;
use i18n::tr;
use std::process::ExitCode;

fn main() -> ExitCode {
    // Crash/error telemetry is opt-in. PII stays disabled and every event is
    // scrubbed before it leaves the machine.
    let _guard = if telemetry::is_enabled() {
        Some(sentry::init((
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
        )))
    } else {
        None
    };

    let cli = Cli::parse();
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
        if _guard.is_some() {
            sentry::capture_message("cli.command.failed", sentry::Level::Error);
        }
        eprintln!("\x1b[31m{}\x1b[0m {e}", tr("main-error-prefix"));
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
