//! Command-line client for Kramli shopping and todo lists.

mod api;
mod cli;
mod config;
mod i18n;
mod mcp;
mod models;
mod output;
mod telemetry;
mod tui;
#[cfg(test)]
mod test_env;

#[cfg(not(test))]
use clap::Parser;
use cli::{Cli, Commands};
use config::Config;
#[cfg(not(test))]
use dialoguer::Confirm;
use i18n::tr;
#[cfg(not(test))]
use std::io::IsTerminal;
use std::process::ExitCode;

#[cfg(not(test))]
fn main() -> ExitCode {
    run_with_cli(Cli::parse())
}

#[cfg(test)]
fn main() -> ExitCode {
    ExitCode::SUCCESS
}

fn run_with_cli(cli: Cli) -> ExitCode {
    run_with_cli_hooks(
        cli,
        ensure_first_run_preferences,
        || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
        },
    )
}

fn run_with_cli_hooks<F, G>(cli: Cli, ensure_prefs: F, build_runtime: G) -> ExitCode
where
    F: Fn(&Cli) -> Result<(), String>,
    G: Fn() -> Result<tokio::runtime::Runtime, String>,
{
    if let Err(error) = ensure_prefs(&cli) {
        eprintln!("\x1b[31m{}\x1b[0m {error}", tr("main-error-prefix"));
        return ExitCode::FAILURE;
    }

    // Crash/error telemetry is enabled only after explicit env or saved user
    // consent. PII stays disabled and every event is scrubbed before it leaves
    // the machine.
    let guard = init_telemetry();
    let runtime = match build_runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("\x1b[31m{}\x1b[0m {error}", tr("main-error-prefix"));
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = runtime.block_on(cli::run(cli)) {
        if guard.is_some() && telemetry::should_capture_command_error(&e) {
            sentry::capture_message(&telemetry::scrub_message(&e), sentry::Level::Error);
        }
        eprintln!("\x1b[31m{}\x1b[0m {e}", tr("main-error-prefix"));
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn init_telemetry() -> Option<sentry::ClientInitGuard> {
    init_telemetry_when(telemetry::is_enabled())
}

fn init_telemetry_when(enabled: bool) -> Option<sentry::ClientInitGuard> {
    enabled.then(|| {
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
                ..sentry::ClientOptions::default()
            },
        ))
    })
}

fn first_run_prompt_blocked(cli: &Cli, stdin_terminal: bool, stdout_terminal: bool) -> bool {
    !should_prompt_first_run_preferences(cli) || !stdin_terminal || !stdout_terminal
}

#[cfg(not(test))]
fn ensure_first_run_preferences(cli: &Cli) -> Result<(), String> {
    ensure_first_run_preferences_with(
        cli,
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
        |prompt, default_value| {
            Confirm::new()
                .with_prompt(prompt)
                .default(default_value)
                .interact()
                .map_err(|error| error.to_string())
        },
    )
}

#[cfg(test)]
fn ensure_first_run_preferences(cli: &Cli) -> Result<(), String> {
    ensure_first_run_preferences_with(cli, false, false, |_, _| Ok(true))
}

fn ensure_first_run_preferences_with<F>(
    cli: &Cli,
    stdin_terminal: bool,
    stdout_terminal: bool,
    mut ask_confirm: F,
) -> Result<(), String>
where
    F: FnMut(&str, bool) -> Result<bool, String>,
{
    if first_run_prompt_blocked(cli, stdin_terminal, stdout_terminal) {
        return Ok(());
    }

    let mut cfg = Config::load();
    let mut changed = false;

    if !cfg.telemetry_preference_set() {
        println!("{}", tr("main-consent-telemetry-title"));
        println!("{}", tr("main-consent-telemetry-body"));
        println!("{}", tr("main-consent-telemetry-change"));
        let enabled = ask_confirm(&tr("main-consent-telemetry-prompt"), true)?;
        cfg.set_telemetry_enabled(enabled);
        changed = true; }

    if !cfg.bootstrap_icons_preference_set() {
        println!();
        println!("{}", tr("main-consent-bootstrap-body"));
        let enabled = ask_confirm(&tr("main-consent-bootstrap-prompt"), true)?;
        cfg.set_bootstrap_icons_enabled(enabled);
        changed = true; }

    if changed { cfg.save()?; }
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
                Commands::Batch { .. }
                    | Commands::Completions { .. }
                    | Commands::Mcp
                    | Commands::Privacy { .. }
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{HandoffCmd, ListCmd, PrivacyCmd};
    use crate::config::Config;
    use clap_complete::Shell;

    const DO_NOT_TRACK_ENV: &str = "DO_NOT_TRACK";

    fn cli_for(command: Option<Commands>) -> Cli {
        Cli {
            json: false,
            interactive: false,
            command,
        }
    }

    #[test]
    fn run_with_cli_hooks_handles_preference_and_runtime_failures() {
        fn runtime_error() -> Result<tokio::runtime::Runtime, String> {
            Err("runtime failed".to_string())
        }

        let cli = cli_for(Some(Commands::Status));

        let preference_failure = run_with_cli_hooks(
            cli_for(Some(Commands::Status)),
            |_| Err("pref failed".to_string()),
            runtime_error,
        );
        assert_eq!(preference_failure, ExitCode::FAILURE);

        let runtime_failure = run_with_cli_hooks(cli, |_| Ok(()), runtime_error);
        assert_eq!(runtime_failure, ExitCode::FAILURE);

        let runtime_success = run_with_cli_hooks(
            cli_for(Some(Commands::Status)),
            |_| Ok(()),
            || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())
            },
        );
        assert_eq!(runtime_success, ExitCode::SUCCESS);
    }

    #[test]
    fn run_with_cli_covers_success_and_error_outcomes() {
        assert_eq!(run_with_cli(cli_for(Some(Commands::Status))), ExitCode::SUCCESS);
        assert_eq!(
            run_with_cli(cli_for(Some(Commands::Lists {
                action: ListCmd::Resolve {
                    reference: " ".to_string()
                }
            }))),
            ExitCode::FAILURE
        );
    }

    #[test]
    fn first_run_prompt_is_disabled_for_non_interactive_paths() {
        let mut cli = cli_for(Some(Commands::Status));
        cli.json = true;
        assert!(!should_prompt_first_run_preferences(&cli));

        assert!(!should_prompt_first_run_preferences(&cli_for(None)));
        assert!(!should_prompt_first_run_preferences(&cli_for(Some(
            Commands::Batch {
                file: "-".to_string(),
                keep_going: false,
            },
        ))));
        assert!(!should_prompt_first_run_preferences(&cli_for(Some(
            Commands::Completions { shell: Shell::Bash },
        ))));
        assert!(!should_prompt_first_run_preferences(&cli_for(Some(
            Commands::Mcp
        ))));
        assert!(!should_prompt_first_run_preferences(&cli_for(Some(
            Commands::Privacy {
                action: PrivacyCmd::Reset,
            },
        ))));
    }

    #[test]
    fn first_run_prompt_is_enabled_for_interactive_or_regular_commands() {
        let mut cli = cli_for(None);
        cli.interactive = true;
        assert!(should_prompt_first_run_preferences(&cli));

        assert!(should_prompt_first_run_preferences(&cli_for(Some(
            Commands::Status,
        ))));
        assert!(should_prompt_first_run_preferences(&cli_for(Some(
            Commands::Handoff {
                action: HandoffCmd::Clear,
            },
        ))));
    }

    #[test]
    fn first_run_preferences_skip_prompt_when_streams_are_not_terminal() {
        let cli = cli_for(Some(Commands::Status));
        assert!(ensure_first_run_preferences(&cli).is_ok());
    }

    #[test]
    fn first_run_prompt_blocker_covers_stream_combinations() {
        let cli = cli_for(Some(Commands::Status));
        assert!(first_run_prompt_blocked(&cli, false, true));
        assert!(first_run_prompt_blocked(&cli, true, false));
        assert!(!first_run_prompt_blocked(&cli, true, true));

        let mut json_cli = cli_for(Some(Commands::Status));
        json_cli.json = true;
        assert!(first_run_prompt_blocked(&json_cli, true, true));
    }

    #[test]
    fn telemetry_init_respects_disable_environment() {
        let guard = init_telemetry_when(false);
        assert!(guard.is_none());
    }

    #[test]
    fn telemetry_init_reads_disabled_environment() {
        crate::test_env::with_env_vars(&[(DO_NOT_TRACK_ENV, "1")], || {
            let guard = init_telemetry();
            assert!(guard.is_none());
        });
    }

    #[test]
    fn telemetry_init_can_enable_guard_from_environment() {
        let guard = init_telemetry_when(true);
        assert!(guard.is_some());
    }

    #[test]
    fn test_main_entrypoint_is_inert_under_cfg_test() {
        assert_eq!(main(), ExitCode::SUCCESS);
    }

    #[test]
    fn run_with_cli_can_capture_errors_when_enabled() {
        crate::test_env::with_env_vars(
            &[("KRAMLI_TELEMETRY", "1"), ("KRAMLI_CAPTURE_COMMAND_ERRORS", "1")],
            || {
                let exit = run_with_cli(cli_for(Some(Commands::Lists {
                    action: ListCmd::Resolve {
                        reference: " ".to_string(),
                    },
                })));
                assert_eq!(exit, ExitCode::FAILURE);
            },
        );
    }

    #[test]
    fn first_run_prompt_helper_covers_confirm_save_and_error_paths() {
        let config_root = std::env::temp_dir().join(format!(
            "kramli-main-first-run-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0_u128, |value| value.as_nanos())
        ));
        std::fs::create_dir_all(&config_root).expect("temp config root should exist");

        crate::test_env::with_env_vars(
            &[
                (
                    "HOME",
                    config_root
                        .to_str()
                        .expect("config root should be valid utf-8"),
                ),
                (
                    "XDG_CONFIG_HOME",
                    config_root
                        .to_str()
                        .expect("config root should be valid utf-8"),
                ),
                ("DO_NOT_TRACK", ""),
                ("KRAMLI_NO_TELEMETRY", ""),
                ("KRAMLI_TELEMETRY", ""),
                ("KRAMLI_BOOTSTRAP_ICONS", ""),
                ("KRAMLI_TUI_BOOTSTRAP_ICONS", ""),
                ("KRAMLI_LOAD_BOOTSTRAP_ICONS", ""),
            ],
            || {
                let cli = cli_for(Some(Commands::Status));
                let mut prompts = Vec::new();
                ensure_first_run_preferences_with(&cli, true, true, |prompt, default| {
                    prompts.push((prompt.to_string(), default));
                    Ok(false)
                })
                .expect("prompt helper should succeed");

                assert_eq!(prompts.len(), 2);
                assert!(prompts.iter().all(|(_, default)| *default));

                let saved = Config::load();
                assert!(saved.telemetry_preference_set());
                assert!(saved.bootstrap_icons_preference_set());
                assert!(!saved.telemetry_enabled());
                assert!(!saved.bootstrap_icons_enabled());

                let _ = std::fs::remove_file(Config::path());

                let failing = ensure_first_run_preferences_with(
                    &cli_for(Some(Commands::Handoff {
                        action: HandoffCmd::Clear,
                    })),
                    true,
                    true,
                    |_, _| Err("prompt failed".to_string()),
                );
                assert!(failing.is_err());
            },
        );
    }
}
