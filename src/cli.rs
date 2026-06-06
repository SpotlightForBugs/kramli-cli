use std::fs;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use colored::control::set_override;
use colored::Colorize;
use serde_json::{json, Value};

use crate::api::ApiClient;
use crate::config::Config;
use crate::models::*;
use crate::output;

#[derive(Parser)]
#[command(
    name = "kramli",
    about = "Kramli \u{2013} shopping list & todo CLI",
    version,
    propagate_version = true
)]
pub struct Cli {
    /// Output machine-readable JSON instead of human-friendly text.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Log in with an API key (generate one at kramli.de/settings#api-keys)
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
        action: ItemCmd,
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
        /// Search query (min 2 characters)
        query: String,
    },
    /// Show activity feed for a list
    Activity {
        /// List ID
        list_id: i64,
        /// Number of entries
        #[arg(short, long, default_value = "20")]
        limit: u32,
    },
    /// Undo the last action on a list
    Undo { list_id: i64 },
    /// Redo the last undone action on a list
    Redo { list_id: i64 },
    /// Send or clear cross-device handoff events
    Handoff {
        #[command(subcommand)]
        action: HandoffCmd,
    },
    /// Show your profile
    Profile,
    /// Account security level and login confirmation
    Security {
        #[command(subcommand)]
        action: SecurityCmd,
    },
    /// Accept pending legal terms/privacy documents
    #[command(name = "accept-terms")]
    AcceptTerms {
        /// Optional document keys (comma-separated): agb,privacy
        #[arg(long, value_delimiter = ',')]
        docs: Option<Vec<String>>,
    },
    /// Check server connectivity
    Ping,
    /// Show CLI configuration
    Config,
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

// ─── Subcommands ───

#[derive(Subcommand)]
pub enum ListCmd {
    /// Show all lists
    #[command(alias = "ls")]
    List,
    /// Resolve a list reference (ID, /lists/l/<slug>, or full URL)
    Resolve { reference: String },
    /// Show list details
    Show { id: i64 },
    /// Create a new list
    Create {
        name: String,
        #[arg(short, long)]
        icon: Option<String>,
        #[arg(short, long)]
        color: Option<String>,
        #[arg(short, long)]
        folder: Option<i64>,
        /// Custom states as CSV (e.g. "Open,In Progress,Review,Done")
        /// or JSON array (e.g. '[{"name":"Review","color":"#7c3aed"}]').
        #[arg(long)]
        states: Option<String>,
    },
    /// Update a list
    Update {
        id: i64,
        #[arg(short, long)]
        name: Option<String>,
        #[arg(short, long)]
        icon: Option<String>,
        #[arg(short, long)]
        color: Option<String>,
        /// Custom states as CSV (e.g. "Open,In Progress,Review,Done")
        /// or JSON array (e.g. '[{"name":"Review","color":"#7c3aed"}]').
        #[arg(long)]
        states: Option<String>,
    },
    /// Delete a list
    #[command(alias = "rm")]
    Delete { id: i64 },
    /// Move a list into a folder
    Move {
        id: i64,
        /// Folder ID (omit to remove from folder)
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
        return Err("List reference is empty.".into());
    }

    if let Ok(id) = raw.parse::<i64>() {
        return Ok(id);
    }

    if let Some(slug) = extract_slug_from_reference(raw) {
        if let Some(id) = decode_list_slug(&slug) {
            return Ok(id);
        }
    }

    Err(
        "Could not resolve list reference. Use a numeric ID, a slug like '1eSwyM', or a URL like https://kramli.de/lists/l/1eSwyM".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn rejects_empty_list_references() {
        assert!(resolve_list_reference("  ").is_err());
    }
}

#[derive(Subcommand)]
pub enum ItemCmd {
    /// Show all items in a list
    #[command(alias = "ls")]
    List {
        list_id: i64,
        /// Show only open (not done) items
        #[arg(long, conflicts_with = "completed")]
        open: bool,
        /// Show only completed items
        #[arg(long, conflicts_with = "open")]
        completed: bool,
        /// Filter by custom state/progress label
        #[arg(long)]
        state: Option<String>,
        /// Filter by case-insensitive text match in item title
        #[arg(long)]
        contains: Option<String>,
    },
    /// Show detailed view of a single item (notes, images, comments)
    Show { id: i64 },
    /// Add a new item
    Add {
        list_id: i64,
        text: String,
        #[arg(short, long)]
        quantity: Option<String>,
        #[arg(short, long)]
        due: Option<String>,
        #[arg(short, long)]
        priority: Option<String>,
        #[arg(short, long)]
        tags: Option<String>,
        #[arg(short, long)]
        notes: Option<String>,
        #[arg(long)]
        parent: Option<i64>,
        /// Assign to user IDs (comma-separated)
        #[arg(short, long)]
        assign: Option<String>,
        /// Item colour (hex, e.g. #ff4d4f)
        #[arg(long)]
        color: Option<String>,
        /// Item state / progress label (e.g. "In Progress", "Review")
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
        #[arg(short, long)]
        priority: Option<String>,
        #[arg(long)]
        tags: Option<String>,
        #[arg(short, long)]
        notes: Option<String>,
        /// Assign to user IDs (comma-separated)
        #[arg(short, long)]
        assign: Option<String>,
        /// Item colour (hex)
        #[arg(long)]
        color: Option<String>,
        /// Item state / progress label (e.g. "In Progress", "Review")
        #[arg(long, alias = "state")]
        progress: Option<String>,
    },
    /// Toggle item done/undone
    #[command(alias = "check")]
    Done { id: i64 },
    /// Toggle upvote on an item
    #[command(alias = "upvote")]
    Vote { id: i64 },
    /// Delete an item
    #[command(alias = "rm")]
    Delete { id: i64 },
    /// Show only completed items
    #[command(name = "done-list")]
    DoneList { list_id: i64 },
    /// Add a comment to an item
    Comment { id: i64, text: String },
    /// Mark all items as done
    #[command(name = "check-all")]
    CheckAll { list_id: i64 },
    /// Delete all completed items
    #[command(name = "clear-done")]
    ClearDone { list_id: i64 },
}

#[derive(Subcommand)]
pub enum FolderCmd {
    #[command(alias = "ls")]
    List,
    Create {
        name: String,
        #[arg(short, long)]
        icon: Option<String>,
        #[arg(short, long)]
        color: Option<String>,
    },
    Update {
        id: i64,
        #[arg(short, long)]
        name: Option<String>,
        #[arg(short, long)]
        icon: Option<String>,
        #[arg(short, long)]
        color: Option<String>,
    },
    #[command(alias = "rm")]
    Delete { id: i64 },
}

#[derive(Subcommand)]
pub enum MemberCmd {
    #[command(alias = "ls")]
    List {
        list_id: i64,
    },
    Invite {
        list_id: i64,
        email: String,
        #[arg(short, long, default_value = "editor")]
        role: String,
    },
    Remove {
        list_id: i64,
        user_id: i64,
    },
    Role {
        list_id: i64,
        user_id: i64,
        role: String,
    },
    /// Generate a reusable invite link
    #[command(name = "invite-link")]
    InviteLink {
        list_id: i64,
    },
    /// Revoke public share link
    Unshare {
        list_id: i64,
    },
    Leave {
        list_id: i64,
    },
}

#[derive(Subcommand)]
pub enum KeyCmd {
    /// List your API keys
    #[command(alias = "ls")]
    List,
    /// Create a new API key
    Create {
        /// Human-readable name for the key
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
pub enum SecurityCmd {
    /// Security level, factors, and login-alert email preference
    Status,
    /// Confirm an unusual login (token from email or security notice)
    Ack {
        /// Signed ack token (or set KRAMLI_ACK_TOKEN)
        token: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum HandoffCmd {
    /// Report that this CLI is currently viewing a list
    Viewing {
        list_id: i64,
        #[arg(long)]
        list_name: Option<String>,
        /// Optional device label shown on other clients
        #[arg(long)]
        device: Option<String>,
    },
    /// Ask your other devices to continue with this list
    Continue {
        list_id: i64,
        #[arg(long)]
        list_name: Option<String>,
        /// Optional device label shown on other clients
        #[arg(long)]
        device: Option<String>,
    },
    /// Clear the current handoff viewing state
    Clear,
}

// ─── Dispatch ───

pub async fn run(cli: Cli) -> Result<(), String> {
    // Honour NO_COLOR convention (https://no-color.org/)
    if std::env::var("NO_COLOR").is_ok() || cli.json {
        set_override(false);
    }

    match cli.command {
        Commands::Login { url } => run_login(url).await,
        Commands::Logout => run_logout(),
        Commands::Status => run_status(cli.json).await,
        Commands::Lists { action } => run_lists(action, cli.json).await,
        Commands::Items { action } => run_items(action, cli.json).await,
        Commands::Folders { action } => run_folders(action, cli.json).await,
        Commands::Members { action } => run_members(action, cli.json).await,
        Commands::Keys { action } => run_keys(action, cli.json).await,
        Commands::Search { query } => run_search(&query, cli.json).await,
        Commands::Activity { list_id, limit } => run_activity(list_id, limit, cli.json).await,
        Commands::Undo { list_id } => run_undo(list_id).await,
        Commands::Redo { list_id } => run_redo(list_id).await,
        Commands::Handoff { action } => run_handoff(action, cli.json).await,
        Commands::Profile => run_profile(cli.json).await,
        Commands::Security { action } => run_security(action, cli.json).await,
        Commands::AcceptTerms { docs } => run_accept_terms(docs, cli.json).await,
        Commands::Ping => run_ping(cli.json).await,
        Commands::Config => run_config(cli.json),
        Commands::Mcp => crate::mcp::run_stdio().await,
        Commands::Batch { file, keep_going } => run_batch(&file, keep_going, cli.json).await,
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "kramli", &mut std::io::stdout());
            Ok(())
        }
    }
}

// ─── Login ───

async fn run_login(url: Option<String>) -> Result<(), String> {
    let mut cfg = Config::load();
    if let Some(ref u) = url {
        cfg.set_base_url(Some(u.clone()));
        cfg.save()?;
    }

    println!(
        "Generate an API key at {}/settings#api-keys",
        cfg.base_url().trim_end_matches('/')
    );

    let key: String = dialoguer::Password::new()
        .with_prompt("API Key")
        .interact()
        .map_err(|e| format!("Input error: {e}"))?;

    let key = key.trim().to_string();
    if !key.starts_with("kramli_") {
        return Err("Invalid API key format. Keys start with 'kramli_'.".into());
    }

    cfg.set_api_key(&key)?;
    cfg.save()?;

    let api = ApiClient::new(&cfg)?;
    match api.get::<Profile>("/profile").await {
        Ok(p) => {
            let name = p
                .display_name
                .as_deref()
                .unwrap_or(p.email.as_deref().unwrap_or("(unknown)"));
            println!("{} Logged in as {}", "✓".green(), name.bold());
            println!("  API key stored in system keychain.");
        }
        Err(e) => {
            cfg.delete_api_key()?;
            return Err(format!("Invalid API key: {e}"));
        }
    }
    Ok(())
}

fn run_logout() -> Result<(), String> {
    let cfg = Config::load();
    cfg.delete_api_key()?;
    println!("{} Logged out. API key removed from keychain.", "✓".green());
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
                out["profile"] = serde_json::to_value(&p).unwrap_or_default();
            }
        }
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return Ok(());
    }
    if cfg.has_api_key() {
        println!("Server:  {}", cfg.base_url());
        let src = if cfg.api_key_from_env() {
            "env KRAMLI_API_KEY"
        } else {
            "keychain"
        };
        println!("API Key: {} ({})", "stored".green(), src);
        let api = ApiClient::new(&cfg)?;
        match api.get::<Profile>("/profile").await {
            Ok(p) => {
                println!(
                    "Name:    {}",
                    p.display_name.as_deref().unwrap_or("(unknown)")
                );
                println!("Email:   {}", p.email.as_deref().unwrap_or("-"));
            }
            Err(e) => println!("{} {e}", "Profile unavailable:".yellow()),
        }
    } else {
        println!("Status:  {}", "not logged in".red());
        println!("  Run `kramli login` or set KRAMLI_API_KEY.");
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
        let parsed: Value =
            serde_json::from_str(trimmed).map_err(|e| format!("Invalid --states JSON: {e}"))?;
        if !parsed.is_array() {
            return Err("--states JSON must be an array.".into());
        }
        return Ok(parsed);
    }

    let mut states = Vec::new();
    for chunk in trimmed.split(',') {
        let part = chunk.trim();
        if part.is_empty() {
            continue;
        }
        let mut pieces = part.splitn(2, ':');
        let name = pieces.next().unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        let mut state = serde_json::Map::new();
        state.insert("name".into(), Value::String(name.to_string()));
        if let Some(color) = pieces.next().map(str::trim).filter(|c| !c.is_empty()) {
            state.insert("color".into(), Value::String(color.to_string()));
        }
        states.push(Value::Object(state));
    }

    if states.is_empty() {
        return Err("No valid states provided for --states.".into());
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

async fn fetch_item_from_list(
    api: &ApiClient,
    list_id: i64,
    item_id: i64,
) -> Result<ListItem, String> {
    let items: Vec<ListItem> = api.get(&format!("/lists/{list_id}/items")).await?;
    items
        .into_iter()
        .find(|item| item.id == item_id)
        .ok_or_else(|| format!("Item #{item_id} not found in list #{list_id}."))
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

fn auto_handoff_enabled() -> bool {
    env_flag_enabled("KRAMLI_AUTO_HANDOFF", true)
}

async fn maybe_auto_handoff(api: &ApiClient, list_id: i64, list_name: Option<&str>, as_json: bool) {
    if as_json || !auto_handoff_enabled() {
        return;
    }

    let mut body = serde_json::Map::new();
    body.insert("list_id".into(), Value::from(list_id));
    body.insert(
        "device_label".into(),
        Value::String(default_handoff_device_label(None)),
    );
    body.insert("integration".into(), Value::String("cli".to_string()));
    if let Some(name) = list_name.map(str::trim).filter(|name| !name.is_empty()) {
        body.insert("list_name".into(), Value::String(name.to_string()));
    }

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
                println!("{} list reference resolved", "✓".green());
                println!("  list_id: {id}");
                println!("  canonical: /lists/{id}");
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
        } => {
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
                println!("{} Created list #{}", "✓".green(), list.id);
                output::print_list_detail(&list);
            }
        }
        ListCmd::Update {
            id,
            name,
            icon,
            color,
            states,
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
            if let Some(states_raw) = states {
                body.insert("states".into(), parse_states_arg(&states_raw)?);
            }
            if body.is_empty() {
                return Err("No changes specified.".into());
            }
            let list: ShoppingList = api.put(&format!("/lists/{id}"), &body).await?;
            json_or!(as_json, list, {
                println!("{} List updated.", "✓".green());
                output::print_list_detail(&list);
            });
        }
        ListCmd::Delete { id } => {
            let resp: OkResponse = api.delete(&format!("/lists/{id}")).await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else {
                println!("{} List deleted.", "✓".green());
                if let Some(t) = resp.undo_token {
                    println!("  Undo token: {t}");
                }
            }
        }
        ListCmd::Move { id, folder_id } => {
            let body = json!({"folder_id": folder_id});
            let list: ShoppingList = api.put(&format!("/lists/{id}"), &body).await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&list).unwrap_or_default()
                );
            } else {
                match folder_id {
                    Some(fid) => println!("{} Moved list #{id} to folder #{fid}.", "✓".green()),
                    None => println!("{} Removed list #{id} from folder.", "✓".green()),
                }
            }
        }
    }
    Ok(())
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
        } => {
            let items: Vec<ListItem> = api.get(&format!("/lists/{list_id}/items")).await?;
            let state_filter = state
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty());
            let contains_filter = contains
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty());
            let filtered: Vec<ListItem> = items
                .into_iter()
                .filter(|item| {
                    let is_done = item.is_done.unwrap_or(false);
                    if open && is_done {
                        return false;
                    }
                    if completed && !is_done {
                        return false;
                    }

                    if let Some(state_value) = state_filter.as_deref() {
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

                    if let Some(query) = contains_filter.as_deref() {
                        let text = item.text.to_ascii_lowercase();
                        if !text.contains(query) {
                            return false;
                        }
                    }

                    true
                })
                .collect();

            json_or!(as_json, filtered, output::print_items(&filtered));
            maybe_auto_handoff(&api, list_id, None, as_json).await;
        }
        ItemCmd::Show { id } => {
            // Fetch item from its list (the items endpoint returns full data)
            let comments: Vec<crate::models::ItemComment> =
                api.get(&format!("/items/{id}/comments")).await?;
            let (item, _) = find_item_across_lists(&api, id)
                .await?
                .ok_or_else(|| format!("Item #{id} not found."))?;
            if as_json {
                let mut val = serde_json::to_value(&item).unwrap_or_default();
                val["comments"] = serde_json::to_value(&comments).unwrap_or_default();
                println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
            } else {
                output::print_item_detail(&item, &comments);
            }
            if let Some(list_id) = item.list_id {
                maybe_auto_handoff(&api, list_id, None, as_json).await;
            }
        }
        ItemCmd::Add {
            list_id,
            text,
            quantity,
            due,
            priority,
            tags,
            notes,
            parent,
            assign,
            color,
            progress,
        } => {
            let tag_vec = tags.map(|t| t.split(',').map(|s| s.trim().to_string()).collect());
            let body = CreateItem {
                text,
                quantity,
                notes,
                due_date: due,
                priority,
                tags: tag_vec,
                parent_item_id: parent,
            };
            // Build a Value so we can add extra fields (assign, color)
            let mut val = serde_json::to_value(&body).map_err(|e| e.to_string())?;
            if let Some(a) = assign {
                let ids: Vec<Value> = a
                    .split(',')
                    .filter_map(|s| s.trim().parse::<i64>().ok())
                    .map(Value::from)
                    .collect();
                val["assigned_to"] = Value::Array(ids);
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
                println!("{} Item created #{}", "✓".green(), item.id);
            }
        }
        ItemCmd::Update {
            id,
            text,
            quantity,
            due,
            priority,
            tags,
            notes,
            assign,
            color,
            progress,
        } => {
            let tags_provided = tags.is_some();
            let mut body = serde_json::Map::new();
            if let Some(t) = text {
                body.insert("text".into(), Value::String(t));
            }
            if let Some(q) = quantity {
                body.insert("quantity".into(), Value::String(q));
            }
            if let Some(d) = due {
                body.insert("due_date".into(), Value::String(d));
            }
            if let Some(p) = priority {
                body.insert("priority".into(), Value::String(p));
            }
            if let Some(n) = notes {
                body.insert("notes".into(), Value::String(n));
            }
            if let Some(c) = color {
                body.insert("color".into(), Value::String(c));
            }
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
                let ids: Vec<Value> = a
                    .split(',')
                    .filter_map(|s| s.trim().parse::<i64>().ok())
                    .map(Value::from)
                    .collect();
                body.insert("assigned_to".into(), Value::Array(ids));
            }
            if body.is_empty() {
                return Err("No changes specified.".into());
            }
            let mut item: ListItem = api.put(&format!("/items/{id}"), &body).await?;
            if !tags_provided {
                enrich_item_tags_from_list(&api, &mut item).await;
            }
            json_or!(as_json, item, println!("{} Item updated.", "✓".green()));
        }
        ItemCmd::Done { id } => {
            let mut resp: Value = api
                .patch_json(&format!("/items/{id}/done"), &json!({}))
                .await?;
            enrich_done_response_tags(&api, id, &mut resp).await;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else {
                println!("{} Toggled.", "✓".green());
            }
        }
        ItemCmd::Vote { id } => {
            let resp: Value = api
                .patch_json(&format!("/items/{id}/upvote"), &json!({}))
                .await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else {
                let upvoted_by_me = resp
                    .get("upvoted_by_me")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                let upvote_count = resp
                    .get("upvote_count")
                    .and_then(|value| value.as_i64())
                    .unwrap_or(0);
                if upvoted_by_me {
                    println!(
                        "{} Upvoted item #{} ({} votes).",
                        "✓".green(),
                        id,
                        upvote_count
                    );
                } else {
                    println!(
                        "{} Removed vote on item #{} ({} votes).",
                        "✓".green(),
                        id,
                        upvote_count
                    );
                }
            }
        }
        ItemCmd::Delete { id } => {
            let resp: OkResponse = api.delete(&format!("/items/{id}")).await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else {
                println!("{} Item deleted.", "✓".green());
                if let Some(t) = resp.undo_token {
                    println!("  Undo token: {t}");
                }
            }
        }
        ItemCmd::DoneList { list_id } => {
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
                println!("{}", "No completed items.".dimmed());
            } else {
                output::print_items(&done);
            }
            maybe_auto_handoff(&api, list_id, None, as_json).await;
        }
        ItemCmd::Comment { id, text } => {
            let resp: Value = api
                .post(&format!("/items/{id}/comments"), &json!({"text": text}))
                .await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else {
                println!("{} Comment added.", "✓".green());
            }
        }
        ItemCmd::CheckAll { list_id } => {
            let resp: Value = api
                .post(&format!("/lists/{list_id}/check-all"), &json!({}))
                .await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else {
                println!("{} All items marked done.", "✓".green());
            }
        }
        ItemCmd::ClearDone { list_id } => {
            let resp: Value = api
                .post(&format!("/lists/{list_id}/clear-done"), &json!({}))
                .await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else {
                let count = resp.get("count").and_then(|v| v.as_i64()).unwrap_or(0);
                println!("{} {count} completed items deleted.", "✓".green());
            }
        }
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
        FolderCmd::Create { name, icon, color } => {
            let body = CreateFolder { name, icon, color };
            let f: Folder = api.post("/folders", &body).await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&f).unwrap_or_default());
            } else {
                println!("{} Folder created #{}", "✓".green(), f.id);
            }
        }
        FolderCmd::Update {
            id,
            name,
            icon,
            color,
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
            if body.is_empty() {
                return Err("No changes specified.".into());
            }
            let f: Folder = api.put(&format!("/folders/{id}"), &body).await?;
            json_or!(as_json, f, println!("{} Folder updated.", "✓".green()));
        }
        FolderCmd::Delete { id } => {
            let _: OkResponse = api.delete(&format!("/folders/{id}")).await?;
            println!("{} Folder deleted.", "✓".green());
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
            println!("{} Invitation sent to {email}", "✓".green());
        }
        MemberCmd::Remove { list_id, user_id } => {
            api.delete_ok(&format!("/lists/{list_id}/members/{user_id}"))
                .await?;
            println!("{} Member removed.", "✓".green());
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
            println!("{} Role updated.", "✓".green());
        }
        MemberCmd::InviteLink { list_id } => {
            let resp: Value = api
                .post(&format!("/lists/{list_id}/invite-link"), &json!({}))
                .await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else if let Some(token) = resp.get("token").and_then(|v| v.as_str()) {
                println!("Invite link: https://kram.li/i/{token}");
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            }
        }
        MemberCmd::Unshare { list_id } => {
            api.delete_ok(&format!("/lists/{list_id}/share")).await?;
            println!("{} Public link removed.", "✓".green());
        }
        MemberCmd::Leave { list_id } => {
            let _: Value = api
                .post(&format!("/lists/{list_id}/leave"), &json!({}))
                .await?;
            println!("{} Left the list.", "✓".green());
        }
    }
    Ok(())
}

// ─── API Keys ───

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
                println!("{}", "No API keys.".dimmed());
            } else {
                for k in &keys {
                    let active = if k.is_active.unwrap_or(true) {
                        "active".green().to_string()
                    } else {
                        "revoked".red().to_string()
                    };
                    let scopes = k.scopes.as_deref().unwrap_or("all");
                    let label = k.name.as_deref().unwrap_or("(no name)");
                    let last = k.last_used_at.as_deref().unwrap_or("never");
                    println!(
                        "  #{:<4} {:<20} [{active}]  scopes: {scopes}  last used: {last}",
                        k.id,
                        label.bold(),
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
                    println!("{} API key created:", "✓".green());
                    println!();
                    println!("  {}", raw.bold());
                    println!();
                    println!(
                        "  {} Store this key securely. It will not be shown again.",
                        "!".yellow()
                    );
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
            println!("{} API key #{key_id} revoked.", "✓".green());
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
                .map_err(|e| format!("Could not parse search response: {e}"))?,
        )
    } else {
        None
    };
    let grouped = SearchResponse::from_value(raw)
        .map_err(|e| format!("Could not parse search response: {e}"))?;

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
        .post(&format!("/lists/{list_id}/undo"), &json!({}))
        .await?;
    println!("{} Undone.", "✓".green());
    Ok(())
}

async fn run_redo(list_id: i64) -> Result<(), String> {
    let api = get_api()?;
    let _: Value = api
        .post(&format!("/lists/{list_id}/redo"), &json!({}))
        .await?;
    println!("{} Redone.", "✓".green());
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
    if let Ok(value) = std::env::var("KRAMLI_DEVICE_LABEL") {
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
            let mut body = serde_json::Map::new();
            body.insert("list_id".into(), Value::from(list_id));
            body.insert(
                "device_label".into(),
                Value::String(default_handoff_device_label(device)),
            );
            body.insert("integration".into(), Value::String("cli".to_string()));
            if let Some(name) = normalize_optional_text(list_name) {
                body.insert("list_name".into(), Value::String(name));
            }
            let resp: Value = api.post("/activity/viewing", &body).await?;
            json_or!(
                as_json,
                resp,
                println!("{} Handoff viewing sent.", "✓".green())
            );
        }
        HandoffCmd::Continue {
            list_id,
            list_name,
            device,
        } => {
            let mut body = serde_json::Map::new();
            body.insert("list_id".into(), Value::from(list_id));
            body.insert(
                "device_label".into(),
                Value::String(default_handoff_device_label(device)),
            );
            body.insert("integration".into(), Value::String("cli".to_string()));
            if let Some(name) = normalize_optional_text(list_name) {
                body.insert("list_name".into(), Value::String(name));
            }
            let resp: Value = api.post("/activity/continue-on-device", &body).await?;
            json_or!(
                as_json,
                resp,
                println!("{} Continue-on-device handoff sent.", "✓".green())
            );
        }
        HandoffCmd::Clear => {
            let resp: Value = api.post("/activity/clear", &json!({})).await?;
            json_or!(as_json, resp, println!("{} Handoff cleared.", "✓".green()));
        }
    }

    Ok(())
}

async fn run_batch(file: &str, keep_going: bool, as_json: bool) -> Result<(), String> {
    if as_json {
        return Err("`kramli batch` does not support --json yet.".into());
    }

    let source = if file == "-" {
        "stdin".to_string()
    } else {
        file.to_string()
    };

    let content = if file == "-" {
        use tokio::io::AsyncReadExt;
        let mut buffer = String::new();
        tokio::io::stdin()
            .read_to_string(&mut buffer)
            .await
            .map_err(|e| format!("Could not read batch commands from stdin: {e}"))?;
        buffer
    } else {
        fs::read_to_string(file)
            .map_err(|e| format!("Could not read batch file '{}': {e}", file))?
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
                let err = format!("Batch parse error in {source}:{line_no}: {e}");
                failed += 1;
                if first_error.is_none() {
                    first_error = Some(err.clone());
                }
                eprintln!("{} {err}", "✗".red());
                if !keep_going {
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
                let err = format!("Batch parse error in {source}:{line_no}: {e}");
                failed += 1;
                if first_error.is_none() {
                    first_error = Some(err.clone());
                }
                eprintln!("{} {err}", "✗".red());
                if !keep_going {
                    break;
                }
                continue;
            }
        };

        if matches!(nested_cli.command, Commands::Batch { .. }) {
            let err = format!("Nested `batch` command is not supported ({source}:{line_no}).");
            failed += 1;
            if first_error.is_none() {
                first_error = Some(err.clone());
            }
            eprintln!("{} {err}", "✗".red());
            if !keep_going {
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
            "{} Batch finished: {succeeded} command(s) succeeded.",
            "✓".green()
        );
        return Ok(());
    }

    println!(
        "{} Batch finished: {succeeded} succeeded, {failed} failed.",
        "!".yellow()
    );

    Err(first_error.unwrap_or_else(|| "Batch failed.".to_string()))
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
            println!("Security level: {}", level.bold());
            println!("Score:          {score}/{max_score}");
            let login_alerts = data
                .get("security_email_login_alerts")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            println!(
                "Login emails:   {}",
                if login_alerts { "on" } else { "off" }
            );
            if let Some(factors) = summary.get("factors").and_then(Value::as_array) {
                println!("\nFactors:");
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
                .or_else(|| std::env::var("KRAMLI_ACK_TOKEN").ok());
            let Some(token) = token else {
                return Err(
                    "Missing ack token: pass as argument or set KRAMLI_ACK_TOKEN".to_string(),
                );
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
                let message = data
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Login confirmed.");
                println!("{} {}", "✓".green(), message);
                Ok(())
            } else {
                let message = data
                    .get("error")
                    .or_else(|| data.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Ack failed.");
                Err(message.to_string())
            }
        }
    }
}

async fn run_profile(as_json: bool) -> Result<(), String> {
    let api = get_api()?;
    let p: Profile = api.get("/profile").await?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&p).unwrap_or_default());
        return Ok(());
    }
    println!(
        "Name:   {}",
        p.display_name.as_deref().unwrap_or("(unknown)").bold()
    );
    println!("Email:  {}", p.email.as_deref().unwrap_or("-"));
    if let Some(id) = p.id {
        println!("ID:     {id}");
    }
    if p.is_anonymous.unwrap_or(false) {
        println!("        (guest account)");
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
                return Err(format!(
                    "Invalid doc key '{value}'. Use --docs agb,privacy (or omit to accept all pending)."
                ));
            }
        }
    }

    let body = if let Some(values) = normalized_docs {
        if values.is_empty() {
            json!({})
        } else {
            json!({ "docs": values })
        }
    } else {
        json!({})
    };

    let resp: Value = api.post("/accept-terms", &body).await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&resp).unwrap_or_default()
        );
        return Ok(());
    }

    let pending_count = resp
        .get("legal")
        .and_then(|legal| legal.get("pending"))
        .and_then(Value::as_array)
        .map(|pending| pending.len())
        .unwrap_or(0);

    if pending_count == 0 {
        println!("{} Terms and privacy accepted.", "✓".green());
    } else {
        println!(
            "{} Updated legal acceptance, but {pending_count} document(s) are still pending.",
            "✓".green()
        );
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
        .map_err(|e| format!("Network error: {e}"))?;
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
        println!("{} Server reachable ({ms}ms)", "✓".green());
    } else {
        println!("{} Server returned {}", "✗".red(), resp.status());
    }
    Ok(())
}

fn run_config(as_json: bool) -> Result<(), String> {
    let cfg = Config::load();
    if as_json {
        let out = json!({
            "config_path": Config::path().display().to_string(),
            "server": cfg.base_url(),
            "api_key_stored": cfg.has_api_key(),
            "api_key_source": if cfg.api_key_from_env() { "env" } else { "keychain" },
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return Ok(());
    }
    println!("Config:  {}", Config::path().display());
    println!("Server:  {}", cfg.base_url());
    let src = if cfg.api_key_from_env() {
        " (env)"
    } else {
        " (keychain)"
    };
    println!(
        "API Key: {}{src}",
        if cfg.has_api_key() {
            "stored".green().to_string()
        } else {
            "not set".red().to_string()
        }
    );
    Ok(())
}
