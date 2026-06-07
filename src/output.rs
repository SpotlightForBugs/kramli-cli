use colored::Colorize;

use crate::i18n::{tr, tr_args};
use crate::models::{
    ActivityEntry, Folder, ItemComment, ListItem, Member, SearchResults, ShoppingList,
};

#[derive(Copy, Clone)]
enum IconStyle {
    Label,
    Emoji,
    Raw,
}

fn icon_style() -> IconStyle {
    match std::env::var("KRAMLI_ICON_STYLE")
        .ok()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "emoji" => IconStyle::Emoji,
        "raw" => IconStyle::Raw,
        _ => IconStyle::Label,
    }
}

fn parse_hex_color(input: &str) -> Option<(u8, u8, u8)> {
    let hex = input.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

fn map_bootstrap_icon_emoji(icon: &str, fallback: &str) -> String {
    match icon {
        "bi-cart-fill" => "🛒".to_string(),
        "bi-egg-fried" => "🍳".to_string(),
        "bi-people-fill" => "👥".to_string(),
        "bi-tag" => "🏷️".to_string(),
        "bi-tools" => "🛠️".to_string(),
        "bi-paperclip" => "📎".to_string(),
        "bi-book-fill" => "📚".to_string(),
        "bi-check-circle-fill" => "✅".to_string(),
        "bi-fire" => "🔥".to_string(),
        "bi-cup-hot" => "☕".to_string(),
        "bi-folder2" => "📁".to_string(),
        _ => fallback.to_string(),
    }
}

fn map_bootstrap_icon_label(icon: &str, fallback_name: &str) -> String {
    let label = match icon {
        "bi-cart-fill" => "cart",
        "bi-egg-fried" => "food",
        "bi-people-fill" => "team",
        "bi-tag" => "tag",
        "bi-tools" => "tools",
        "bi-paperclip" => "clip",
        "bi-book-fill" => "book",
        "bi-check-circle-fill" => "done",
        "bi-fire" => "fire",
        "bi-cup-hot" => "coffee",
        "bi-folder2" => "folder",
        _ => {
            let stripped = icon.strip_prefix("bi-").unwrap_or(icon);
            if stripped.is_empty() {
                fallback_name
            } else {
                stripped
            }
        }
    };
    format!("[{label}]")
}

fn fallback_icon(style: IconStyle, fallback_name: &str) -> String {
    match style {
        IconStyle::Emoji => match fallback_name {
            "folder" => "📁".to_string(),
            _ => "📋".to_string(),
        },
        IconStyle::Raw | IconStyle::Label => format!("[{fallback_name}]"),
    }
}

fn display_icon(raw: Option<&str>, fallback_name: &str) -> String {
    let style = icon_style();
    let Some(icon) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return fallback_icon(style, fallback_name);
    };

    if icon.starts_with("bi-") {
        return match style {
            IconStyle::Emoji => {
                map_bootstrap_icon_emoji(icon, &fallback_icon(style, fallback_name))
            }
            IconStyle::Raw => icon.to_string(),
            IconStyle::Label => map_bootstrap_icon_label(icon, fallback_name),
        };
    }

    icon.to_string()
}

fn color_dot(raw: Option<&str>) -> String {
    let Some(color) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::new();
    };

    match parse_hex_color(color) {
        Some((r, g, b)) => format!(" {}", "●".truecolor(r, g, b)),
        None => String::new(),
    }
}

fn colorize_text(raw: Option<&str>, text: &str) -> String {
    match raw.and_then(parse_hex_color) {
        Some((r, g, b)) => text.truecolor(r, g, b).to_string(),
        None => text.to_string(),
    }
}

fn colorize_bold_text(raw: Option<&str>, text: &str) -> String {
    match raw.and_then(parse_hex_color) {
        Some((r, g, b)) => text.bold().truecolor(r, g, b).to_string(),
        None => text.bold().to_string(),
    }
}

fn role_label(raw: &str) -> String {
    match raw {
        "owner" => tr("role-owner"),
        "admin" => tr("role-admin"),
        "editor" => tr("role-editor"),
        "viewer" => tr("role-viewer"),
        _ => raw.to_string(),
    }
}

fn view_mode_label(raw: &str) -> String {
    match raw {
        "list" => tr("view-list"),
        "board" => tr("view-board"),
        "calendar" => tr("view-calendar"),
        "timeline" => tr("view-timeline"),
        _ => raw.to_string(),
    }
}

fn member_type_label(raw: &str) -> String {
    match raw {
        "member" => tr("member-type-member"),
        "invite" => tr("member-type-invite"),
        _ => raw.to_string(),
    }
}

fn activity_action_label(raw: &str) -> String {
    match raw {
        "item_created" => tr("activity-item-created"),
        "item_updated" => tr("activity-item-updated"),
        "item_deleted" => tr("activity-item-deleted"),
        "item_done_toggled" => tr("activity-item-toggled"),
        "item_commented" => tr("activity-item-commented"),
        "list_updated" => tr("activity-list-updated"),
        "member_invited" => tr("activity-member-invited"),
        "member_removed" => tr("activity-member-removed"),
        _ => raw.to_string(),
    }
}

// ── Lists ──

pub fn print_lists(lists: &[ShoppingList]) {
    if lists.is_empty() {
        println!("{}", tr("output-no-lists").dimmed());
        return;
    }
    for l in lists {
        let icon = colorize_text(l.color.as_deref(), &display_icon(l.icon.as_deref(), "list"));
        let name = colorize_bold_text(l.color.as_deref(), &l.name);
        let done = l.done_count.unwrap_or(0);
        let total = l.item_count.unwrap_or(0);
        let role = l.role.as_deref().unwrap_or("");
        let role_badge = if role == "owner" {
            "".to_string()
        } else {
            format!(" ({})", role_label(role).dimmed())
        };
        let color_badge = color_dot(l.color.as_deref());
        println!(
            "  {icon} {:<36} {done:>3}/{total:<3}  #{}{role_badge}{color_badge}",
            name, l.id,
        );
    }
}

pub fn print_list_detail(l: &ShoppingList) {
    let icon = colorize_text(l.color.as_deref(), &display_icon(l.icon.as_deref(), "list"));
    let name = colorize_bold_text(l.color.as_deref(), &l.name);
    println!("{icon}  {name} (#{}) ", l.id);
    if let Some(ref c) = l.color {
        let dot = color_dot(Some(c));
        println!("   {}:     {c}{dot}", tr("label-color"));
    }
    if let Some(ref vm) = l.view_mode {
        println!("   {}:   {}", tr("label-view"), view_mode_label(vm));
    }
    if let Some(ref r) = l.role {
        println!("   {}:     {}", tr("label-role"), role_label(r));
    }
    if let Some(ref states) = l.states {
        let names: Vec<String> = states
            .iter()
            .filter_map(|s| {
                let name = s.name.as_deref()?.trim();
                if name.is_empty() {
                    return None;
                }
                if s.is_done.unwrap_or(false) {
                    Some(format!("{}{}", name, "✓".green()))
                } else {
                    Some(name.to_string())
                }
            })
            .collect();
        if !names.is_empty() {
            println!("   {}:    {}", tr("label-states"), names.join(" | "));
        }
    }
    if let Some(fid) = l.folder_id {
        println!("   {}: {fid}", tr("label-folder-id"));
    }
    let done = l.done_count.unwrap_or(0);
    let total = l.item_count.unwrap_or(0);
    println!(
        "   {}:  {}",
        tr("label-items"),
        tr_args(
            "items-done-ratio",
            &[("done", done.to_string()), ("total", total.to_string())],
        )
    );
}

// ── Items ──

/// Strip HTML tags from a string (best-effort, no full parser needed).
fn strip_html(s: &str) -> String {
    let br = s
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");
    let mut out = String::with_capacity(br.len());
    let mut in_tag = false;
    for ch in br.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn human_size(bytes: i64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut val = bytes as f64;
    for unit in &["KB", "MB", "GB"] {
        val /= 1024.0;
        if val < 1024.0 {
            return format!("{val:.1} {unit}");
        }
    }
    format!("{val:.1} TB")
}

pub fn print_item_detail(item: &ListItem, comments: &[ItemComment]) {
    let check = if item.is_done.unwrap_or(false) {
        "✓".green().to_string()
    } else {
        "○".to_string()
    };
    let pri = match item.priority.as_deref() {
        Some("high") => " !!!".red().to_string(),
        Some("medium") => " !!".yellow().to_string(),
        Some("low") => " !".dimmed().to_string(),
        _ => String::new(),
    };
    println!(
        "{check} {}{pri}  {}",
        item.text.bold(),
        format!("(#{})", item.id).dimmed()
    );

    if let Some(ref q) = item.quantity {
        if !q.is_empty() {
            println!("  {}  {q}", tr("label-quantity").dimmed());
        }
    }
    if let Some(ref d) = item.due_date {
        println!("  {}  {}", tr("label-due").dimmed(), d.yellow());
    }
    if let Some(ref d) = item.planned_date {
        println!("  {}  {d}", tr("label-planned").dimmed());
    }
    if let Some(ref r) = item.repeat_label {
        println!("  {}  {r}", tr("label-repeat").dimmed());
    }
    if let Some(ref p) = item.progress {
        println!("  {}  {p}", tr("label-state").dimmed());
    }
    if let Some(ref c) = item.color {
        let dot = color_dot(Some(c));
        println!("  {}  {c}{dot}", tr("label-color").dimmed());
    }
    if let Some(ref tags) = item.tags {
        if !tags.is_empty() {
            let t: Vec<String> = tags
                .iter()
                .map(|t| format!("#{t}").cyan().to_string())
                .collect();
            println!("  {}  {}", tr("label-tags").dimmed(), t.join("  "));
        }
    }
    if let Some(ref assigned) = item.assigned_to {
        if !assigned.is_empty() {
            let ids: Vec<String> = assigned.iter().map(|id| id.to_string()).collect();
            println!("  {}  {}", tr("label-assigned").dimmed(), ids.join(", "));
        }
    }
    if let Some(ref notes) = item.notes {
        let plain = strip_html(notes);
        if !plain.trim().is_empty() {
            println!("  {}", tr("label-notes").magenta());
            for line in plain.lines() {
                println!("    {line}");
            }
        }
    }
    if let Some(ref tldr) = item.tldr {
        if !tldr.is_empty() {
            println!("  {}  {tldr}", tr("label-tldr").blue());
        }
    }
    if let Some(ref url) = item.image_url {
        println!("  {}  {url}", tr("label-image").dimmed());
    }
    if let Some(ref atts) = item.attachments {
        if !atts.is_empty() {
            println!("  {} ({})", tr("label-attachments").dimmed(), atts.len());
            for a in atts {
                let name = a
                    .original_filename
                    .as_deref()
                    .or(a.filename.as_deref())
                    .unwrap_or("?");
                let size = a.file_size.map(human_size).unwrap_or_default();
                let url = a.url.as_deref().unwrap_or("");
                println!("    • {name}  {size}  {}", url.dimmed());
            }
        }
    }
    if let Some(ref created) = item.created_at {
        let display = &created[..created.len().min(16)];
        println!("  {}  {display}", tr("label-created").dimmed());
    }
    if let Some(ref updated) = item.updated_at {
        let display = &updated[..updated.len().min(16)];
        println!("  {}  {display}", tr("label-updated").dimmed());
    }
    // Comments
    if !comments.is_empty() {
        println!();
        println!("  {} ({})", tr("label-comments").bold(), comments.len());
        for c in comments {
            let who = c
                .user_name
                .as_deref()
                .or(c.user_email.as_deref())
                .unwrap_or("?");
            let text = c.text.as_deref().unwrap_or("");
            let when = c
                .created_at
                .as_deref()
                .map(|s| &s[..s.len().min(16)])
                .unwrap_or("");
            println!("    {}  {}  {text}", when.dimmed(), who.bold());
        }
    }
    println!();
}

pub fn print_items(items: &[ListItem]) {
    if items.is_empty() {
        println!("{}", tr("output-no-items").dimmed());
        return;
    }
    for i in items {
        let check = if i.is_done.unwrap_or(false) {
            "✓".green().to_string()
        } else {
            "○".dimmed().to_string()
        };
        let indent = "  ".repeat(i.depth.unwrap_or(0) as usize);
        let pri = match i.priority.as_deref() {
            Some("high") => " !!!".red().to_string(),
            Some("medium") => " !!".yellow().to_string(),
            Some("low") => " !".dimmed().to_string(),
            _ => String::new(),
        };
        let qty = match &i.quantity {
            Some(q) if !q.is_empty() => format!(" ({})", q.dimmed()),
            _ => String::new(),
        };
        let due = match &i.due_date {
            Some(d) => format!("  {}: {}", tr("label-due-lower"), d.yellow()),
            None => String::new(),
        };
        let tags_str = match &i.tags {
            Some(t) if !t.is_empty() => {
                let joined: Vec<String> = t.iter().map(|tg| format!("#{tg}")).collect();
                format!("  {}", joined.join(" ").dimmed())
            }
            _ => String::new(),
        };
        let progress = match i.progress.as_deref().map(str::trim) {
            Some(p) if !p.is_empty() => format!("  [{}]", p.cyan()),
            _ => String::new(),
        };
        let sub = match (i.child_count, i.done_child_count) {
            (Some(c), Some(d)) if c > 0 => format!(" [{d}/{c}]").dimmed().to_string(),
            _ => String::new(),
        };
        let text = if i.is_done.unwrap_or(false) {
            i.text.dimmed().strikethrough().to_string()
        } else {
            colorize_text(i.color.as_deref(), &i.text)
        };
        println!(
            "  {indent}{check} {text}{pri}{qty}{sub}{progress}{due}{tags_str}  (#{id})",
            id = i.id,
        );
    }
}

// ── Folders ──

pub fn print_folders(folders: &[Folder]) {
    if folders.is_empty() {
        println!("{}", tr("output-no-folders").dimmed());
        return;
    }
    for f in folders {
        let icon = colorize_text(
            f.color.as_deref(),
            &display_icon(f.icon.as_deref(), "folder"),
        );
        let name = colorize_bold_text(f.color.as_deref(), &f.name);
        let color_badge = color_dot(f.color.as_deref());
        println!("  {icon} {:<36} #{}{color_badge}", name, f.id);
    }
}

// ── Members ──

pub fn print_members(members: &[Member]) {
    if members.is_empty() {
        println!("{}", tr("output-no-members").dimmed());
        return;
    }
    for m in members {
        let name = m
            .display_name
            .clone()
            .unwrap_or_else(|| tr("common-unknown"));
        let email = m.email.as_deref().unwrap_or("");
        let role = m.role.as_deref().unwrap_or("?");
        let kind = m.member_type.as_deref().unwrap_or("member");
        let badge = if kind == "invite" {
            format!(" ({})", tr("member-invited")).yellow().to_string()
        } else {
            String::new()
        };
        println!(
            "  {name:<24} {email:<30} [{} | {}]{badge}",
            role_label(role),
            member_type_label(kind)
        );
    }
}

// ── Search ──

pub fn print_search(results: &SearchResults) {
    let mut any = false;
    if let Some(ref lists) = results.lists {
        if !lists.is_empty() {
            any = true;
            println!("{}", tr("label-lists").bold().underline());
            for l in lists {
                let icon =
                    colorize_text(l.color.as_deref(), &display_icon(l.icon.as_deref(), "list"));
                let name = colorize_text(l.color.as_deref(), &l.name);
                let color_badge = color_dot(l.color.as_deref());
                println!("  {icon} {name} (#{}){color_badge} ", l.id);
            }
        }
    }
    if let Some(ref items) = results.items {
        if !items.is_empty() {
            any = true;
            println!("{}", tr("label-items").bold().underline());
            for i in items {
                let check = if i.is_done.unwrap_or(false) {
                    "✓"
                } else {
                    "○"
                };
                let ln = i.list_name.as_deref().unwrap_or("?");
                println!(
                    "  {check} {} (#{}) {}",
                    i.text,
                    i.id,
                    tr_args("search-in-list", &[("list", ln.to_string())])
                );
            }
        }
    }
    if !any {
        println!("{}", tr("output-no-results").dimmed());
    }
}

// ── Activity ──

fn activity_detail_text(detail: Option<&serde_json::Value>) -> String {
    let Some(detail) = detail else {
        return String::new();
    };
    match detail {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(serde_json::Value::as_str) {
                return format!("{}: {text}", tr("label-text"));
            }
            if let Some(changes) = map.get("changes").and_then(serde_json::Value::as_array) {
                let entries: Vec<String> = changes
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect();
                if !entries.is_empty() {
                    return format!("{}: {}", tr("label-changes"), entries.join("; "));
                }
            }

            let pairs: Vec<String> = map
                .iter()
                .map(|(key, value)| {
                    let rendered = value
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string());
                    format!("{key}={rendered}")
                })
                .collect();
            if pairs.is_empty() {
                String::new()
            } else {
                pairs.join(", ")
            }
        }
        _ => detail.to_string(),
    }
}

pub fn print_activity(entries: &[ActivityEntry]) {
    if entries.is_empty() {
        println!("{}", tr("output-no-activity").dimmed());
        return;
    }
    for a in entries {
        let who = a
            .display_name
            .as_deref()
            .or(a.user_name.as_deref())
            .unwrap_or("?");
        let action = a
            .action
            .as_deref()
            .map(activity_action_label)
            .unwrap_or_else(|| "?".to_string());
        let detail = activity_detail_text(a.detail.as_ref());
        let when = a.created_at.as_deref().unwrap_or("");
        println!(
            "  {} {} {}  {}",
            who.bold(),
            action,
            detail.dimmed(),
            when.dimmed()
        );
    }
}
