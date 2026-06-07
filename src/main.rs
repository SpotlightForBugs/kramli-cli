mod api;
mod cli;
mod config;
mod i18n;
mod mcp;
mod models;
mod output;
mod telemetry;

use clap::Parser;
use cli::Cli;
use i18n::tr;

#[tokio::main]
async fn main() {
    // Crash/error telemetry is opt-out. We deliberately disable PII so the
    // CLI never ships OS usernames, IPs, or hostnames, and we scrub API keys
    // and response bodies from event payloads before they leave the machine.
    let _guard = if telemetry::is_enabled() {
        Some(sentry::init((
            "https://9435ede2d0d8eceedf3b3e0eb5cb6aff@o4509985277018112.ingest.de.sentry.io/4510966154002512",
            sentry::ClientOptions {
                release: sentry::release_name!(),
                send_default_pii: false,
                before_send: Some(std::sync::Arc::new(telemetry::scrub_event)),
                ..Default::default()
            },
        )))
    } else {
        None
    };

    let cli = Cli::parse();

    if let Err(e) = cli::run(cli).await {
        if _guard.is_some() {
            sentry::capture_message(&telemetry::scrub_message(&e), sentry::Level::Error);
        }
        eprintln!("\x1b[31m{}\x1b[0m {e}", tr("main-error-prefix"));
        std::process::exit(1);
    }
}
