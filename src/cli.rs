use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use colored::control::set_override;
use colored::Colorize;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::Command as TokioCommand;

use crate::api::ApiClient;
use crate::config::Config;
use crate::i18n::{apply_profile_locale, current_locale_code, is_explicit_lang_set, tr, tr_args};
use crate::models::*;
use crate::output;
use crate::telemetry;

const NO_COLOR_ENV: &str = "NO_COLOR";
const KRAMLI_DEVICE_LABEL_ENV: &str = "KRAMLI_DEVICE_LABEL";
const KRAMLI_ACK_TOKEN_ENV: &str = "KRAMLI_ACK_TOKEN";

#[derive(Parser)]
#[command(
    name = "kramli",
    about = "Kramli - shopping list and todo CLI",
    version,
    propagate_version = true
)]
/// Parsed command-line interface for `kramli`.
pub(crate) struct Cli {
    /// Output machine-readable JSON instead of human-friendly text.
    #[arg(long, global = true)]
    pub(crate) json: bool,

    /// Start a full-screen terminal UI
    #[arg(short = 'i', long)]
    pub(crate) interactive: bool,

    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

#[derive(Subcommand)]
/// Top-level CLI commands.
pub(crate) enum Commands {
    /// Log in with an API key (create one at kramli.de/settings#api-keys)
    Login {
        /// Server URL (default: https://kramli.de)
        #[arg(long)]
        url: Option<String>,
    },
    /// Log out and remove stored credentials
    Logout,
    /// Show login status and profile
    Status,
    /// Manage lists
    #[command(alias = "ls")]
    Lists {
        #[command(subcommand)]
        action: ListCmd,
    },
    /// Manage items in a list
    #[command(alias = "i")]
    Items {
        #[command(subcommand)]
        action: Box<ItemCmd>,
    },
    /// Manage folders
    Folders {
        #[command(subcommand)]
        action: FolderCmd,
    },
    /// Manage members and sharing
    #[command(alias = "share")]
    Members {
        #[command(subcommand)]
        action: MemberCmd,
    },
    /// Manage API keys (create, list, revoke)
    Keys {
        #[command(subcommand)]
        action: KeyCmd,
    },
    /// Search across all lists and items
    Search {
        /// Search query (minimum 2 characters)
        query: String,
    },
    /// Show activity feed for a list
    Activity {
        /// List ID
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
        /// Number of entries
        #[arg(short, long, default_value = "20")]
        limit: u32,
    },
    /// Undo the last action on a list
    Undo {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
    },
    /// Redo the last undone action on a list
    Redo {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
    },
    /// Show profile
    Profile,
    /// Account security level and login confirmation
    Security {
        #[command(subcommand)]
        action: SecurityCmd,
    },
    /// Accept pending terms/privacy documents
    #[command(name = "accept-terms")]
    AcceptTerms {
        /// Optional document keys (comma-separated): agb,privacy
        #[arg(long, value_delimiter = ',')]
        docs: Option<Vec<String>>,
    },
    /// Send or clear cross-device activity handoff state
    Handoff {
        #[command(subcommand)]
        action: HandoffCmd,
    },
    /// Check server connectivity
    Ping,
    /// Show CLI configuration
    Config,
    /// Check whether a newer kramli CLI release is available
    #[command(name = "update-check", alias = "update")]
    UpdateCheck,
    /// Manage local privacy preferences
    Privacy {
        #[command(subcommand)]
        action: PrivacyCmd,
    },
    /// Run a local stdio MCP server using the CLI login
    Mcp,
    /// Run multiple CLI commands from a file or stdin
    Batch {
        /// File path with one command per line (`-` reads stdin)
        #[arg(default_value = "-")]
        file: String,
        /// Continue after errors and report all failures
        #[arg(short = 'k', long)]
        keep_going: bool,
    },
    /// Generate shell completions
    Completions {
        /// Shell to generate for
        shell: Shell,
    },
}

const UPDATE_CHECK_URL: &str =
    "https://api.github.com/repos/SpotlightForBugs/kramli-cli/releases/latest";
const UPDATE_CHECK_INTERVAL_SECS: i64 = 60 * 60 * 24;

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SemverTriplet {
    major: u64,
    minor: u64,
    patch: u64,
}

// ─── Subcommands ───

#[derive(Subcommand)]
/// List management subcommands.
pub(crate) enum ListCmd {
    /// List all lists
    #[command(alias = "ls")]
    List,
    /// Resolve a list reference (ID, /lists/l/<slug>, or full URL)
    Resolve { reference: String },
    /// Show list details
    Show {
        #[arg(value_parser = resolve_list_reference)]
        id: i64,
    },
    /// Create a list
    Create {
        name: String,
        #[arg(short, long)]
        icon: Option<String>,
        #[arg(short, long)]
        color: Option<String>,
        #[arg(short, long)]
        folder: Option<i64>,
        /// Custom states as CSV (e.g. "Open,In Progress,Review,Done")
        /// or as JSON array (e.g. '[{"name":"Review","color":"#7c3aed"}]').
        #[arg(long)]
        states: Option<String>,
    },
    /// Update a list
    Update {
        #[arg(value_parser = resolve_list_reference)]
        id: i64,
        #[arg(short, long)]
        name: Option<String>,
        #[arg(short, long)]
        icon: Option<String>,
        #[arg(short, long)]
        color: Option<String>,
        /// Custom states as CSV (e.g. "Open,In Progress,Review,Done")
        /// or as JSON array (e.g. '[{"name":"Review","color":"#7c3aed"}]').
        #[arg(long)]
        states: Option<String>,
    },
    /// Delete a list
    #[command(alias = "rm")]
    Delete {
        #[arg(value_parser = resolve_list_reference)]
        id: i64,
    },
    /// Move a list to a folder
    Move {
        #[arg(value_parser = resolve_list_reference)]
        id: i64,
        /// Folder ID (omit to remove the list from a folder)
        folder_id: Option<i64>,
    },
}

const LIST_ID_XOR_KEY: i64 = 0x5A3F_1D7E;
const B62_ALPHABET: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn decode_list_slug(slug: &str) -> Option<i64> {
    let mut n: i64 = 0;
    for ch in slug.chars() {
        let idx = B62_ALPHABET.find(ch)? as i64;
        n = n.checked_mul(62)?.checked_add(idx)?;
    }
    Some(n ^ LIST_ID_XOR_KEY)
}

fn extract_slug_from_reference(reference: &str) -> Option<String> {
    let raw = reference.trim();
    if raw.is_empty() {
        return None;
    }

    // Full URL or path containing /lists/l/<slug>
    if let Some(pos) = raw.find("/lists/l/") {
        let after = &raw[(pos + 9)..];
        let slug: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if !slug.is_empty() {
            return Some(slug);
        }
    }

    // Bare slug
    if raw.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Some(raw.to_string());
    }

    None
}

fn resolve_list_reference(reference: &str) -> Result<i64, String> {
    let raw = reference.trim();
    if raw.is_empty() {
        return Err(tr("cli-list-reference-empty"));
    }

    if let Ok(id) = raw.parse::<i64>() {
        return Ok(id);
    }

    if let Some(slug) = extract_slug_from_reference(raw) {
        if let Some(id) = decode_list_slug(&slug) {
            return Ok(id);
        }
    }

    Err(tr("cli-list-reference-invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KRAMLI_LANG_ENV: &str = "KRAMLI_LANG";

    fn encode_list_slug(id: i64) -> String {
        let mut n = id ^ LIST_ID_XOR_KEY;
        if n == 0 {
            return "0".to_string();
        }
        let mut chars = Vec::new();
        while n > 0 {
            let idx = (n % 62) as usize;
            chars.push(B62_ALPHABET.as_bytes()[idx] as char);
            n /= 62;
        }
        chars.iter().rev().collect()
    }

    #[test]
    fn resolves_numeric_list_references() {
        assert_eq!(resolve_list_reference("46"), Ok(46));
    }

    #[test]
    fn resolves_private_list_urls_with_hash_item_fragments() {
        let slug = encode_list_slug(46);

        assert_eq!(
            resolve_list_reference(&format!("https://kramli.de/lists/l/{slug}#item-123")),
            Ok(46)
        );
    }

    #[test]
    fn parses_items_list_with_private_url_reference() {
        let slug = encode_list_slug(46);
        let cli = Cli::try_parse_from([
            "kramli",
            "items",
            "list",
            &format!("https://kramli.de/lists/l/{slug}#item-5968"),
        ])
        .expect("items list with URL reference should parse");

        assert!(matches!(
            cli.command,
            Some(Commands::Items { action })
                if matches!(*action, ItemCmd::List { list_id: 46, .. })
        ));
    }

    #[test]
    fn parses_lists_show_with_private_url_reference() {
        let slug = encode_list_slug(46);
        let cli = Cli::try_parse_from([
            "kramli",
            "lists",
            "show",
            &format!("https://kramli.de/lists/l/{slug}#item-5968"),
        ])
        .expect("lists show with URL reference should parse");

        assert!(matches!(
            cli.command,
            Some(Commands::Lists {
                action: ListCmd::Show { id: 46 }
            })
        ));
    }

    #[test]
    fn rejects_empty_list_references() {
        assert!(resolve_list_reference("  ").is_err());
    }

    #[test]
    fn batch_child_args_strips_program_name() {
        assert_eq!(batch_child_args("kramli ping").unwrap(), vec!["ping"]);
    }

    #[test]
    fn batch_child_args_forces_json_once() {
        assert_eq!(
            batch_child_args("kramli --json ping").unwrap(),
            vec!["ping"]
        );
    }

    #[test]
    fn batch_child_args_rejects_nested_batch() {
        assert!(batch_child_args("batch -").is_err());
    }

    #[test]
    fn parses_interactive_mode_without_command() {
        let cli = Cli::try_parse_from(["kramli", "-i"]).expect("interactive parse should work");
        assert!(cli.interactive);
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_regular_mode_with_command() {
        let cli = Cli::try_parse_from(["kramli", "status"]).expect("status parse should work");
        assert!(!cli.interactive);
        assert!(matches!(cli.command, Some(Commands::Status)));
    }

    #[test]
    fn rejects_interactive_flag_after_subcommand() {
        assert!(Cli::try_parse_from(["kramli", "status", "--interactive"]).is_err());
    }

    #[test]
    fn parses_boxed_items_subcommand() {
        let cli = Cli::try_parse_from([
            "kramli",
            "items",
            "update",
            "123",
            "--reminder-time",
            "09:00",
        ])
        .expect("items update parse should work");

        assert!(matches!(
            cli.command,
            Some(Commands::Items { action })
                if matches!(
                    *action,
                    ItemCmd::Update {
                        id: 123,
                        reminder_time: Some(ref time),
                        ..
                    } if time == "09:00"
                )
        ));
    }

    #[test]
    fn parses_update_check_command() {
        let cli = Cli::try_parse_from(["kramli", "update-check"])
            .expect("update-check parse should work");
        assert!(matches!(cli.command, Some(Commands::UpdateCheck)));
    }

    #[test]
    fn parses_manual_handoff_command_for_compatibility() {
        let cli = Cli::try_parse_from(["kramli", "handoff", "clear"])
            .expect("handoff clear parse should work");
        assert!(matches!(
            cli.command,
            Some(Commands::Handoff {
                action: HandoffCmd::Clear
            })
        ));
    }

    #[test]
    fn parses_privacy_reset_command() {
        let cli = Cli::try_parse_from(["kramli", "privacy", "reset"])
            .expect("privacy reset parse should work");
        assert!(matches!(
            cli.command,
            Some(Commands::Privacy {
                action: PrivacyCmd::Reset
            })
        ));
    }

    #[test]
    fn handoff_payload_does_not_request_integration_open() {
        let body = handoff_body(42, Some("Groceries".to_string()), "Kramli CLI".to_string());
        assert_eq!(body.get("list_id"), Some(&Value::from(42)));
        assert_eq!(
            body.get("list_name"),
            Some(&Value::String("Groceries".to_string()))
        );
        assert_eq!(
            body.get("device_label"),
            Some(&Value::String("Kramli CLI".to_string()))
        );
        assert!(body.get("integration").is_none());
    }

    #[test]
    fn compares_semver_triplets() {
        assert_eq!(
            parse_semver_triplet("v0.1.8"),
            Some(SemverTriplet {
                major: 0,
                minor: 1,
                patch: 8,
            })
        );
        assert_eq!(update_is_available("0.1.8", "v0.2.0"), Some(true));
        assert_eq!(update_is_available("0.1.8", "0.1.8"), Some(false));
        assert_eq!(update_is_available("0.1.8", "invalid"), None);
    }

    #[test]
    fn invite_url_prefers_server_url_and_supports_tokens() {
        assert_eq!(
            invite_url_from_response(&json!({"invite_url": "https://kram.li/i/server"})),
            Some("https://kram.li/i/server".to_string())
        );
        assert_eq!(
            invite_url_from_response(&json!({"invite_token": "abc123"})),
            Some("https://kram.li/i/abc123".to_string())
        );
        assert_eq!(
            invite_url_from_response(&json!({"token": "legacy"})),
            Some("https://kram.li/i/legacy".to_string())
        );
        assert_eq!(invite_url_from_response(&json!({})), None);
    }

    #[test]
    fn reminder_details_enable_reminders_by_default() {
        assert_eq!(effective_reminder_value(None, true), Some(true));
        assert_eq!(effective_reminder_value(Some(true), true), Some(true));
        assert_eq!(effective_reminder_value(Some(false), true), Some(false));
        assert_eq!(effective_reminder_value(None, false), None);
    }

    #[test]
    fn reminder_details_exclude_travel_time_semantics() {
        assert!(!reminder_details_provided(&None, None, &None));
        assert_eq!(
            effective_reminder_value(None, reminder_details_provided(&None, None, &None)),
            None
        );
        assert!(reminder_details_provided(
            &Some("09:00".to_string()),
            None,
            &None
        ));
    }

    fn sample_profile(lang: Option<&str>) -> Profile {
        Profile {
            id: Some(7),
            display_name: Some("Ada".to_string()),
            email: Some("ada@example.test".to_string()),
            photo_url: None,
            lang: lang.map(str::to_string),
            is_anonymous: Some(false),
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
            legal: None,
            terms_accepted: Some(true),
        }
    }

    fn command_samples() -> Vec<(Commands, &'static str)> {
        vec![
            (Commands::Login { url: None }, "login"),
            (Commands::Logout, "logout"),
            (Commands::Status, "status"),
            (
                Commands::Lists {
                    action: ListCmd::List,
                },
                "lists",
            ),
            (
                Commands::Items {
                    action: Box::new(ItemCmd::Done { id: 1 }),
                },
                "items",
            ),
            (
                Commands::Folders {
                    action: FolderCmd::List,
                },
                "folders",
            ),
            (
                Commands::Members {
                    action: MemberCmd::List { list_id: 1 },
                },
                "members",
            ),
            (
                Commands::Keys {
                    action: KeyCmd::List,
                },
                "keys",
            ),
            (
                Commands::Search {
                    query: "milk".to_string(),
                },
                "search",
            ),
            (
                Commands::Activity {
                    list_id: 1,
                    limit: 5,
                },
                "activity",
            ),
            (Commands::Undo { list_id: 1 }, "undo"),
            (Commands::Redo { list_id: 1 }, "redo"),
            (Commands::Profile, "profile"),
            (
                Commands::Security {
                    action: SecurityCmd::Status,
                },
                "security",
            ),
            (Commands::AcceptTerms { docs: None }, "accept_terms"),
            (
                Commands::Handoff {
                    action: HandoffCmd::Clear,
                },
                "handoff",
            ),
            (Commands::Ping, "ping"),
            (Commands::Config, "config"),
            (Commands::UpdateCheck, "update_check"),
            (
                Commands::Privacy {
                    action: PrivacyCmd::Reset,
                },
                "privacy",
            ),
            (Commands::Mcp, "mcp"),
            (
                Commands::Batch {
                    file: "-".to_string(),
                    keep_going: false,
                },
                "batch",
            ),
            (Commands::Completions { shell: Shell::Bash }, "completions"),
        ]
    }

    #[test]
    fn command_classifiers_cover_all_top_level_variants() {
        for (command, label) in command_samples() {
            assert_eq!(command_trace_name(&command), label);
        }

        assert!(!command_supports_profile_locale(Some(&Commands::Login {
            url: None
        })));
        assert!(!command_supports_profile_locale(Some(
            &Commands::Completions { shell: Shell::Bash }
        )));
        assert!(!command_supports_profile_locale(Some(
            &Commands::UpdateCheck
        )));
        assert!(!command_supports_profile_locale(Some(&Commands::Privacy {
            action: PrivacyCmd::Reset,
        })));
        assert!(command_supports_profile_locale(None));
        assert!(command_supports_profile_locale(Some(&Commands::Status)));

        assert!(!command_supports_auto_update_check(&Commands::Login {
            url: None
        }));
        assert!(!command_supports_auto_update_check(
            &Commands::Completions { shell: Shell::Bash }
        ));
        assert!(!command_supports_auto_update_check(&Commands::UpdateCheck));
        assert!(!command_supports_auto_update_check(&Commands::Privacy {
            action: PrivacyCmd::Reset,
        }));
        assert!(command_supports_auto_update_check(&Commands::Status));
    }

    #[test]
    fn profile_locale_helpers_cover_env_profile_and_resolved_sources() {
        crate::i18n::set_locale("en");
        let profile = sample_profile(Some(" en_US.UTF-8 "));
        assert_eq!(profile_lang(&profile).as_deref(), Some("en_US.UTF-8"));
        assert_eq!(effective_lang_source(&profile), "profile");

        let resolved_profile = sample_profile(Some("fr-FR"));
        assert_eq!(effective_lang_source(&resolved_profile), "resolved");

        std::env::set_var(TEST_KRAMLI_LANG_ENV, "de");
        assert_eq!(effective_lang_source(&resolved_profile), "env");
        apply_profile_locale_now(&sample_profile(Some("fr")));
        std::env::remove_var(TEST_KRAMLI_LANG_ENV);

        let empty_profile = sample_profile(Some("   "));
        assert_eq!(profile_lang(&empty_profile), None);

        crate::i18n::set_locale("pt-BR");
        apply_profile_locale_now(&sample_profile(Some("fr-FR")));
        assert_eq!(current_locale_code(), "fr-FR");

        let json = profile_json_with_lang(&sample_profile(Some("fr-FR")));
        assert_eq!(
            json.get("profile_lang").and_then(Value::as_str),
            Some("fr-FR")
        );
        assert_eq!(json.get("lang").and_then(Value::as_str), Some("fr-FR"));
        assert_eq!(
            json.get("lang_source").and_then(Value::as_str),
            Some("profile")
        );
    }

    fn list_response(id: i64, name: &str) -> String {
        json!({"id": id, "name": name}).to_string()
    }

    async fn api_with_responses(
        responses: Vec<String>,
    ) -> (ApiClient, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("test server should have addr");
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for body in responses {
                let (mut stream, _) = listener.accept().await.expect("request should connect");
                let mut buffer = [0_u8; 4096];
                let read = stream.read(&mut buffer).await.expect("request should read");
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                requests.push(request.lines().next().unwrap_or_default().to_string());
                let header = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                stream
                    .write_all(header.as_bytes())
                    .await
                    .expect("response header should write");
                stream
                    .write_all(body.as_bytes())
                    .await
                    .expect("response body should write");
            }
            requests
        });

        (ApiClient::for_tests(&format!("http://{addr}")), handle)
    }

    async fn server_with_base_url(
        responses: Vec<String>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let (api, handle) = api_with_responses(responses).await;
        (api.base_url_for_tests().to_string(), handle)
    }

    async fn with_env_vars_async<T, Fut>(vars: &[(&str, &str)], f: impl FnOnce() -> Fut) -> T
    where
        Fut: std::future::Future<Output = T>,
    {
        let previous = vars
            .iter()
            .map(|(key, _)| ((*key).to_string(), std::env::var(key).ok()))
            .collect::<Vec<_>>();
        for (key, value) in vars {
            std::env::set_var(key, value);
        }

        let result = f().await;

        for (key, value) in previous {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }

        result
    }

    fn temp_batch_file(name: &str, content: &str) -> String {
        let path = std::env::temp_dir().join(format!(
            "kramli-cli-{name}-{}-{}.txt",
            std::process::id(),
            unix_timestamp_secs()
        ));
        std::fs::write(&path, content).expect("batch file should write");
        path.to_string_lossy().into_owned()
    }

    const TEST_KRAMLI_API_KEY_ENV: &str = "KRAMLI_API_KEY";

    #[test]
    fn list_update_and_batch_helpers_cover_branch_variants() {
        let body = update_list_body(
            Some("Groceries".to_string()),
            Some("cart".to_string()),
            Some("#ffffff".to_string()),
            Some("Open:#336699,Done".to_string()),
        )
        .expect("list body should build");
        assert_eq!(
            body.get("name"),
            Some(&Value::String("Groceries".to_string()))
        );
        assert_eq!(body.get("icon"), Some(&Value::String("cart".to_string())));
        assert_eq!(
            body.get("color"),
            Some(&Value::String("#ffffff".to_string()))
        );
        assert!(body.get("states").is_some_and(Value::is_array));
        assert!(update_list_body(None, None, None, None).is_err());

        let mut failed = 0;
        let mut first_error = None;
        assert!(!record_batch_failure(
            &mut failed,
            &mut first_error,
            "line 1: bad".to_string(),
            false,
        ));
        assert_eq!(failed, 1);
        assert_eq!(first_error.as_deref(), Some("line 1: bad"));
        assert!(record_batch_failure(
            &mut failed,
            &mut first_error,
            "line 2: worse".to_string(),
            true,
        ));
        assert_eq!(failed, 2);
        assert_eq!(first_error.as_deref(), Some("line 1: bad"));
    }

    #[tokio::test]
    async fn list_mutation_helpers_cover_json_and_human_paths() {
        let responses = vec![
            list_response(7, "Created"),
            list_response(8, "Created JSON"),
            list_response(7, "Updated"),
            list_response(7, "Updated JSON"),
            json!({"ok": true, "undo_token": "undo-1"}).to_string(),
            json!({"ok": true}).to_string(),
            list_response(7, "Moved"),
            list_response(7, "Moved Home"),
            list_response(7, "Moved JSON"),
        ];
        let (api, requests) = api_with_responses(responses).await;

        run_lists_create(
            &api,
            false,
            "Created".to_string(),
            Some("cart".to_string()),
            Some("#ffffff".to_string()),
            Some(3),
            Some("Open:#336699,Done".to_string()),
        )
        .await
        .expect("create human should succeed");
        run_lists_create(
            &api,
            true,
            "Created JSON".to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("create json should succeed");
        run_lists_update(
            &api,
            false,
            7,
            Some("Updated".to_string()),
            None,
            None,
            None,
        )
        .await
        .expect("update human should succeed");
        run_lists_update(&api, true, 7, None, Some("list".to_string()), None, None)
            .await
            .expect("update json should succeed");
        run_lists_delete(&api, false, 7)
            .await
            .expect("delete human should succeed");
        run_lists_delete(&api, true, 7)
            .await
            .expect("delete json should succeed");
        run_lists_move(&api, false, 7, Some(3))
            .await
            .expect("move to folder should succeed");
        run_lists_move(&api, false, 7, None)
            .await
            .expect("move out of folder should succeed");
        run_lists_move(&api, true, 7, Some(4))
            .await
            .expect("move json should succeed");

        let requests = requests.await.expect("test server should finish");
        assert_eq!(requests[0], "POST /api/lists HTTP/1.1");
        assert_eq!(requests[1], "POST /api/lists HTTP/1.1");
        assert_eq!(requests[2], "PUT /api/lists/7 HTTP/1.1");
        assert_eq!(requests[3], "PUT /api/lists/7 HTTP/1.1");
        assert_eq!(requests[4], "DELETE /api/lists/7 HTTP/1.1");
        assert_eq!(requests[5], "DELETE /api/lists/7 HTTP/1.1");
        assert_eq!(requests[6], "PUT /api/lists/7 HTTP/1.1");
        assert_eq!(requests[7], "PUT /api/lists/7 HTTP/1.1");
        assert_eq!(requests[8], "PUT /api/lists/7 HTTP/1.1");
    }

    #[tokio::test]
    async fn env_var_helper_restores_existing_values() {
        std::env::set_var(TEST_KRAMLI_API_KEY_ENV, "before");
        with_env_vars_async(&[(TEST_KRAMLI_API_KEY_ENV, "during")], || async {
            assert_eq!(
                std::env::var(TEST_KRAMLI_API_KEY_ENV).as_deref(),
                Ok("during")
            );
        })
        .await;
        assert_eq!(
            std::env::var(TEST_KRAMLI_API_KEY_ENV).as_deref(),
            Ok("before")
        );
        std::env::remove_var(TEST_KRAMLI_API_KEY_ENV);
    }

    #[tokio::test]
    async fn run_lists_dispatches_mutation_subcommands() {
        let responses = vec![
            list_response(7, "Created"),
            list_response(7, "Updated"),
            json!({"ok": true}).to_string(),
            list_response(7, "Moved"),
        ];
        let (base_url, requests) = server_with_base_url(responses).await;

        with_env_vars_async(
            &[("KRAMLI_URL", &base_url), ("KRAMLI_API_KEY", "test")],
            || async {
                run_lists(
                    ListCmd::Create {
                        name: "Created".to_string(),
                        icon: None,
                        color: None,
                        folder: None,
                        states: None,
                    },
                    true,
                )
                .await
                .expect("create should dispatch");
                run_lists(
                    ListCmd::Update {
                        id: 7,
                        name: Some("Updated".to_string()),
                        icon: None,
                        color: None,
                        states: None,
                    },
                    true,
                )
                .await
                .expect("update should dispatch");
                run_lists(ListCmd::Delete { id: 7 }, true)
                    .await
                    .expect("delete should dispatch");
                run_lists(
                    ListCmd::Move {
                        id: 7,
                        folder_id: Some(3),
                    },
                    true,
                )
                .await
                .expect("move should dispatch");
            },
        )
        .await;

        let requests = requests.await.expect("test server should finish");
        assert_eq!(requests[0], "POST /api/lists HTTP/1.1");
        assert_eq!(requests[1], "PUT /api/lists/7 HTTP/1.1");
        assert_eq!(requests[2], "DELETE /api/lists/7 HTTP/1.1");
        assert_eq!(requests[3], "PUT /api/lists/7 HTTP/1.1");
    }

    #[tokio::test]
    async fn run_batch_reports_parse_and_nested_command_errors() {
        let shell_error = temp_batch_file("shell-error", "\"unterminated\n");
        assert!(run_batch(&shell_error, false, false).await.is_err());

        let cli_error = temp_batch_file("cli-error", "not-a-command\n");
        assert!(run_batch(&cli_error, false, false).await.is_err());

        let nested_error = temp_batch_file("nested-error", "batch -\n");
        assert!(run_batch(&nested_error, false, false).await.is_err());
    }
}

#[derive(Subcommand)]
/// Item management subcommands.
pub(crate) enum ItemCmd {
    /// List all items in a list
    #[command(alias = "ls")]
    List {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
        /// Show only open (not done) items
        #[arg(long, conflicts_with = "completed")]
        open: bool,
        /// Show only completed items
        #[arg(long, conflicts_with = "open")]
        completed: bool,
        /// Filter by custom state
        #[arg(long)]
        state: Option<String>,
        /// Filter by title text (case-insensitive)
        #[arg(long)]
        contains: Option<String>,
        /// Sort by creation date (newest first)
        #[arg(long, conflicts_with = "oldest")]
        newest: bool,
        /// Sort by creation date (oldest first)
        #[arg(long, conflicts_with = "newest")]
        oldest: bool,
        /// Limit number of returned items after filtering/sorting
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show item details (notes, images, comments)
    Show { id: i64 },
    /// Add a new item
    Add {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
        text: String,
        #[arg(short, long)]
        quantity: Option<String>,
        #[arg(short, long)]
        due: Option<String>,
        #[arg(long)]
        due_time: Option<String>,
        #[arg(long, alias = "planned-date")]
        planned: Option<String>,
        #[arg(long)]
        planned_time: Option<String>,
        #[arg(long)]
        reminder: Option<bool>,
        #[arg(long)]
        reminder_time: Option<String>,
        #[arg(long)]
        reminder_days_before: Option<i64>,
        #[arg(long, value_delimiter = ',')]
        reminder_offsets: Option<Vec<i64>>,
        /// Travel time in minutes (independent from reminders)
        #[arg(long, alias = "travel-time", alias = "wegzeit")]
        travel_time_minutes: Option<i64>,
        #[arg(short, long)]
        priority: Option<String>,
        #[arg(short, long)]
        tags: Option<String>,
        #[arg(short, long)]
        notes: Option<String>,
        #[arg(long)]
        parent: Option<i64>,
        /// Assign user IDs (comma-separated)
        #[arg(short, long)]
        assign: Option<String>,
        /// Item color (hex, e.g. #ff4d4f)
        #[arg(long)]
        color: Option<String>,
        /// Item state (e.g. "In Progress", "Review")
        #[arg(long, alias = "state")]
        progress: Option<String>,
    },
    /// Update an item
    Update {
        id: i64,
        #[arg(short, long)]
        text: Option<String>,
        #[arg(short, long)]
        quantity: Option<String>,
        #[arg(short, long)]
        due: Option<String>,
        #[arg(long)]
        due_time: Option<String>,
        #[arg(long, alias = "planned-date")]
        planned: Option<String>,
        #[arg(long)]
        planned_time: Option<String>,
        #[arg(long)]
        reminder: Option<bool>,
        #[arg(long)]
        reminder_time: Option<String>,
        #[arg(long)]
        reminder_days_before: Option<i64>,
        #[arg(long, value_delimiter = ',')]
        reminder_offsets: Option<Vec<i64>>,
        /// Travel time in minutes (independent from reminders)
        #[arg(long, alias = "travel-time", alias = "wegzeit")]
        travel_time_minutes: Option<i64>,
        #[arg(short, long)]
        priority: Option<String>,
        #[arg(long)]
        tags: Option<String>,
        #[arg(short, long)]
        notes: Option<String>,
        /// Assign user IDs (comma-separated)
        #[arg(short, long)]
        assign: Option<String>,
        /// Item color (hex)
        #[arg(long)]
        color: Option<String>,
        /// Item state (e.g. "In Progress", "Review")
        #[arg(long, alias = "state")]
        progress: Option<String>,
    },
    /// Toggle an item between done and not done
    #[command(alias = "check")]
    Done { id: i64 },
    /// Add or remove your vote on an item
    #[command(alias = "upvote")]
    Vote { id: i64 },
    /// Delete an item
    #[command(alias = "rm")]
    Delete { id: i64 },
    /// Show only completed items
    #[command(name = "done-list")]
    DoneList {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
    },
    /// Add a comment to an item
    Comment { id: i64, text: String },
    /// Mark all items as done
    #[command(name = "check-all")]
    CheckAll {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
    },
    /// Delete all completed items
    #[command(name = "clear-done")]
    ClearDone {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
    },
}

#[derive(Subcommand)]
/// Folder management subcommands.
pub(crate) enum FolderCmd {
    #[command(alias = "ls")]
    List,
    Create {
        name: String,
        #[arg(short, long)]
        icon: Option<String>,
        #[arg(short, long)]
        color: Option<String>,
        /// Parent folder ID for nested folders
        #[arg(short, long)]
        parent: Option<i64>,
    },
    Update {
        id: i64,
        #[arg(short, long)]
        name: Option<String>,
        #[arg(short, long)]
        icon: Option<String>,
        #[arg(short, long)]
        color: Option<String>,
        /// Parent folder ID (omit to leave unchanged)
        #[arg(short, long)]
        parent: Option<i64>,
    },
    #[command(alias = "rm")]
    Delete { id: i64 },
}

#[derive(Subcommand)]
/// Sharing and membership subcommands.
pub(crate) enum MemberCmd {
    #[command(alias = "ls")]
    List {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
    },
    Invite {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
        email: String,
        #[arg(short, long, default_value = "editor")]
        role: String,
    },
    Remove {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
        user_id: i64,
    },
    Role {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
        user_id: i64,
        role: String,
    },
    /// Create a reusable invite link
    #[command(name = "invite-link")]
    InviteLink {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
    },
    /// Revoke public share link
    Unshare {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
    },
    Leave {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
    },
}

#[derive(Subcommand)]
/// API key management subcommands.
pub(crate) enum KeyCmd {
    /// List API keys
    #[command(alias = "ls")]
    List,
    /// Create a new API key
    Create {
        /// Human-readable key name
        name: String,
        /// Scopes (comma-separated). Available: lists:read, lists:write,
        /// folders:read, folders:write, sharing, profile:read, profile:write,
        /// search, all
        #[arg(short, long, default_value = "all")]
        scopes: String,
    },
    /// Revoke an API key
    Revoke { key_id: i64 },
}

#[derive(Subcommand)]
/// Account security subcommands.
pub(crate) enum SecurityCmd {
    /// Security level, factors, and login alert emails
    Status,
    /// Confirm an unusual login (token from email/security notice)
    Ack {
        /// Signed ack token (or set KRAMLI_ACK_TOKEN)
        token: Option<String>,
    },
}

#[derive(Subcommand)]
/// Local privacy preference subcommands.
pub(crate) enum PrivacyCmd {
    /// Reset telemetry and Bootstrap icon preferences so Kramli asks again
    Reset,
}

#[derive(Subcommand)]
/// Cross-device handoff subcommands.
pub(crate) enum HandoffCmd {
    /// Mark a list as currently viewed on this device
    Viewing {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
        #[arg(long)]
        list_name: Option<String>,
        #[arg(long)]
        device: Option<String>,
    },
    /// Ask another device to continue with this list
    Continue {
        #[arg(value_parser = resolve_list_reference)]
        list_id: i64,
        #[arg(long)]
        list_name: Option<String>,
        #[arg(long)]
        device: Option<String>,
    },
    /// Clear current handoff state
    Clear,
}

// ─── Dispatch ───

/// Execute a parsed CLI invocation.
pub(crate) async fn run(cli: Cli) -> Result<(), String> {
    let command_label = if cli.interactive {
        "interactive"
    } else {
        cli.command.as_ref().map_or("missing", command_trace_name)
    };
    let mode_label = if cli.interactive {
        "interactive"
    } else if cli.json {
        "json"
    } else {
        "human"
    };
    let transaction = telemetry::TraceTransaction::start("cli.command", "cli.command");
    transaction.set_tag("command", command_label);
    transaction.set_tag("mode", mode_label);
    transaction.set_data_i64(
        "cli.has_command",
        if cli.command.is_some() || cli.interactive {
            1
        } else {
            0
        },
    );

    let result = run_inner(cli).await;
    transaction.set_tag("outcome", if result.is_ok() { "ok" } else { "error" });
    if result.is_err() {
        transaction.set_tag("error.category", "command_error");
    }
    transaction.finish(result.is_ok() || !telemetry::should_capture_command_error(""));
    result
}

async fn run_inner(cli: Cli) -> Result<(), String> {
    // Honour NO_COLOR convention (https://no-color.org/)
    if std::env::var(NO_COLOR_ENV).is_ok() || cli.json {
        set_override(false);
    }

    if cli.interactive {
        if cli.json {
            return Err(tr("cli-interactive-json-conflict"));
        }
        if cli.command.is_some() {
            return Err(tr("cli-interactive-subcommand-conflict"));
        }
        return crate::tui::run_tui().await;
    }

    let should_auto_update_check = cli
        .command
        .as_ref()
        .is_some_and(command_supports_auto_update_check)
        && !cli.json;

    let Some(command) = cli.command else {
        let mut cmd = Cli::command();
        cmd.print_help().map_err(|error| error.to_string())?;
        println!();
        return Ok(());
    };

    maybe_apply_profile_locale(Some(&command)).await;

    let result = run_command(command, cli.json).await;
    if result.is_ok() && should_auto_update_check {
        maybe_auto_update_notice().await;
    }
    result
}

fn command_trace_name(command: &Commands) -> &'static str {
    match command {
        Commands::Login { .. } => "login",
        Commands::Logout => "logout",
        Commands::Status => "status",
        Commands::Lists { .. } => "lists",
        Commands::Items { .. } => "items",
        Commands::Folders { .. } => "folders",
        Commands::Members { .. } => "members",
        Commands::Keys { .. } => "keys",
        Commands::Search { .. } => "search",
        Commands::Activity { .. } => "activity",
        Commands::Undo { .. } => "undo",
        Commands::Redo { .. } => "redo",
        Commands::Profile => "profile",
        Commands::Security { .. } => "security",
        Commands::AcceptTerms { .. } => "accept_terms",
        Commands::Handoff { .. } => "handoff",
        Commands::Ping => "ping",
        Commands::Config => "config",
        Commands::UpdateCheck => "update_check",
        Commands::Privacy { .. } => "privacy",
        Commands::Mcp => "mcp",
        Commands::Batch { .. } => "batch",
        Commands::Completions { .. } => "completions",
    }
}

async fn run_command(command: Commands, as_json: bool) -> Result<(), String> {
    match command {
        Commands::Login { url } => run_login(url).await,
        Commands::Logout => run_logout(),
        Commands::Status => run_status(as_json).await,
        Commands::Lists { action } => run_lists(action, as_json).await,
        Commands::Items { action } => run_items(*action, as_json).await,
        Commands::Folders { action } => run_folders(action, as_json).await,
        Commands::Members { action } => run_members(action, as_json).await,
        Commands::Keys { action } => run_keys(action, as_json).await,
        Commands::Search { query } => run_search(&query, as_json).await,
        Commands::Activity { list_id, limit } => run_activity(list_id, limit, as_json).await,
        Commands::Undo { list_id } => run_undo(list_id).await,
        Commands::Redo { list_id } => run_redo(list_id).await,
        Commands::Profile => run_profile(as_json).await,
        Commands::Security { action } => run_security(action, as_json).await,
        Commands::AcceptTerms { docs } => run_accept_terms(docs, as_json).await,
        Commands::Handoff { action } => run_handoff(action, as_json).await,
        Commands::Ping => run_ping(as_json).await,
        Commands::Config => run_config(as_json),
        Commands::UpdateCheck => run_update_check(as_json).await,
        Commands::Privacy { action } => run_privacy(action, as_json),
        Commands::Mcp => crate::mcp::run_stdio().await,
        Commands::Batch { file, keep_going } => run_batch(&file, keep_going, as_json).await,
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "kramli", &mut std::io::stdout());
            Ok(())
        }
    }
}

fn command_supports_profile_locale(command: Option<&Commands>) -> bool {
    !matches!(
        command,
        Some(Commands::Login { .. })
            | Some(Commands::Completions { .. })
            | Some(Commands::UpdateCheck)
            | Some(Commands::Privacy { .. })
    )
}

fn command_supports_auto_update_check(command: &Commands) -> bool {
    !matches!(
        command,
        Commands::Login { .. }
            | Commands::Completions { .. }
            | Commands::UpdateCheck
            | Commands::Privacy { .. }
    )
}

async fn maybe_apply_profile_locale(command: Option<&Commands>) {
    if is_explicit_lang_set() || !command_supports_profile_locale(command) {
        return;
    }

    let cfg = Config::load();
    if !cfg.has_api_key() {
        return;
    }

    let Ok(api) = ApiClient::new(&cfg) else {
        return;
    };

    let Ok(profile) = api.get::<Profile>("/profile").await else {
        return;
    };

    apply_profile_locale(profile.lang.as_deref());
}

fn profile_lang(profile: &Profile) -> Option<String> {
    profile
        .lang
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn effective_lang_source(profile: &Profile) -> &'static str {
    if is_explicit_lang_set() {
        return "env";
    }

    fn primary_lang(raw: &str) -> String {
        raw.trim()
            .split(',')
            .next()
            .unwrap_or("")
            .split('.')
            .next()
            .unwrap_or("")
            .split('@')
            .next()
            .unwrap_or("")
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
    }

    if let Some(profile_lang) = profile_lang(profile) {
        let profile_primary = primary_lang(&profile_lang);
        let current_primary = primary_lang(&current_locale_code());
        if !profile_primary.is_empty() && profile_primary == current_primary {
            return "profile";
        }
    }

    "resolved"
}

fn apply_profile_locale_now(profile: &Profile) {
    if !is_explicit_lang_set() {
        apply_profile_locale(profile.lang.as_deref());
    }
}

fn profile_json_with_lang(profile: &Profile) -> Value {
    let mut out = serde_json::to_value(profile).unwrap_or_else(|_| empty_json_object());
    if let Some(obj) = out.as_object_mut() {
        let source = effective_lang_source(profile);
        let profile_lang = profile_lang(profile);
        obj.insert("lang".to_string(), Value::String(current_locale_code()));
        if let Some(lang) = profile_lang {
            obj.insert("profile_lang".to_string(), Value::String(lang));
        }
        obj.insert("lang_source".to_string(), Value::String(source.to_string()));
    }
    out
}

// ─── Login ───

async fn run_login(url: Option<String>) -> Result<(), String> {
    let mut cfg = Config::load();
    if let Some(ref u) = url {
        cfg.set_base_url(Some(u.clone()));
        cfg.save()?;
    }

    println!(
        "{}",
        tr_args(
            "cli-login-generate-api-key",
            &[(
                "url",
                format!("{}/settings#api-keys", cfg.base_url().trim_end_matches('/')),
            )],
        )
    );

    let key: String = dialoguer::Password::new()
        .with_prompt(tr("cli-api-key-label"))
        .interact()
        .map_err(|e| tr_args("cli-input-error", &[("error", e.to_string())]))?;

    let key = key.trim().to_string();
    if !key.starts_with("kramli_") {
        return Err(tr("cli-api-key-invalid-format"));
    }

    cfg.set_api_key(&key)?;
    cfg.save()?;

    let api = ApiClient::new(&cfg)?;
    match api.get::<Profile>("/profile").await {
        Ok(p) => {
            apply_profile_locale_now(&p);
            let name = p
                .display_name
                .clone()
                .or_else(|| p.email.clone())
                .unwrap_or_else(|| tr("common-unknown"));
            println!("{} {} {}", "✓".green(), tr("cli-logged-in-as"), name.bold());
            println!("  {}", tr("cli-api-key-stored"));
        }
        Err(e) => {
            cfg.delete_api_key()?;
            return Err(tr_args("cli-api-key-invalid", &[("error", e)]));
        }
    }
    Ok(())
}

fn run_logout() -> Result<(), String> {
    let cfg = Config::load();
    cfg.delete_api_key()?;
    println!("{} {}", "✓".green(), tr("cli-logged-out"));
    Ok(())
}

async fn run_status(as_json: bool) -> Result<(), String> {
    let cfg = Config::load();
    if as_json {
        let mut out = json!({
            "server": cfg.base_url(),
            "logged_in": cfg.has_api_key(),
            "key_source": if cfg.api_key_from_env() { "env" } else { "keychain" },
        });
        if cfg.has_api_key() {
            let api = ApiClient::new(&cfg)?;
            if let Ok(p) = api.get::<Profile>("/profile").await {
                out["profile"] = profile_json_with_lang(&p);
            }
        }
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return Ok(());
    }
    if cfg.has_api_key() {
        println!("{}        {}", tr("label-server"), cfg.base_url());
        let src = if cfg.api_key_from_env() {
            tr("cli-key-source-env")
        } else {
            tr("cli-key-source-keychain")
        };
        println!(
            "{} {} ({})",
            tr("label-api-key"),
            tr("label-stored").green(),
            src
        );
        let api = ApiClient::new(&cfg)?;
        match api.get::<Profile>("/profile").await {
            Ok(p) => {
                let display_name = p
                    .display_name
                    .clone()
                    .unwrap_or_else(|| tr("common-unknown"));
                println!("{}          {}", tr("label-name"), display_name);
                println!(
                    "{}        {}",
                    tr("label-email"),
                    p.email.as_deref().unwrap_or("-")
                );
            }
            Err(e) => println!("{} {e}", tr("cli-profile-unavailable").yellow()),
        }
    } else {
        println!(
            "{}        {}",
            tr("label-status"),
            tr("cli-not-logged-in").red()
        );
        println!("  {}", tr("cli-login-hint"));
    }
    Ok(())
}

fn get_api() -> Result<ApiClient, String> {
    let cfg = Config::load();
    ApiClient::new(&cfg)
}

fn parse_states_arg(raw: &str) -> Result<Value, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Value::Array(Vec::new()));
    }

    if trimmed.starts_with('[') {
        let parsed: Value = serde_json::from_str(trimmed)
            .map_err(|e| tr_args("cli-states-invalid-json", &[("error", e.to_string())]))?;
        if !parsed.is_array() {
            return Err(tr("cli-states-json-must-array"));
        }
        return Ok(parsed);
    }

    let mut states = Vec::new();
    for chunk in trimmed.split(',') {
        let part = chunk.trim();
        if part.is_empty() {
            continue;
        }
        let (name, color) = part.split_once(':').unwrap_or((part, ""));
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let mut state = serde_json::Map::new();
        state.insert("name".into(), Value::String(name.to_string()));
        let color = color.trim();
        if !color.is_empty() {
            state.insert("color".into(), Value::String(color.to_string()));
        }
        states.push(Value::Object(state));
    }

    if states.is_empty() {
        return Err(tr("cli-states-empty"));
    }

    Ok(Value::Array(states))
}

fn normalize_progress_value(progress: Option<String>) -> Option<Value> {
    progress.map(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            Value::Null
        } else {
            Value::String(trimmed.to_string())
        }
    })
}

fn reminder_details_provided(
    reminder_time: &Option<String>,
    reminder_days_before: Option<i64>,
    reminder_offsets: &Option<Vec<i64>>,
) -> bool {
    reminder_time
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || reminder_days_before.is_some()
        || reminder_offsets
            .as_ref()
            .is_some_and(|offsets| !offsets.is_empty())
}

fn effective_reminder_value(reminder: Option<bool>, has_reminder_details: bool) -> Option<bool> {
    reminder.or_else(|| has_reminder_details.then_some(true))
}

async fn fetch_item_from_list(
    api: &ApiClient,
    list_id: i64,
    item_id: i64,
) -> Result<ListItem, String> {
    let items: Vec<ListItem> = api.get(&format!("/lists/{list_id}/items")).await?;
    items
        .into_iter()
        .find(|item| item.id == item_id)
        .ok_or_else(|| {
            tr_args(
                "cli-item-not-found-in-list",
                &[
                    ("item_id", item_id.to_string()),
                    ("list_id", list_id.to_string()),
                ],
            )
        })
}

async fn find_item_across_lists(
    api: &ApiClient,
    item_id: i64,
) -> Result<Option<(ListItem, ShoppingList)>, String> {
    let lists: Vec<ShoppingList> = api.get("/lists").await?;
    for list in lists {
        let items: Vec<ListItem> = api.get(&format!("/lists/{}/items", list.id)).await?;
        if let Some(item) = items.into_iter().find(|candidate| candidate.id == item_id) {
            return Ok(Some((item, list)));
        }
    }
    Ok(None)
}

fn parse_search_item_id(query: &str) -> Option<i64> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if candidate.is_empty() || !candidate.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    candidate.parse::<i64>().ok().filter(|id| *id > 0)
}

async fn enrich_item_tags_from_list(api: &ApiClient, item: &mut ListItem) {
    let Some(list_id) = item.list_id else {
        return;
    };
    if let Ok(full_item) = fetch_item_from_list(api, list_id, item.id).await {
        item.tags = full_item.tags;
    }
}

async fn enrich_done_response_tags(api: &ApiClient, item_id: i64, response: &mut Value) {
    let Some(list_id) = response.get("list_id").and_then(Value::as_i64) else {
        return;
    };
    let Some(response_object) = response.as_object_mut() else {
        return;
    };
    if let Ok(full_item) = fetch_item_from_list(api, list_id, item_id).await {
        let tags = full_item
            .tags
            .unwrap_or_default()
            .into_iter()
            .map(Value::String)
            .collect::<Vec<Value>>();
        response_object.insert("tags".into(), Value::Array(tags));
    }
}

fn env_flag_enabled(name: &str, default_value: bool) -> bool {
    let Ok(raw) = std::env::var(name) else {
        return default_value;
    };
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return default_value;
    }
    match normalized.as_str() {
        "0" | "false" | "off" | "no" => false,
        "1" | "true" | "on" | "yes" => true,
        _ => default_value,
    }
}

fn auto_update_check_enabled() -> bool {
    if env_flag_enabled("DO_NOT_TRACK", false) || env_flag_enabled("KRAMLI_NO_TELEMETRY", false) {
        return false;
    }
    env_flag_enabled("KRAMLI_AUTO_UPDATE_CHECK", false)
}

fn auto_handoff_enabled() -> bool {
    env_flag_enabled("KRAMLI_AUTO_HANDOFF", true)
}

fn handoff_body(
    list_id: i64,
    list_name: Option<String>,
    device_label: String,
) -> serde_json::Map<String, Value> {
    let mut body = serde_json::Map::new();
    body.insert("list_id".into(), Value::from(list_id));
    body.insert("device_label".into(), Value::String(device_label));
    if let Some(name) = normalize_optional_text(list_name) {
        body.insert("list_name".into(), Value::String(name));
    }
    body
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

fn parse_semver_triplet(raw: &str) -> Option<SemverTriplet> {
    fn parse_segment(segment: &str) -> Option<u64> {
        let digits: String = segment
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            None
        } else {
            digits.parse::<u64>().ok()
        }
    }

    let normalized = raw.trim().trim_start_matches(['v', 'V']);
    let mut parts = normalized.split('.');
    let major = parse_segment(parts.next()?)?;
    let minor = parse_segment(parts.next()?)?;
    let patch = parse_segment(parts.next()?)?;
    Some(SemverTriplet {
        major,
        minor,
        patch,
    })
}

fn update_is_available(current: &str, latest: &str) -> Option<bool> {
    let current = parse_semver_triplet(current)?;
    let latest = parse_semver_triplet(latest)?;
    Some(latest > current)
}

async fn fetch_latest_release() -> Result<GitHubRelease, String> {
    let span = telemetry::TraceSpan::child("http.client", "update_check");
    span.set_tag("operation", "update_check");
    span.set_tag("api.method", "GET");
    span.set_tag("api.route", "external_release");
    let response = match reqwest::Client::new()
        .get(UPDATE_CHECK_URL)
        .header(
            reqwest::header::USER_AGENT,
            format!("kramli-cli/{}", env!("CARGO_PKG_VERSION")),
        )
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(Duration::from_secs(4))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            span.set_status(false);
            span.finish();
            return Err(tr_args(
                "api-network-error",
                &[("error", error.to_string())],
            ));
        }
    };

    span.set_tag(
        "api.status_class",
        telemetry::status_class(response.status().as_u16()),
    );
    if !response.status().is_success() {
        span.set_status(false);
        span.finish();
        return Err(tr_args(
            "cli-update-check-http",
            &[("status", response.status().as_u16().to_string())],
        ));
    }

    let text = match response.text().await {
        Ok(text) => text,
        Err(error) => {
            span.set_status(false);
            span.finish();
            return Err(tr_args(
                "api-network-error",
                &[("error", error.to_string())],
            ));
        }
    };
    span.set_data_i64("api.response_bytes", text.len() as i64);
    let result = serde_json::from_str::<GitHubRelease>(&text)
        .map_err(|e| tr_args("api-network-error", &[("error", e.to_string())]));
    span.set_status(result.is_ok());
    span.finish();
    result
}

fn invite_url_from_response(resp: &Value) -> Option<String> {
    resp.get("invite_url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            resp.get("invite_token")
                .and_then(Value::as_str)
                .map(|token| format!("https://kram.li/i/{token}"))
        })
        .or_else(|| {
            resp.get("token")
                .and_then(Value::as_str)
                .map(|token| format!("https://kram.li/i/{token}"))
        })
}

async fn maybe_auto_update_notice() {
    if !auto_update_check_enabled() {
        return;
    }

    let mut cfg = Config::load();
    let now = unix_timestamp_secs();
    if let Some(last) = cfg.update_check_last() {
        if now.saturating_sub(last) < UPDATE_CHECK_INTERVAL_SECS {
            return;
        }
    }

    let release = match fetch_latest_release().await {
        Ok(release) => release,
        Err(_) => {
            cfg.set_update_check_state(now, None, None);
            let _ = cfg.save();
            return;
        }
    };

    cfg.set_update_check_state(
        now,
        Some(release.tag_name.clone()),
        release.html_url.clone(),
    );
    let _ = cfg.save();

    if update_is_available(env!("CARGO_PKG_VERSION"), &release.tag_name) != Some(true) {
        return;
    }

    let url = release.html_url.unwrap_or_else(|| {
        "https://github.com/SpotlightForBugs/kramli-cli/releases/latest".to_string()
    });
    eprintln!(
        "{} {}",
        "↑".cyan(),
        tr_args(
            "cli-update-auto-available",
            &[
                ("current", env!("CARGO_PKG_VERSION").to_string()),
                ("latest", release.tag_name),
                ("url", url),
            ],
        )
    );
}

async fn maybe_auto_handoff(api: &ApiClient, list_id: i64, list_name: Option<&str>, as_json: bool) {
    if as_json || !auto_handoff_enabled() {
        return;
    }

    let body = handoff_body(
        list_id,
        list_name.map(str::to_string),
        default_handoff_device_label(None),
    );

    let _: Result<Value, String> = api.post("/activity/viewing", &body).await;
}

/// Print JSON if --json flag is set, otherwise call the human-friendly formatter.
macro_rules! json_or {
    ($as_json:expr, $data:expr, $fmt:expr) => {
        if $as_json {
            println!(
                "{}",
                serde_json::to_string_pretty(&$data).unwrap_or_default()
            );
        } else {
            $fmt;
        }
    };
}

// ─── Lists ───

async fn run_lists(cmd: ListCmd, as_json: bool) -> Result<(), String> {
    let api = get_api()?;
    match cmd {
        ListCmd::List => {
            let lists: Vec<ShoppingList> = api.get("/lists").await?;
            json_or!(as_json, lists, output::print_lists(&lists));
        }
        ListCmd::Resolve { reference } => {
            let id = resolve_list_reference(&reference)?;
            let payload = json!({
                "reference": reference,
                "list_id": id,
                "canonical_path": format!("/lists/{id}"),
            });
            json_or!(as_json, payload, {
                println!("{} {}", "✓".green(), tr("cli-list-reference-resolved"));
                println!("  {}: {id}", tr("label-list-id"));
                println!("  {}: /lists/{id}", tr("label-canonical"));
            });
        }
        ListCmd::Show { id } => {
            let list: ShoppingList = api.get(&format!("/lists/{id}")).await?;
            json_or!(as_json, list, output::print_list_detail(&list));
            maybe_auto_handoff(&api, id, Some(&list.name), as_json).await;
        }
        ListCmd::Create {
            name,
            icon,
            color,
            folder,
            states,
        } => run_lists_create(&api, as_json, name, icon, color, folder, states).await?,
        ListCmd::Update {
            id,
            name,
            icon,
            color,
            states,
        } => run_lists_update(&api, as_json, id, name, icon, color, states).await?,
        ListCmd::Delete { id } => run_lists_delete(&api, as_json, id).await?,
        ListCmd::Move { id, folder_id } => run_lists_move(&api, as_json, id, folder_id).await?,
    }
    Ok(())
}

async fn run_lists_create(
    api: &ApiClient,
    as_json: bool,
    name: String,
    icon: Option<String>,
    color: Option<String>,
    folder: Option<i64>,
    states: Option<String>,
) -> Result<(), String> {
    let body = CreateList {
        name,
        icon,
        color,
        folder_id: folder,
    };
    let mut payload = serde_json::to_value(&body).map_err(|e| e.to_string())?;
    if let Some(states_raw) = states {
        payload["states"] = parse_states_arg(&states_raw)?;
    }
    let list: ShoppingList = api.post("/lists", &payload).await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&list).unwrap_or_default()
        );
    } else {
        println!(
            "{} {}",
            "✓".green(),
            tr_args("cli-list-created", &[("id", list.id.to_string())])
        );
        output::print_list_detail(&list);
    }
    Ok(())
}

async fn run_lists_update(
    api: &ApiClient,
    as_json: bool,
    id: i64,
    name: Option<String>,
    icon: Option<String>,
    color: Option<String>,
    states: Option<String>,
) -> Result<(), String> {
    let body = update_list_body(name, icon, color, states)?;
    let list: ShoppingList = api.put(&format!("/lists/{id}"), &body).await?;
    json_or!(as_json, list, {
        println!("{} {}", "✓".green(), tr("cli-list-updated"));
        output::print_list_detail(&list);
    });
    Ok(())
}

fn update_list_body(
    name: Option<String>,
    icon: Option<String>,
    color: Option<String>,
    states: Option<String>,
) -> Result<serde_json::Map<String, Value>, String> {
    let mut body = serde_json::Map::new();
    insert_string(&mut body, "name", name);
    insert_string(&mut body, "icon", icon);
    insert_string(&mut body, "color", color);
    if let Some(states_raw) = states {
        body.insert("states".into(), parse_states_arg(&states_raw)?);
    }
    if body.is_empty() {
        return Err(tr("cli-no-changes"));
    }
    Ok(body)
}

async fn run_lists_delete(api: &ApiClient, as_json: bool, id: i64) -> Result<(), String> {
    let resp: OkResponse = api.delete(&format!("/lists/{id}")).await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&resp).unwrap_or_default()
        );
    } else {
        println!("{} {}", "✓".green(), tr("cli-list-deleted"));
        if let Some(t) = resp.undo_token {
            println!("  {}: {t}", tr("label-undo-token"));
        }
    }
    Ok(())
}

async fn run_lists_move(
    api: &ApiClient,
    as_json: bool,
    id: i64,
    folder_id: Option<i64>,
) -> Result<(), String> {
    let body = json!({"folder_id": folder_id});
    let list: ShoppingList = api.put(&format!("/lists/{id}"), &body).await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&list).unwrap_or_default()
        );
    } else {
        print_list_move_result(id, folder_id);
    }
    Ok(())
}

fn print_list_move_result(id: i64, folder_id: Option<i64>) {
    match folder_id {
        Some(fid) => println!(
            "{} {}",
            "✓".green(),
            tr_args(
                "cli-list-moved-folder",
                &[("id", id.to_string()), ("folder_id", fid.to_string())],
            )
        ),
        None => println!(
            "{} {}",
            "✓".green(),
            tr_args("cli-list-removed-folder", &[("id", id.to_string())])
        ),
    }
}

// ─── Items ───

async fn run_items(cmd: ItemCmd, as_json: bool) -> Result<(), String> {
    let api = get_api()?;
    match cmd {
        ItemCmd::List {
            list_id,
            open,
            completed,
            state,
            contains,
            newest,
            oldest,
            limit,
        } => {
            run_items_list(
                &api,
                as_json,
                ItemListArgs {
                    list_id,
                    open,
                    completed,
                    state,
                    contains,
                    newest,
                    oldest,
                    limit,
                },
            )
            .await?
        }
        ItemCmd::Show { id } => run_items_show(&api, as_json, id).await?,
        ItemCmd::Add {
            list_id,
            text,
            quantity,
            due,
            due_time,
            planned,
            planned_time,
            reminder,
            reminder_time,
            reminder_days_before,
            reminder_offsets,
            travel_time_minutes,
            priority,
            tags,
            notes,
            parent,
            assign,
            color,
            progress,
        } => {
            run_items_add(
                &api,
                as_json,
                ItemAddArgs {
                    list_id,
                    text,
                    quantity,
                    due,
                    due_time,
                    planned,
                    planned_time,
                    reminder,
                    reminder_time,
                    reminder_days_before,
                    reminder_offsets,
                    travel_time_minutes,
                    priority,
                    tags,
                    notes,
                    parent,
                    assign,
                    color,
                    progress,
                },
            )
            .await?
        }
        ItemCmd::Update {
            id,
            text,
            quantity,
            due,
            due_time,
            planned,
            planned_time,
            reminder,
            reminder_time,
            reminder_days_before,
            reminder_offsets,
            travel_time_minutes,
            priority,
            tags,
            notes,
            assign,
            color,
            progress,
        } => {
            run_items_update(
                &api,
                as_json,
                ItemUpdateArgs {
                    id,
                    text,
                    quantity,
                    due,
                    due_time,
                    planned,
                    planned_time,
                    reminder,
                    reminder_time,
                    reminder_days_before,
                    reminder_offsets,
                    travel_time_minutes,
                    priority,
                    tags,
                    notes,
                    assign,
                    color,
                    progress,
                },
            )
            .await?
        }
        ItemCmd::Done { id } => run_items_done(&api, as_json, id).await?,
        ItemCmd::Vote { id } => run_items_vote(&api, as_json, id).await?,
        ItemCmd::Delete { id } => run_items_delete(&api, as_json, id).await?,
        ItemCmd::DoneList { list_id } => run_items_done_list(&api, as_json, list_id).await?,
        ItemCmd::Comment { id, text } => run_items_comment(&api, as_json, id, text).await?,
        ItemCmd::CheckAll { list_id } => run_items_check_all(&api, as_json, list_id).await?,
        ItemCmd::ClearDone { list_id } => run_items_clear_done(&api, as_json, list_id).await?,
    }
    Ok(())
}

struct ItemListArgs {
    list_id: i64,
    open: bool,
    completed: bool,
    state: Option<String>,
    contains: Option<String>,
    newest: bool,
    oldest: bool,
    limit: Option<usize>,
}

async fn run_items_list(api: &ApiClient, as_json: bool, args: ItemListArgs) -> Result<(), String> {
    let ItemListArgs {
        list_id,
        open,
        completed,
        state,
        contains,
        newest,
        oldest,
        limit,
    } = args;
    let list = if as_json {
        None
    } else {
        api.get::<ShoppingList>(&format!("/lists/{list_id}"))
            .await
            .ok()
    };
    let items: Vec<ListItem> = api.get(&format!("/lists/{list_id}/items")).await?;
    let state_filter = state
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let contains_filter = contains
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let mut filtered: Vec<ListItem> = items
        .into_iter()
        .filter(|item| {
            item_matches_filters(
                item,
                open,
                completed,
                state_filter.as_deref(),
                contains_filter.as_deref(),
            )
        })
        .collect();

    if newest {
        filtered.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    } else if oldest {
        filtered.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    }

    if let Some(max) = limit {
        filtered.truncate(max);
    }

    json_or!(
        as_json,
        filtered,
        output::print_items_for_list(list.as_ref(), &filtered)
    );
    maybe_auto_handoff(
        api,
        list_id,
        list.as_ref().map(|l| l.name.as_str()),
        as_json,
    )
    .await;
    Ok(())
}

fn item_matches_filters(
    item: &ListItem,
    open: bool,
    completed: bool,
    state_filter: Option<&str>,
    contains_filter: Option<&str>,
) -> bool {
    let is_done = item.is_done.unwrap_or(false);
    if open && is_done {
        return false;
    }
    if completed && !is_done {
        return false;
    }

    if let Some(state_value) = state_filter {
        let item_state = item
            .progress
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if item_state != state_value {
            return false;
        }
    }

    if let Some(query) = contains_filter {
        let text = item.text.to_ascii_lowercase();
        if !text.contains(query) {
            return false;
        }
    }

    true
}

async fn run_items_show(api: &ApiClient, as_json: bool, id: i64) -> Result<(), String> {
    let comments: Vec<crate::models::ItemComment> =
        api.get(&format!("/items/{id}/comments")).await?;
    let (item, _) = find_item_across_lists(api, id)
        .await?
        .ok_or_else(|| tr_args("cli-item-not-found", &[("id", id.to_string())]))?;
    if as_json {
        let mut val = serde_json::to_value(&item).unwrap_or_default();
        val["comments"] = serde_json::to_value(&comments).unwrap_or_default();
        println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
    } else {
        output::print_item_detail(&item, &comments);
    }
    if let Some(list_id) = item.list_id {
        maybe_auto_handoff(api, list_id, None, as_json).await;
    }
    Ok(())
}

struct ItemAddArgs {
    list_id: i64,
    text: String,
    quantity: Option<String>,
    due: Option<String>,
    due_time: Option<String>,
    planned: Option<String>,
    planned_time: Option<String>,
    reminder: Option<bool>,
    reminder_time: Option<String>,
    reminder_days_before: Option<i64>,
    reminder_offsets: Option<Vec<i64>>,
    travel_time_minutes: Option<i64>,
    priority: Option<String>,
    tags: Option<String>,
    notes: Option<String>,
    parent: Option<i64>,
    assign: Option<String>,
    color: Option<String>,
    progress: Option<String>,
}

async fn run_items_add(api: &ApiClient, as_json: bool, args: ItemAddArgs) -> Result<(), String> {
    let ItemAddArgs {
        list_id,
        text,
        quantity,
        due,
        due_time,
        planned,
        planned_time,
        reminder,
        reminder_time,
        reminder_days_before,
        reminder_offsets,
        travel_time_minutes,
        priority,
        tags,
        notes,
        parent,
        assign,
        color,
        progress,
    } = args;
    let tag_vec = tags.map(|t| t.split(',').map(|s| s.trim().to_string()).collect());
    let reminder = effective_reminder_value(
        reminder,
        reminder_details_provided(&reminder_time, reminder_days_before, &reminder_offsets),
    );
    let body = CreateItem {
        text,
        quantity,
        notes,
        due_date: due,
        due_time,
        planned_date: planned,
        planned_time,
        reminder,
        reminder_time,
        reminder_days_before,
        reminder_offsets,
        travel_time_minutes,
        priority,
        tags: tag_vec,
        parent_item_id: parent,
    };
    let mut val = serde_json::to_value(&body).map_err(|e| e.to_string())?;
    if let Some(a) = assign {
        val["assigned_to"] = Value::Array(parse_id_values(&a));
    }
    if let Some(c) = color {
        val["color"] = Value::String(c);
    }
    if let Some(progress_value) = normalize_progress_value(progress) {
        val["progress"] = progress_value;
    }
    let item: ListItem = api.post(&format!("/lists/{list_id}/items"), &val).await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&item).unwrap_or_default()
        );
    } else {
        println!(
            "{} {}",
            "✓".green(),
            tr_args("cli-item-created", &[("id", item.id.to_string())])
        );
    }
    Ok(())
}

struct ItemUpdateArgs {
    id: i64,
    text: Option<String>,
    quantity: Option<String>,
    due: Option<String>,
    due_time: Option<String>,
    planned: Option<String>,
    planned_time: Option<String>,
    reminder: Option<bool>,
    reminder_time: Option<String>,
    reminder_days_before: Option<i64>,
    reminder_offsets: Option<Vec<i64>>,
    travel_time_minutes: Option<i64>,
    priority: Option<String>,
    tags: Option<String>,
    notes: Option<String>,
    assign: Option<String>,
    color: Option<String>,
    progress: Option<String>,
}

async fn run_items_update(
    api: &ApiClient,
    as_json: bool,
    args: ItemUpdateArgs,
) -> Result<(), String> {
    let ItemUpdateArgs {
        id,
        text,
        quantity,
        due,
        due_time,
        planned,
        planned_time,
        reminder,
        reminder_time,
        reminder_days_before,
        reminder_offsets,
        travel_time_minutes,
        priority,
        tags,
        notes,
        assign,
        color,
        progress,
    } = args;
    let tags_provided = tags.is_some();
    let mut body = serde_json::Map::new();
    let reminder = effective_reminder_value(
        reminder,
        reminder_details_provided(&reminder_time, reminder_days_before, &reminder_offsets),
    );
    insert_string(&mut body, "text", text);
    insert_string(&mut body, "quantity", quantity);
    insert_string(&mut body, "due_date", due);
    insert_string(&mut body, "due_time", due_time);
    insert_string(&mut body, "planned_date", planned);
    insert_string(&mut body, "planned_time", planned_time);
    if let Some(r) = reminder {
        body.insert("reminder".into(), Value::Bool(r));
    }
    insert_string(&mut body, "reminder_time", reminder_time);
    if let Some(days) = reminder_days_before {
        body.insert("reminder_days_before".into(), Value::from(days));
    }
    if let Some(offsets) = reminder_offsets {
        body.insert(
            "reminder_offsets".into(),
            Value::Array(offsets.into_iter().map(Value::from).collect()),
        );
    }
    if let Some(minutes) = travel_time_minutes {
        body.insert("travel_time_minutes".into(), Value::from(minutes));
    }
    insert_string(&mut body, "priority", priority);
    insert_string(&mut body, "notes", notes);
    insert_string(&mut body, "color", color);
    if let Some(progress_value) = normalize_progress_value(progress) {
        body.insert("progress".into(), progress_value);
    }
    if let Some(t) = tags {
        let arr: Vec<Value> = t
            .split(',')
            .map(|s| Value::String(s.trim().to_string()))
            .collect();
        body.insert("tags".into(), Value::Array(arr));
    }
    if let Some(a) = assign {
        body.insert("assigned_to".into(), Value::Array(parse_id_values(&a)));
    }
    if body.is_empty() {
        return Err(tr("cli-no-changes"));
    }
    let mut item: ListItem = api.put(&format!("/items/{id}"), &body).await?;
    if !tags_provided {
        enrich_item_tags_from_list(api, &mut item).await;
    }
    json_or!(
        as_json,
        item,
        println!("{} {}", "✓".green(), tr("cli-item-updated"))
    );
    Ok(())
}

fn insert_string(body: &mut serde_json::Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        body.insert(key.into(), Value::String(value));
    }
}

fn parse_id_values(raw: &str) -> Vec<Value> {
    raw.split(',')
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .map(Value::from)
        .collect()
}

fn empty_json_object() -> Value {
    Value::Object(serde_json::Map::new())
}

async fn run_items_done(api: &ApiClient, as_json: bool, id: i64) -> Result<(), String> {
    let mut resp: Value = api
        .patch_json(&format!("/items/{id}/done"), &empty_json_object())
        .await?;
    enrich_done_response_tags(api, id, &mut resp).await;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&resp).unwrap_or_default()
        );
    } else {
        println!("{} {}", "✓".green(), tr("cli-item-toggled"));
    }
    Ok(())
}

async fn run_items_vote(api: &ApiClient, as_json: bool, id: i64) -> Result<(), String> {
    let resp: Value = api
        .patch_json(&format!("/items/{id}/upvote"), &empty_json_object())
        .await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&resp).unwrap_or_default()
        );
        return Ok(());
    }

    let upvoted_by_me = resp
        .get("upvoted_by_me")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let upvote_count = resp
        .get("upvote_count")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let key = if upvoted_by_me {
        "cli-item-upvoted"
    } else {
        "cli-item-vote-removed"
    };
    println!(
        "{} {}",
        "✓".green(),
        tr_args(
            key,
            &[("id", id.to_string()), ("count", upvote_count.to_string())],
        )
    );
    Ok(())
}

async fn run_items_delete(api: &ApiClient, as_json: bool, id: i64) -> Result<(), String> {
    let resp: OkResponse = api.delete(&format!("/items/{id}")).await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&resp).unwrap_or_default()
        );
    } else {
        println!("{} {}", "✓".green(), tr("cli-item-deleted"));
        if let Some(t) = resp.undo_token {
            println!("  {}: {t}", tr("label-undo-token"));
        }
    }
    Ok(())
}

async fn run_items_done_list(api: &ApiClient, as_json: bool, list_id: i64) -> Result<(), String> {
    let list = if as_json {
        None
    } else {
        api.get::<ShoppingList>(&format!("/lists/{list_id}"))
            .await
            .ok()
    };
    let items: Vec<ListItem> = api.get(&format!("/lists/{list_id}/items")).await?;
    let done: Vec<ListItem> = items
        .into_iter()
        .filter(|i| i.is_done.unwrap_or(false))
        .collect();
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&done).unwrap_or_default()
        );
    } else if done.is_empty() {
        println!("{}", tr("cli-no-completed-items").dimmed());
    } else {
        output::print_items_for_list(list.as_ref(), &done);
    }
    maybe_auto_handoff(
        api,
        list_id,
        list.as_ref().map(|l| l.name.as_str()),
        as_json,
    )
    .await;
    Ok(())
}

async fn run_items_comment(
    api: &ApiClient,
    as_json: bool,
    id: i64,
    text: String,
) -> Result<(), String> {
    let resp: Value = api
        .post(&format!("/items/{id}/comments"), &json!({"text": text}))
        .await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&resp).unwrap_or_default()
        );
    } else {
        println!("{} {}", "✓".green(), tr("cli-comment-added"));
    }
    Ok(())
}

async fn run_items_check_all(api: &ApiClient, as_json: bool, list_id: i64) -> Result<(), String> {
    let resp: Value = api
        .post(&format!("/lists/{list_id}/check-all"), &empty_json_object())
        .await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&resp).unwrap_or_default()
        );
    } else {
        println!("{} {}", "✓".green(), tr("cli-check-all-done"));
    }
    Ok(())
}

async fn run_items_clear_done(api: &ApiClient, as_json: bool, list_id: i64) -> Result<(), String> {
    let resp: Value = api
        .post(
            &format!("/lists/{list_id}/clear-done"),
            &empty_json_object(),
        )
        .await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&resp).unwrap_or_default()
        );
    } else {
        let count = resp.get("count").and_then(|v| v.as_i64()).unwrap_or(0);
        println!(
            "{} {}",
            "✓".green(),
            tr_args("cli-clear-done", &[("count", count.to_string())])
        );
    }
    Ok(())
}

// ─── Folders ───

async fn run_folders(cmd: FolderCmd, as_json: bool) -> Result<(), String> {
    let api = get_api()?;
    match cmd {
        FolderCmd::List => {
            let f: Vec<Folder> = api.get("/folders").await?;
            json_or!(as_json, f, output::print_folders(&f));
        }
        FolderCmd::Create {
            name,
            icon,
            color,
            parent,
        } => {
            let body = CreateFolder {
                name,
                icon,
                color,
                parent_folder_id: parent,
            };
            let f: Folder = api.post("/folders", &body).await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&f).unwrap_or_default());
            } else {
                println!(
                    "{} {}",
                    "✓".green(),
                    tr_args("cli-folder-created", &[("id", f.id.to_string())])
                );
            }
        }
        FolderCmd::Update {
            id,
            name,
            icon,
            color,
            parent,
        } => {
            let mut body = serde_json::Map::new();
            if let Some(n) = name {
                body.insert("name".into(), Value::String(n));
            }
            if let Some(i) = icon {
                body.insert("icon".into(), Value::String(i));
            }
            if let Some(c) = color {
                body.insert("color".into(), Value::String(c));
            }
            if let Some(parent_id) = parent {
                body.insert("parent_folder_id".into(), Value::from(parent_id));
            }
            if body.is_empty() {
                return Err(tr("cli-no-changes"));
            }
            let f: Folder = api.put(&format!("/folders/{id}"), &body).await?;
            json_or!(
                as_json,
                f,
                println!("{} {}", "✓".green(), tr("cli-folder-updated"))
            );
        }
        FolderCmd::Delete { id } => {
            let _: OkResponse = api.delete(&format!("/folders/{id}")).await?;
            println!("{} {}", "✓".green(), tr("cli-folder-deleted"));
        }
    }
    Ok(())
}

// ─── Members ───

async fn run_members(cmd: MemberCmd, as_json: bool) -> Result<(), String> {
    let api = get_api()?;
    match cmd {
        MemberCmd::List { list_id } => {
            let m: Vec<Member> = api.get(&format!("/lists/{list_id}/members")).await?;
            json_or!(as_json, m, output::print_members(&m));
        }
        MemberCmd::Invite {
            list_id,
            email,
            role,
        } => {
            let _: Value = api
                .post(
                    &format!("/lists/{list_id}/invite"),
                    &json!({"email": email, "role": role}),
                )
                .await?;
            println!(
                "{} {}",
                "✓".green(),
                tr_args("cli-member-invited", &[("email", email)])
            );
        }
        MemberCmd::Remove { list_id, user_id } => {
            api.delete_ok(&format!("/lists/{list_id}/members/{user_id}"))
                .await?;
            println!("{} {}", "✓".green(), tr("cli-member-removed"));
        }
        MemberCmd::Role {
            list_id,
            user_id,
            role,
        } => {
            let _: Value = api
                .patch_json(
                    &format!("/lists/{list_id}/members/{user_id}"),
                    &json!({"role": role}),
                )
                .await?;
            println!("{} {}", "✓".green(), tr("cli-member-role-updated"));
        }
        MemberCmd::InviteLink { list_id } => {
            let resp: Value = api
                .post(
                    &format!("/lists/{list_id}/invite-link"),
                    &empty_json_object(),
                )
                .await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else if let Some(url) = invite_url_from_response(&resp) {
                println!("{}", tr_args("cli-invite-link", &[("url", url)]),);
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            }
        }
        MemberCmd::Unshare { list_id } => {
            api.delete_ok(&format!("/lists/{list_id}/share")).await?;
            println!("{} {}", "✓".green(), tr("cli-public-link-removed"));
        }
        MemberCmd::Leave { list_id } => {
            let _: Value = api
                .post(&format!("/lists/{list_id}/leave"), &empty_json_object())
                .await?;
            println!("{} {}", "✓".green(), tr("cli-list-left"));
        }
    }
    Ok(())
}

// ─── API Keys ───

fn display_api_key_scopes(scopes: &ApiKeyScopes) -> String {
    let map_scope = |scope: &str| {
        if scope.trim().eq_ignore_ascii_case("all") {
            tr("label-all")
        } else {
            scope.trim().to_string()
        }
    };

    match scopes {
        ApiKeyScopes::Single(value) => map_scope(value),
        ApiKeyScopes::Multiple(values) => values
            .iter()
            .map(|value| map_scope(value))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

async fn run_keys(cmd: KeyCmd, as_json: bool) -> Result<(), String> {
    let api = get_api()?;
    match cmd {
        KeyCmd::List => {
            let keys: Vec<ApiKey> = api.get("/api-keys").await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&keys).unwrap_or_default()
                );
            } else if keys.is_empty() {
                println!("{}", tr("cli-no-api-keys").dimmed());
            } else {
                for k in &keys {
                    let active = if k.is_active.unwrap_or(true) {
                        tr("label-active").green().to_string()
                    } else {
                        tr("label-revoked").red().to_string()
                    };
                    let scopes = k
                        .scopes
                        .as_ref()
                        .map_or_else(|| tr("label-all"), display_api_key_scopes);
                    let label = k.name.clone().unwrap_or_else(|| tr("label-no-name"));
                    let last = k.last_used_at.clone().unwrap_or_else(|| tr("label-never"));
                    println!(
                        "  #{:<4} {:<20} [{active}]  {}: {scopes}  {}: {last}",
                        k.id,
                        label.bold(),
                        tr("label-scopes"),
                        tr("label-last-used"),
                    );
                }
            }
        }
        KeyCmd::Create { name, scopes } => {
            let scope_list: Vec<String> = scopes.split(',').map(|s| s.trim().to_string()).collect();
            let resp: Value = api
                .post(
                    "/api-keys",
                    &json!({
                        "name": name,
                        "scopes": scope_list,
                    }),
                )
                .await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else {
                if let Some(raw) = resp.get("key").and_then(|v| v.as_str()) {
                    println!("{} {}", "✓".green(), tr("cli-api-key-created"));
                    println!();
                    println!("  {}", raw.bold());
                    println!();
                    println!("  {} {}", "!".yellow(), tr("cli-api-key-save-warning"));
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&resp).unwrap_or_default()
                    );
                }
            }
        }
        KeyCmd::Revoke { key_id } => {
            let _: Value = api.delete(&format!("/api-keys/{key_id}")).await?;
            println!(
                "{} {}",
                "✓".green(),
                tr_args("cli-api-key-revoked", &[("id", key_id.to_string())])
            );
        }
    }
    Ok(())
}

// ─── Search / Activity / Undo / Redo / Profile / Ping / Config ───

async fn run_search(query: &str, as_json: bool) -> Result<(), String> {
    let api = get_api()?;
    let raw: Value = api.get_query("/search", &[("q", query)]).await?;
    let response_for_json = if as_json {
        Some(
            SearchResponse::from_value(raw.clone())
                .map_err(|e| tr_args("cli-search-parse-error", &[("error", e)]))?,
        )
    } else {
        None
    };
    let grouped = SearchResponse::from_value(raw)
        .map_err(|e| tr_args("cli-search-parse-error", &[("error", e)]))?;

    let mut grouped = grouped.into_grouped();
    let mut fallback_applied = false;

    if let Some(item_id) = parse_search_item_id(query) {
        let item_already_present = grouped
            .items
            .as_ref()
            .is_some_and(|items| items.iter().any(|item| item.id == item_id));

        if !item_already_present {
            if let Some((item, list)) = find_item_across_lists(&api, item_id).await? {
                let fallback_hit = SearchItemHit {
                    id: item.id,
                    text: item.text,
                    list_id: item.list_id.or(Some(list.id)),
                    list_name: Some(list.name),
                    is_done: item.is_done,
                };
                grouped
                    .items
                    .get_or_insert_with(Vec::new)
                    .push(fallback_hit);
                fallback_applied = true;
            }
        }
    }

    if as_json {
        if fallback_applied {
            println!(
                "{}",
                serde_json::to_string_pretty(&grouped).unwrap_or_default()
            );
        } else if let Some(response) = response_for_json {
            println!(
                "{}",
                serde_json::to_string_pretty(&response).unwrap_or_default()
            );
        }
    } else {
        output::print_search(&grouped);
    }
    Ok(())
}

async fn run_activity(list_id: i64, limit: u32, as_json: bool) -> Result<(), String> {
    let api = get_api()?;
    let lim = limit.to_string();
    let e: Vec<ActivityEntry> = api
        .get_query(&format!("/lists/{list_id}/activity"), &[("limit", &lim)])
        .await?;
    json_or!(as_json, e, output::print_activity(&e));
    maybe_auto_handoff(&api, list_id, None, as_json).await;
    Ok(())
}

async fn run_undo(list_id: i64) -> Result<(), String> {
    let api = get_api()?;
    let _: Value = api
        .post(&format!("/lists/{list_id}/undo"), &empty_json_object())
        .await?;
    println!("{} {}", "✓".green(), tr("cli-undo-done"));
    Ok(())
}

async fn run_redo(list_id: i64) -> Result<(), String> {
    let api = get_api()?;
    let _: Value = api
        .post(&format!("/lists/{list_id}/redo"), &empty_json_object())
        .await?;
    println!("{} {}", "✓".green(), tr("cli-redo-done"));
    Ok(())
}

fn normalize_optional_text(raw: Option<String>) -> Option<String> {
    raw.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn default_handoff_device_label(raw: Option<String>) -> String {
    if let Some(value) = normalize_optional_text(raw) {
        return value.chars().take(80).collect();
    }
    if let Ok(value) = std::env::var(KRAMLI_DEVICE_LABEL_ENV) {
        if !value.trim().is_empty() {
            return value.trim().chars().take(80).collect();
        }
    }
    "Kramli CLI".to_string()
}

async fn run_handoff(cmd: HandoffCmd, as_json: bool) -> Result<(), String> {
    let api = get_api()?;

    match cmd {
        HandoffCmd::Viewing {
            list_id,
            list_name,
            device,
        } => {
            let body = handoff_body(list_id, list_name, default_handoff_device_label(device));
            let resp: Value = api.post("/activity/viewing", &body).await?;
            json_or!(
                as_json,
                resp,
                println!("{} {}", "✓".green(), tr("cli-handoff-viewing-sent"))
            );
        }
        HandoffCmd::Continue {
            list_id,
            list_name,
            device,
        } => {
            let body = handoff_body(list_id, list_name, default_handoff_device_label(device));
            let resp: Value = api.post("/activity/continue-on-device", &body).await?;
            json_or!(
                as_json,
                resp,
                println!("{} {}", "✓".green(), tr("cli-handoff-continue-sent"))
            );
        }
        HandoffCmd::Clear => {
            let resp: Value = api.post("/activity/clear", &empty_json_object()).await?;
            json_or!(
                as_json,
                resp,
                println!("{} {}", "✓".green(), tr("cli-handoff-cleared"))
            );
        }
    }

    Ok(())
}

async fn run_batch(file: &str, keep_going: bool, as_json: bool) -> Result<(), String> {
    if as_json {
        return run_batch_json(file, keep_going).await;
    }

    let source = if file == "-" { "stdin" } else { file }.to_string();

    let content = if file == "-" {
        use tokio::io::AsyncReadExt;
        let mut buffer = String::default();
        tokio::io::stdin()
            .read_to_string(&mut buffer)
            .await
            .map_err(|e| tr_args("cli-batch-read-stdin-error", &[("error", e.to_string())]))?;
        buffer
    } else {
        fs::read_to_string(file).map_err(|e| {
            tr_args(
                "cli-batch-read-file-error",
                &[("file", file.to_string()), ("error", e.to_string())],
            )
        })?
    };

    let mut executed = 0usize;
    let mut failed = 0usize;
    let mut first_error: Option<String> = None;

    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        executed += 1;

        let mut args = match shell_words::split(line) {
            Ok(parsed) => parsed,
            Err(e) => {
                let err = tr_args(
                    "cli-batch-parse-error",
                    &[
                        ("source", source.clone()),
                        ("line", line_no.to_string()),
                        ("error", e.to_string()),
                    ],
                );
                if !record_batch_failure(&mut failed, &mut first_error, err, keep_going) {
                    break;
                }
                continue;
            }
        };

        if args.first().is_some_and(|arg| arg == "kramli") {
            args.remove(0);
        }
        if args.is_empty() {
            continue;
        }

        let mut argv = Vec::with_capacity(args.len() + 1);
        argv.push("kramli".to_string());
        argv.extend(args);

        let mut nested_cli = match Cli::try_parse_from(&argv) {
            Ok(parsed) => parsed,
            Err(e) => {
                let err = tr_args(
                    "cli-batch-parse-error",
                    &[
                        ("source", source.clone()),
                        ("line", line_no.to_string()),
                        ("error", e.to_string()),
                    ],
                );
                if !record_batch_failure(&mut failed, &mut first_error, err, keep_going) {
                    break;
                }
                continue;
            }
        };

        if matches!(nested_cli.command, Some(Commands::Batch { .. })) {
            let err = tr_args(
                "cli-batch-nested-not-supported",
                &[("source", source.clone()), ("line", line_no.to_string())],
            );
            if !record_batch_failure(&mut failed, &mut first_error, err, keep_going) {
                break;
            }
            continue;
        }

        nested_cli.json = false;
        println!("{} {line}", "→".cyan());

        if let Err(err) = Box::pin(run(nested_cli)).await {
            failed += 1;
            if first_error.is_none() {
                first_error = Some(format!("line {line_no}: {err}"));
            }
            eprintln!("{} line {line_no}: {err}", "✗".red());
            if !keep_going {
                break;
            }
        }
    }

    let succeeded = executed.saturating_sub(failed);
    if failed == 0 {
        println!(
            "{} {}",
            "✓".green(),
            tr_args(
                "cli-batch-complete-success",
                &[("count", succeeded.to_string())]
            )
        );
        return Ok(());
    }

    println!(
        "{} {}",
        "!".yellow(),
        tr_args(
            "cli-batch-complete-partial",
            &[
                ("success", succeeded.to_string()),
                ("failed", failed.to_string()),
            ],
        )
    );

    Err(first_error.unwrap_or_else(|| tr("cli-batch-failed")))
}

fn record_batch_failure(
    failed: &mut usize,
    first_error: &mut Option<String>,
    err: String,
    keep_going: bool,
) -> bool {
    *failed += 1;
    if first_error.is_none() {
        *first_error = Some(err.clone());
    }
    eprintln!("{} {err}", "✗".red());
    keep_going
}

async fn run_batch_json(file: &str, keep_going: bool) -> Result<(), String> {
    let source = if file == "-" {
        "stdin".to_string()
    } else {
        file.to_string()
    };

    let content = if file == "-" {
        use tokio::io::AsyncReadExt;
        let mut buffer = String::default();
        tokio::io::stdin()
            .read_to_string(&mut buffer)
            .await
            .map_err(|e| tr_args("cli-batch-read-stdin-error", &[("error", e.to_string())]))?;
        buffer
    } else {
        fs::read_to_string(file).map_err(|e| {
            tr_args(
                "cli-batch-read-file-error",
                &[("file", file.to_string()), ("error", e.to_string())],
            )
        })?
    };

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut results = Vec::new();
    let mut executed = 0usize;
    let mut failed = 0usize;
    let mut first_error: Option<String> = None;

    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        executed += 1;

        let args = match batch_child_args(line) {
            Ok(args) => args,
            Err(error) => {
                let err = tr_args(
                    "cli-batch-parse-error",
                    &[
                        ("source", source.clone()),
                        ("line", line_no.to_string()),
                        ("error", error),
                    ],
                );
                failed += 1;
                if first_error.is_none() {
                    first_error = Some(err.clone());
                }
                results.push(json!({
                    "line": line_no,
                    "command": line,
                    "ok": false,
                    "exit_code": null,
                    "error": err,
                }));
                if !keep_going {
                    break;
                }
                continue;
            }
        };

        let output = TokioCommand::new(&exe)
            .arg("--json")
            .args(&args)
            .output()
            .await;

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let ok = output.status.success();
                if !ok {
                    failed += 1;
                }
                let mut result = json!({
                    "line": line_no,
                    "command": line,
                    "ok": ok,
                    "exit_code": output.status.code(),
                    "stdout": stdout,
                    "stderr": stderr,
                });
                if let Ok(parsed) = serde_json::from_str::<Value>(stdout.trim()) {
                    result["json"] = parsed;
                }
                if !ok && first_error.is_none() {
                    first_error = Some(format!("line {line_no}: command failed"));
                }
                results.push(result);
                if !ok && !keep_going {
                    break;
                }
            }
            Err(error) => {
                failed += 1;
                let err = format!("line {line_no}: {error}");
                if first_error.is_none() {
                    first_error = Some(err.clone());
                }
                results.push(json!({
                    "line": line_no,
                    "command": line,
                    "ok": false,
                    "exit_code": null,
                    "error": err,
                }));
                if !keep_going {
                    break;
                }
            }
        }
    }

    let succeeded = executed.saturating_sub(failed);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "ok": failed == 0,
            "source": source,
            "executed": executed,
            "succeeded": succeeded,
            "failed": failed,
            "results": results,
        }))
        .unwrap_or_default()
    );

    if failed == 0 {
        Ok(())
    } else {
        Err(first_error.unwrap_or_else(|| tr("cli-batch-failed")))
    }
}

fn batch_child_args(line: &str) -> Result<Vec<String>, String> {
    let mut args = shell_words::split(line).map_err(|e| e.to_string())?;
    if args.first().is_some_and(|arg| arg == "kramli") {
        args.remove(0);
    }
    args.retain(|arg| arg != "--json");
    if args.is_empty() {
        return Err(tr("cli-batch-failed"));
    }

    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push("kramli".to_string());
    argv.extend(args.iter().cloned());
    let nested_cli = Cli::try_parse_from(&argv).map_err(|e| e.to_string())?;
    if matches!(nested_cli.command, Some(Commands::Batch { .. })) {
        return Err(tr("cli-batch-nested-not-supported"));
    }
    Ok(args)
}

async fn run_security(cmd: SecurityCmd, as_json: bool) -> Result<(), String> {
    let api = get_api()?;
    match cmd {
        SecurityCmd::Status => {
            let data: Value = api.get("/security").await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&data).unwrap_or_default()
                );
                return Ok(());
            }
            let summary = data.get("security").cloned().unwrap_or(Value::Null);
            let level = summary
                .get("level_label")
                .and_then(Value::as_str)
                .or_else(|| summary.get("level").and_then(Value::as_str))
                .unwrap_or("-");
            let score = summary.get("score").and_then(Value::as_u64).unwrap_or(0);
            let max_score = summary
                .get("max_score")
                .and_then(Value::as_u64)
                .unwrap_or(100);
            println!("{} {}", tr("label-security-level"), level.bold());
            println!("{}        {score}/{max_score}", tr("label-score"));
            let login_alerts = data
                .get("security_email_login_alerts")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            println!(
                "{}   {}",
                tr("label-login-emails"),
                if login_alerts {
                    tr("label-on")
                } else {
                    tr("label-off")
                }
            );
            if let Some(factors) = summary.get("factors").and_then(Value::as_array) {
                println!("\n{}", tr("label-factors"));
                for factor in factors {
                    let label = factor.get("label").and_then(Value::as_str).unwrap_or("?");
                    let met = factor.get("met").and_then(Value::as_bool).unwrap_or(false);
                    let mark = if met { "✓".green() } else { "·".normal() };
                    println!("  {mark} {label}");
                }
            }
            Ok(())
        }
        SecurityCmd::Ack { token } => {
            let token = token
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .or_else(|| std::env::var(KRAMLI_ACK_TOKEN_ENV).ok());
            let Some(token) = token else {
                return Err(tr("cli-security-ack-token-missing"));
            };
            let body = json!({ "token": token });
            let data: Value = api.post("/security/login-ack", &body).await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&data).unwrap_or_default()
                );
                return Ok(());
            }
            if data.get("ok").and_then(Value::as_bool) == Some(true) {
                let default_message = tr("cli-security-ack-confirmed");
                let message = match data.get("message").and_then(Value::as_str) {
                    Some(message) => message,
                    None => default_message.as_str(),
                };
                println!("{} {}", "✓".green(), message);
                Ok(())
            } else {
                let default_message = tr("cli-security-ack-failed");
                let message = match data
                    .get("error")
                    .or_else(|| data.get("message"))
                    .and_then(Value::as_str)
                {
                    Some(message) => message,
                    None => default_message.as_str(),
                };
                Err(message.to_string())
            }
        }
    }
}

async fn run_profile(as_json: bool) -> Result<(), String> {
    let api = get_api()?;
    let p: Profile = api.get("/profile").await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&profile_json_with_lang(&p)).unwrap_or_default()
        );
        return Ok(());
    }
    println!(
        "{}   {}",
        tr("label-name"),
        p.display_name
            .clone()
            .unwrap_or_else(|| tr("common-unknown"))
            .bold()
    );
    println!(
        "{} {}",
        tr("label-email"),
        p.email.as_deref().unwrap_or("-")
    );
    if let Some(id) = p.id {
        println!("{}     {id}", tr("label-id"));
    }
    if p.is_anonymous.unwrap_or(false) {
        println!("        {}", tr("label-guest-account"));
    }
    Ok(())
}

async fn run_accept_terms(docs: Option<Vec<String>>, as_json: bool) -> Result<(), String> {
    let api = get_api()?;

    let normalized_docs = docs.map(|values| {
        values
            .into_iter()
            .map(|value| value.trim().to_lowercase())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
    });

    if let Some(ref values) = normalized_docs {
        for value in values {
            if value != "agb" && value != "privacy" {
                return Err(tr_args(
                    "cli-invalid-doc-key",
                    &[("value", value.to_string())],
                ));
            }
        }
    }

    let body = match normalized_docs {
        Some(values) if !values.is_empty() => json!({ "docs": values }),
        _ => Value::Object(serde_json::Map::new()),
    };

    let resp: Value = api.post("/accept-terms", &body).await?;
    if as_json {
        let pretty = serde_json::to_string_pretty(&resp).unwrap_or_default();
        println!("{pretty}");
        return Ok(());
    }

    let pending_count = resp
        .get("legal")
        .and_then(|legal| legal.get("pending"))
        .and_then(Value::as_array)
        .map_or(0, |pending| pending.len());

    if pending_count == 0 {
        println!("{} {}", "✓".green(), tr("cli-accepted-terms-all"));
    } else {
        println!(
            "{} {}",
            "✓".green(),
            tr_args(
                "cli-accepted-terms-pending",
                &[("count", pending_count.to_string())],
            )
        );
    }

    Ok(())
}

async fn run_update_check(as_json: bool) -> Result<(), String> {
    let release = fetch_latest_release().await?;
    let now = unix_timestamp_secs();

    let mut cfg = Config::load();
    cfg.set_update_check_state(
        now,
        Some(release.tag_name.clone()),
        release.html_url.clone(),
    );
    let _ = cfg.save();

    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let latest_version = release.tag_name.clone();
    let update_available = update_is_available(&current_version, &latest_version);

    if as_json {
        let out = json!({
            "current_version": current_version,
            "latest_version": latest_version,
            "update_available": update_available,
            "url": release.html_url,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return Ok(());
    }

    match update_available {
        Some(true) => {
            println!(
                "{} {}",
                "↑".cyan(),
                tr_args(
                    "cli-update-available",
                    &[
                        ("current", current_version),
                        ("latest", latest_version.clone()),
                    ],
                )
            );
            if let Some(url) = release.html_url {
                println!("  {}", tr_args("cli-update-open-url", &[("url", url)]));
            }
            println!("  {}", tr("cli-update-auto-hint"));
        }
        Some(false) => {
            println!(
                "{} {}",
                "✓".green(),
                tr_args(
                    "cli-update-up-to-date",
                    &[("current", current_version), ("latest", latest_version)],
                )
            );
        }
        None => {
            println!(
                "{} {}",
                "!".yellow(),
                tr_args(
                    "cli-update-version-unknown",
                    &[("current", current_version), ("latest", latest_version),],
                )
            );
        }
    }

    Ok(())
}

async fn run_ping(as_json: bool) -> Result<(), String> {
    let cfg = Config::load();
    let url = format!("{}/api/ping", cfg.base_url().trim_end_matches('/'));
    let start = std::time::Instant::now();
    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| tr_args("api-network-error", &[("error", e.to_string())]))?;
    let ms = start.elapsed().as_millis();
    let ok = resp.status().is_success();
    if as_json {
        println!(
            "{}",
            json!({"ok": ok, "ms": ms, "status": resp.status().as_u16()})
        );
        return Ok(());
    }
    if ok {
        println!(
            "{} {}",
            "✓".green(),
            tr_args("cli-ping-ok", &[("ms", ms.to_string())])
        );
    } else {
        println!(
            "{} {}",
            "✗".red(),
            tr_args(
                "cli-ping-failed",
                &[("status", resp.status().as_u16().to_string())]
            )
        );
    }
    Ok(())
}

fn run_config(as_json: bool) -> Result<(), String> {
    let cfg = Config::load();
    let telemetry_enabled = telemetry::is_enabled();
    let bootstrap_icons_enabled = cfg.bootstrap_icons_enabled();
    let auto_update_enabled = auto_update_check_enabled();
    if as_json {
        let out = json!({
            "config_path": Config::path().display().to_string(),
            "server": cfg.base_url(),
            "api_key_stored": cfg.has_api_key(),
            "api_key_source": if cfg.api_key_from_env() { "env" } else { "keychain" },
            "telemetry_enabled": telemetry_enabled,
            "bootstrap_icons_enabled": bootstrap_icons_enabled,
            "auto_update_check": auto_update_enabled,
            "last_update_check": cfg.update_check_last(),
            "last_update_version": cfg.update_check_latest(),
            "last_update_url": cfg.update_check_url(),
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return Ok(());
    }
    println!(
        "{}  {}",
        tr("label-configuration"),
        Config::path().display()
    );
    println!("{}         {}", tr("label-server"), cfg.base_url());
    let src = if cfg.api_key_from_env() {
        format!(" ({})", tr("label-environment"))
    } else {
        format!(" ({})", tr("label-keychain"))
    };
    println!(
        "{} {}{}",
        tr("label-api-key"),
        if cfg.has_api_key() {
            tr("label-stored").green().to_string()
        } else {
            tr("label-not-set").red().to_string()
        },
        src
    );
    println!(
        "{} {}",
        tr("label-telemetry"),
        if telemetry_enabled {
            tr("label-enabled").green().to_string()
        } else {
            tr("label-disabled").yellow().to_string()
        }
    );
    println!(
        "{} {}",
        tr("label-bootstrap-icons"),
        if bootstrap_icons_enabled {
            tr("label-enabled").green().to_string()
        } else {
            tr("label-disabled").yellow().to_string()
        }
    );
    println!(
        "{} {}",
        tr("label-update-check"),
        if auto_update_enabled {
            tr("label-on").green().to_string()
        } else {
            tr("label-off").yellow().to_string()
        }
    );
    let last_check = cfg
        .update_check_last()
        .map_or_else(|| tr("label-never"), |value| value.to_string());
    println!("{}    {}", tr("label-last-check"), last_check);
    Ok(())
}

fn run_privacy(cmd: PrivacyCmd, as_json: bool) -> Result<(), String> {
    match cmd {
        PrivacyCmd::Reset => {
            let mut cfg = Config::load();
            cfg.reset_privacy_preferences();
            cfg.save()?;
            let out = json!({
                "ok": true,
                "config_path": Config::path().display().to_string(),
                "reset": ["telemetry_enabled", "bootstrap_icons_enabled"],
            });
            if as_json {
                println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            } else {
                println!("{} Privacy preferences reset.", "✓".green());
                println!("  {}", Config::path().display());
            }
            Ok(())
        }
    }
}
