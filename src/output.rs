use colored::Colorize;
use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;

use crate::i18n::{tr, tr_args};
use crate::models::{
    ActivityEntry, Folder, ItemComment, ListItem, Member, SearchResults, ShoppingList,
};

const KRAMLI_ICON_STYLE_ENV: &str = "KRAMLI_ICON_STYLE";

#[derive(Copy, Clone)]
enum IconStyle {
    Label,
    Emoji,
    Raw,
}

fn icon_style() -> IconStyle {
    match std::env::var(KRAMLI_ICON_STYLE_ENV)
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

fn bootstrap_icon_asset_name(raw_icon: &str) -> Option<String> {
    let raw_icon = raw_icon.trim();
    if raw_icon.is_empty() || raw_icon.contains("..") {
        return None;
    }

    for (index, _) in raw_icon.match_indices("bi-") {
        let candidate: String = raw_icon[index + 3..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
            .collect();
        if let Some(icon) = normalize_bootstrap_icon_name(&candidate.replace('_', "-")) {
            return Some(icon);
        }
    }

    let candidate = raw_icon
        .trim_matches(|ch: char| ch == '"' || ch == '\'' || ch == '`' || ch == '[' || ch == ']')
        .trim_end_matches(".svg")
        .rsplit(['/', '#', '?'])
        .next()
        .unwrap_or(raw_icon)
        .trim()
        .trim_start_matches("bootstrap-icons:")
        .trim_start_matches("bootstrap-icon:")
        .trim_start_matches("bi:")
        .trim_start_matches("bi_");
    let candidate = candidate
        .strip_prefix("bi-")
        .unwrap_or(candidate)
        .replace('_', "-")
        .to_ascii_lowercase();

    normalize_bootstrap_icon_name(&candidate)
}

fn normalize_bootstrap_icon_name(candidate: &str) -> Option<String> {
    let icon = candidate.trim();
    (!icon.is_empty()
        && icon.len() <= 80
        && icon
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'))
    .then(|| icon.to_string())
}

fn map_bootstrap_icon_emoji(icon: &str, fallback: &str) -> String {
    match icon {
        "cart-fill" => "🛒".to_string(),
        "egg-fried" => "🍳".to_string(),
        "people-fill" => "👥".to_string(),
        "tag" => "🏷️".to_string(),
        "tools" => "🛠️".to_string(),
        "paperclip" => "📎".to_string(),
        "book-fill" => "📚".to_string(),
        "check-circle-fill" => "✅".to_string(),
        "fire" => "🔥".to_string(),
        "cup-hot" => "☕".to_string(),
        "folder2" | "folder2-open" => "📁".to_string(),
        _ => fallback.to_string(),
    }
}

fn map_bootstrap_icon_label(icon: &str, fallback_name: &str) -> String {
    let label = match icon {
        "cart-fill" => "cart",
        "egg-fried" => "food",
        "people-fill" => "team",
        "tag" => "tag",
        "tools" => "tools",
        "paperclip" => "clip",
        "book-fill" => "book",
        "check-circle-fill" => "done",
        "fire" => "fire",
        "cup-hot" => "coffee",
        "folder2" | "folder2-open" => "folder",
        _ if !icon.is_empty() => icon,
        _ => fallback_name,
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

    if let Some(asset) = bootstrap_icon_asset_name(icon) {
        return match style {
            IconStyle::Emoji => {
                map_bootstrap_icon_emoji(&asset, &fallback_icon(style, fallback_name))
            }
            IconStyle::Raw => format!("bi-{asset}"),
            IconStyle::Label => map_bootstrap_icon_label(&asset, fallback_name),
        };
    }

    icon.to_string()
}

fn color_dot(raw: Option<&str>) -> String {
    let Some(color) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::default();
    };

    match parse_hex_color(color) {
        Some((r, g, b)) => format!(" {}", "●".truecolor(r, g, b)),
        None => String::default(),
    }
}

fn char_display_width(ch: char) -> usize {
    match ch {
        '\t' => 4,
        _ if ch.is_control() => 0,
        _ => 1,
    }
}

fn visible_width_ansi(input: &str) -> usize {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut width = 0;

    while i < bytes.len() {
        if bytes[i] == 0x1b {
            i += 1;
            if i < bytes.len() && bytes[i] == b'[' {
                i += 1;
                while i < bytes.len() {
                    let b = bytes[i];
                    i += 1;
                    if (0x40..=0x7e).contains(&b) {
                        break;
                    }
                }
            }
            continue;
        }

        let Some(ch) = input[i..].chars().next() else {
            break;
        };
        i += ch.len_utf8();
        width += char_display_width(ch);
    }

    width
}

fn wrap_ansi_with_prefix(
    content: &str,
    first_prefix: &str,
    next_prefix: &str,
    width: usize,
) -> String {
    let next_prefix_width = visible_width_ansi(next_prefix);
    if width <= next_prefix_width + 4 {
        return format!("{first_prefix}{content}");
    }

    let bytes = content.as_bytes();
    let mut i = 0;
    let mut col = visible_width_ansi(first_prefix);
    let mut line = first_prefix.to_string();
    let mut lines = Vec::new();

    while i < bytes.len() {
        if bytes[i] == 0x1b {
            let start = i;
            i += 1;
            if i < bytes.len() && bytes[i] == b'[' {
                i += 1;
                while i < bytes.len() {
                    let b = bytes[i];
                    i += 1;
                    if (0x40..=0x7e).contains(&b) {
                        break;
                    }
                }
            }
            line.push_str(&content[start..i]);
            continue;
        }

        let Some(ch) = content[i..].chars().next() else {
            break;
        };
        i += ch.len_utf8();

        if ch == '\n' {
            lines.push(line);
            line = next_prefix.to_string();
            col = next_prefix_width;
            continue;
        }

        let ch_width = char_display_width(ch);
        if col + ch_width > width && col > next_prefix_width {
            lines.push(line);
            line = next_prefix.to_string();
            col = next_prefix_width;
            if ch == ' ' {
                continue;
            }
        }

        line.push(ch);
        col += ch_width;
    }

    lines.push(line);
    lines.join("\n")
}

fn print_wrapped_item_line(first_prefix: &str, next_prefix: &str, content: &str) {
    if !std::io::stdout().is_terminal() {
        println!("{first_prefix}{content}");
        return;
    }

    let width = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(120);
    println!(
        "{}",
        wrap_ansi_with_prefix(content, first_prefix, next_prefix, width)
    );
}

fn list_display_name_with_folder(list: &ShoppingList) -> String {
    let name = list.name.trim();
    let folder = list
        .folder_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match (folder, name.is_empty()) {
        (Some(folder_name), false) => format!("{folder_name} / {name}"),
        (Some(folder_name), true) => folder_name.to_string(),
        (None, false) => list.name.clone(),
        (None, true) => tr("common-unknown"),
    }
}

fn list_folder_parts(list: &ShoppingList) -> Vec<String> {
    if let Some(folder_name) = list
        .folder_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return folder_name
            .split('/')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();
    }

    list.folder_id
        .map(|id| vec![format!("#{}", id)])
        .unwrap_or_default()
}

fn folder_path_parts(folder: &Folder) -> Vec<String> {
    let mut parts = folder
        .parent_folder_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .split('/')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    parts.push(folder.name.clone());
    parts
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

/// Print a grouped, human-readable list overview.
pub(crate) fn print_lists(lists: &[ShoppingList]) {
    if lists.is_empty() {
        println!("{}", tr("output-no-lists").dimmed());
        return;
    }
    let mut lists: Vec<&ShoppingList> = lists.iter().collect();
    lists.sort_by(|a, b| {
        list_folder_parts(a)
            .join("/")
            .to_ascii_lowercase()
            .cmp(&list_folder_parts(b).join("/").to_ascii_lowercase())
            .then_with(|| {
                a.name
                    .to_ascii_lowercase()
                    .cmp(&b.name.to_ascii_lowercase())
            })
            .then_with(|| a.id.cmp(&b.id))
    });

    let folder_icon = display_icon(Some("bi-folder2"), "folder")
        .dimmed()
        .to_string();
    let mut current_folder: Vec<String> = Vec::new();
    for l in lists {
        let folder = list_folder_parts(l);
        let common = current_folder
            .iter()
            .zip(folder.iter())
            .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
            .count();
        for (depth, part) in folder.iter().enumerate().skip(common) {
            let indent = "  ".repeat(depth);
            println!("  {indent}{folder_icon} {}", part.bold());
        }
        current_folder = folder;

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
        let indent = "  ".repeat(current_folder.len());
        println!(
            "  {indent}{icon} {:<36} {done:>3}/{total:<3}  #{}{role_badge}{color_badge}",
            name, l.id,
        );
    }
}

/// Print detailed information for a single list.
pub(crate) fn print_list_detail(l: &ShoppingList) {
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

/// Print note content for a note list detail view.
pub(crate) fn print_note_content(note_content: Option<&str>) {
    let Some(note_content) = note_content
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    println!("   {}:", tr("label-notes"));
    for line in note_content.lines() {
        println!("     {line}");
    }
}

/// Print a note-list document in the item-list view.
pub(crate) fn print_note_for_list(list: Option<&ShoppingList>, note_content: Option<&str>) {
    if let Some(list) = list {
        println!(
            "{}: {}",
            tr("label-items"),
            list_display_name_with_folder(list)
        );
    }

    let note_content = note_content.unwrap_or("").trim();
    if note_content.is_empty() {
        println!("{}", tr("output-no-items").dimmed());
        return;
    }

    println!("  {}", tr("label-notes").magenta());
    for line in note_content.lines() {
        println!("    {line}");
    }
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

fn date_with_time(date: &str, time: Option<&String>) -> String {
    match time
        .map(String::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        Some(time) => format!("{date} {time}"),
        None => date.to_string(),
    }
}

fn reminder_offsets_label(offsets: &[i64]) -> String {
    offsets
        .iter()
        .map(|offset| {
            if *offset >= 1440 && offset % 1440 == 0 {
                format!("{}d", offset / 1440)
            } else if *offset >= 60 && offset % 60 == 0 {
                format!("{}h", offset / 60)
            } else {
                format!("{offset}m")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

struct ScheduleLine {
    label_key: &'static str,
    value: String,
}

struct ItemStatusParts {
    check: String,
    priority: String,
}

struct ItemComments<'a> {
    comments: &'a [ItemComment],
}

impl<'a> ItemComments<'a> {
    fn print(self) {
        if self.comments.is_empty() {
            return;
        }

        println!();
        println!(
            "  {} ({})",
            tr("label-comments").bold(),
            self.comments.len()
        );
        for comment in self.comments {
            let who = comment
                .user_name
                .as_deref()
                .or(comment.user_email.as_deref())
                .unwrap_or("?");
            let text = comment.text.as_deref().unwrap_or("");
            let when = comment
                .created_at
                .as_deref()
                .map_or("", |s| &s[..s.len().min(16)]);
            println!("    {}  {}  {text}", when.dimmed(), who.bold());
        }
    }
}

fn schedule_lines(item: &ListItem) -> Vec<ScheduleLine> {
    let mut lines = Vec::new();

    if let Some(ref date) = item.due_date {
        lines.push(ScheduleLine {
            label_key: "label-due",
            value: date_with_time(date, item.due_time.as_ref())
                .yellow()
                .to_string(),
        });
    }
    if let Some(ref date) = item.planned_date {
        lines.push(ScheduleLine {
            label_key: "label-planned",
            value: date_with_time(date, item.planned_time.as_ref()),
        });
    }
    if let Some(ref repeat) = item.repeat_label {
        lines.push(ScheduleLine {
            label_key: "label-repeat",
            value: repeat.clone(),
        });
    }
    if let Some(reminder) = item.reminder {
        lines.push(ScheduleLine {
            label_key: "label-reminder",
            value: if reminder {
                tr("label-on")
            } else {
                tr("label-off")
            },
        });

        if reminder {
            if let Some(ref time) = item.reminder_time {
                if !time.trim().is_empty() {
                    lines.push(ScheduleLine {
                        label_key: "label-reminder-time",
                        value: time.clone(),
                    });
                }
            }
            if let Some(ref offsets) = item.reminder_offsets {
                if !offsets.is_empty() {
                    lines.push(ScheduleLine {
                        label_key: "label-reminder-offsets",
                        value: reminder_offsets_label(offsets),
                    });
                }
            }
        }
    }
    if let Some(minutes) = item.travel_time_minutes {
        if minutes > 0 {
            lines.push(ScheduleLine {
                label_key: "label-travel-time",
                value: format!("{minutes} min"),
            });
        }
    }

    lines
}

fn item_status_parts(item: &ListItem) -> ItemStatusParts {
    let check = if item.is_done.unwrap_or(false) {
        "✓".green().to_string()
    } else {
        "○".to_string()
    };
    let priority = match item.priority.as_deref() {
        Some("high") => " !!!".red().to_string(),
        Some("medium") => " !!".yellow().to_string(),
        Some("low") => " !".dimmed().to_string(),
        _ => String::default(),
    };
    ItemStatusParts { check, priority }
}

fn print_item_quantity(item: &ListItem) {
    if let Some(quantity) = item.quantity.as_deref().filter(|value| !value.is_empty()) {
        println!("  {}  {quantity}", tr("label-quantity").dimmed());
    }
}

fn print_item_schedule(item: &ListItem) {
    for line in schedule_lines(item) {
        println!("  {}  {}", tr(line.label_key).dimmed(), line.value);
    }
}

fn print_item_progress(item: &ListItem) {
    if let Some(progress) = item.progress.as_deref() {
        println!("  {}  {progress}", tr("label-state").dimmed());
    }
}

fn print_item_color(item: &ListItem) {
    if let Some(color) = item.color.as_deref() {
        let dot = color_dot(Some(color));
        println!("  {}  {color}{dot}", tr("label-color").dimmed());
    }
}

fn print_item_tags(item: &ListItem) {
    let Some(tags) = item.tags.as_ref().filter(|tags| !tags.is_empty()) else {
        return;
    };
    let tags: Vec<String> = tags
        .iter()
        .map(|tag| format!("#{tag}").cyan().to_string())
        .collect();
    println!("  {}  {}", tr("label-tags").dimmed(), tags.join("  "));
}

fn print_item_assignees(item: &ListItem) {
    let Some(assigned) = item
        .assigned_to
        .as_ref()
        .filter(|assigned| !assigned.is_empty())
    else {
        return;
    };
    let ids: Vec<String> = assigned.iter().map(|id| id.to_string()).collect();
    println!("  {}  {}", tr("label-assigned").dimmed(), ids.join(", "));
}

fn print_item_notes(item: &ListItem) {
    let Some(notes) = item.notes.as_deref() else {
        return;
    };
    let plain = strip_html(notes);
    if plain.trim().is_empty() {
        return;
    }

    println!("  {}", tr("label-notes").magenta());
    for line in plain.lines() {
        println!("    {line}");
    }
}

fn print_item_tldr(item: &ListItem) {
    if let Some(tldr) = item.tldr.as_deref().filter(|value| !value.is_empty()) {
        println!("  {}  {tldr}", tr("label-tldr").blue());
    }
}

fn print_item_image(item: &ListItem) {
    if let Some(url) = item.image_url.as_deref() {
        println!("  {}  {url}", tr("label-image").dimmed());
    }
}

fn print_item_attachments(item: &ListItem) {
    let Some(attachments) = item
        .attachments
        .as_ref()
        .filter(|attachments| !attachments.is_empty())
    else {
        return;
    };

    println!(
        "  {} ({})",
        tr("label-attachments").dimmed(),
        attachments.len()
    );
    for attachment in attachments {
        let name = attachment
            .original_filename
            .as_deref()
            .or(attachment.filename.as_deref())
            .unwrap_or("?");
        let size = attachment
            .file_size
            .map_or_else(String::default, human_size);
        let url = attachment.url.as_deref().unwrap_or("");
        println!("    • {name}  {size}  {}", url.dimmed());
    }
}

fn print_item_timestamps(item: &ListItem) {
    if let Some(created) = item.created_at.as_deref() {
        let display = &created[..created.len().min(16)];
        println!("  {}  {display}", tr("label-created").dimmed());
    }
    if let Some(updated) = item.updated_at.as_deref() {
        let display = &updated[..updated.len().min(16)];
        println!("  {}  {display}", tr("label-updated").dimmed());
    }
}

/// Print detailed information for a single item and its comments.
pub(crate) fn print_item_detail(item: &ListItem, comments: &[ItemComment]) {
    let ItemStatusParts { check, priority } = item_status_parts(item);
    println!(
        "{check} {}{priority}  {}",
        item.text.bold(),
        format!("(#{})", item.id).dimmed()
    );

    print_item_quantity(item);
    print_item_schedule(item);
    print_item_progress(item);
    print_item_color(item);
    print_item_tags(item);
    print_item_assignees(item);
    print_item_notes(item);
    print_item_tldr(item);
    print_item_image(item);
    print_item_attachments(item);
    print_item_timestamps(item);
    ItemComments { comments }.print();
    println!();
}

/// Print a compact human-readable item list.
pub(crate) fn print_items(items: &[ListItem]) {
    if items.is_empty() {
        println!("{}", tr("output-no-items").dimmed());
        return;
    }
    let parent_by_id: HashMap<i64, Option<i64>> = items
        .iter()
        .map(|item| (item.id, item.parent_item_id))
        .collect();
    for i in items {
        let check = if i.is_done.unwrap_or(false) {
            "✓".green().to_string()
        } else {
            "○".dimmed().to_string()
        };
        let depth = i
            .depth
            .filter(|depth| *depth > 0)
            .unwrap_or_else(|| computed_item_depth(i, &parent_by_id));
        let indent = "  ".repeat(depth as usize);
        let pri = match i.priority.as_deref() {
            Some("high") => " !!!".red().to_string(),
            Some("medium") => " !!".yellow().to_string(),
            Some("low") => " !".dimmed().to_string(),
            _ => String::default(),
        };
        let qty = match &i.quantity {
            Some(q) if !q.is_empty() => format!(" ({})", q.dimmed()),
            _ => String::default(),
        };
        let due = match &i.due_date {
            Some(d) => format!(
                "  {}: {}",
                tr("label-due-lower"),
                date_with_time(d, i.due_time.as_ref()).yellow()
            ),
            None => String::default(),
        };
        let tags_str = match &i.tags {
            Some(t) if !t.is_empty() => {
                let joined: Vec<String> = t.iter().map(|tg| format!("#{tg}")).collect();
                format!("  {}", joined.join(" ").dimmed())
            }
            _ => String::default(),
        };
        let progress = match i.progress.as_deref().map(str::trim) {
            Some(p) if !p.is_empty() => format!("  [{}]", p.cyan()),
            _ => String::default(),
        };
        let sub = match (i.child_count, i.done_child_count) {
            (Some(c), Some(d)) if c > 0 => format!(" [{d}/{c}]").dimmed().to_string(),
            _ => String::default(),
        };
        let text = if i.is_done.unwrap_or(false) {
            i.text.dimmed().strikethrough().to_string()
        } else {
            colorize_text(i.color.as_deref(), &i.text)
        };
        let first_prefix = format!("  {indent}");
        let next_prefix = format!("  {indent}  ");
        let content = format!(
            "{check} {text}{pri}{qty}{sub}{progress}{due}{tags_str}  (#{id})",
            id = i.id,
        );
        print_wrapped_item_line(&first_prefix, &next_prefix, &content);
    }
}

fn computed_item_depth(item: &ListItem, parent_by_id: &HashMap<i64, Option<i64>>) -> i64 {
    let mut depth = 0;
    let mut parent = item.parent_item_id;
    let mut seen = HashSet::new();
    while let Some(parent_id) = parent {
        if !seen.insert(parent_id) {
            break;
        }
        let Some(next_parent) = parent_by_id.get(&parent_id) else {
            break;
        };
        depth += 1;
        parent = *next_parent;
    }
    depth
}

/// Print a list heading followed by its items.
pub(crate) fn print_items_for_list(list: Option<&ShoppingList>, items: &[ListItem]) {
    if let Some(list) = list {
        println!(
            "{}: {}",
            tr("label-items"),
            list_display_name_with_folder(list)
        );
    }
    print_items(items);
}

#[cfg(test)]
mod wrap_and_icon_tests {
    use super::{list_display_name_with_folder, map_bootstrap_icon_label, wrap_ansi_with_prefix};
    use crate::models::ShoppingList;

    #[test]
    fn keeps_hanging_indent_after_wrap() {
        let out = wrap_ansi_with_prefix("o abcdefghijklmnop", "  ", "    ", 10);
        assert_eq!(out, "  o abcdef\n    ghijkl\n    mnop");
    }

    #[test]
    fn keeps_hanging_indent_after_embedded_newline() {
        let out = wrap_ansi_with_prefix("o first\nsecond", "  ", "    ", 80);
        assert_eq!(out, "  o first\n    second");
    }

    #[test]
    fn does_not_count_ansi_escape_sequences() {
        let red = "\u{1b}[31mRED\u{1b}[0m";
        let out = wrap_ansi_with_prefix(&format!("o {red} tail"), "  ", "    ", 9);
        assert!(out.contains("\u{1b}[31mRED\u{1b}[0m"));
        assert_eq!(out, format!("  o {red} t\n    ail"));
    }

    #[test]
    fn unknown_bootstrap_icons_use_fallback_label() {
        assert_eq!(map_bootstrap_icon_label("badge-3d", "list"), "[badge-3d]");
    }

    #[test]
    fn list_display_name_joins_folder_and_name() {
        let list = ShoppingList {
            id: 42,
            name: "Roadmap".to_string(),
            icon: None,
            color: None,
            folder_id: Some(9),
            folder_name: Some("Work/Backend".to_string()),
            archived: Some(false),
            archive_mode: None,
            view_mode: None,
            role: None,
            item_count: None,
            done_count: None,
            state_config: None,
            states: None,
            created_at: None,
        };

        assert_eq!(
            list_display_name_with_folder(&list),
            "Work/Backend / Roadmap"
        );
    }
}

// ── Folders ──

/// Print folders grouped by their hierarchy.
pub(crate) fn print_folders(folders: &[Folder]) {
    if folders.is_empty() {
        println!("{}", tr("output-no-folders").dimmed());
        return;
    }
    let mut folders: Vec<&Folder> = folders.iter().collect();
    folders.sort_by(|a, b| {
        folder_path_parts(a)
            .join("/")
            .to_ascii_lowercase()
            .cmp(&folder_path_parts(b).join("/").to_ascii_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });

    for f in folders {
        let icon = colorize_text(
            f.color.as_deref(),
            &display_icon(f.icon.as_deref(), "folder"),
        );
        let name = colorize_bold_text(f.color.as_deref(), &f.name);
        let color_badge = color_dot(f.color.as_deref());
        let indent = "  ".repeat(folder_path_parts(f).len().saturating_sub(1));
        println!("  {indent}{icon} {:<36} #{}{color_badge}", name, f.id);
    }
}

// ── Members ──

/// Print list members and pending invites.
pub(crate) fn print_members(members: &[Member]) {
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
            String::default()
        };
        println!(
            "  {name:<24} {email:<30} [{} | {}]{badge}",
            role_label(role),
            member_type_label(kind)
        );
    }
}

// ── Search ──

/// Print grouped search results.
pub(crate) fn print_search(results: &SearchResults) {
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
        return String::default();
    };
    match detail {
        serde_json::Value::Null => String::default(),
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
                        .map_or_else(|| value.to_string(), str::to_string);
                    format!("{key}={rendered}")
                })
                .collect();
            if pairs.is_empty() {
                String::default()
            } else {
                pairs.join(", ")
            }
        }
        _ => detail.to_string(),
    }
}

/// Print activity feed entries.
pub(crate) fn print_activity(entries: &[ActivityEntry]) {
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
            .map_or_else(|| "?".to_string(), activity_action_label);
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

#[cfg(test)]
mod tests {
    use super::{
        activity_action_label, activity_detail_text, bootstrap_icon_asset_name, char_display_width,
        color_dot, colorize_bold_text, colorize_text, date_with_time, display_icon, fallback_icon,
        folder_path_parts, human_size, item_status_parts, list_display_name_with_folder,
        list_folder_parts, map_bootstrap_icon_emoji, map_bootstrap_icon_label, member_type_label,
        parse_hex_color, print_activity, print_folders, print_item_detail, print_items,
        print_items_for_list, print_list_detail, print_lists, print_members, print_search,
        print_wrapped_item_line, reminder_offsets_label, role_label, schedule_lines, strip_html,
        view_mode_label, visible_width_ansi, wrap_ansi_with_prefix, IconStyle, ItemComments,
    };
    use crate::models::{
        ActivityEntry, Attachment, Folder, ItemComment, ListItem, ListState, Member, SearchItemHit,
        SearchListHit, SearchResults, ShoppingList,
    };
    use serde_json::json;

    const TEST_ICON_STYLE_ENV: &str = "KRAMLI_ICON_STYLE";

    fn with_icon_style<T>(value: &str, f: impl FnOnce() -> T) -> T {
        let previous = std::env::var(TEST_ICON_STYLE_ENV).ok();
        std::env::set_var(TEST_ICON_STYLE_ENV, value);
        let output = f();
        match previous {
            Some(previous) => std::env::set_var(TEST_ICON_STYLE_ENV, previous),
            None => std::env::remove_var(TEST_ICON_STYLE_ENV),
        }
        output
    }

    fn with_existing_icon_style<T>(value: &str, f: impl FnOnce() -> T) -> T {
        std::env::set_var(TEST_ICON_STYLE_ENV, "label");
        with_icon_style(value, f)
    }

    fn minimal_item() -> ListItem {
        ListItem {
            id: 1,
            list_id: Some(1),
            text: "Test".to_string(),
            is_done: Some(false),
            quantity: None,
            notes: None,
            tldr: None,
            due_date: None,
            due_time: None,
            planned_date: None,
            planned_time: None,
            repeat_label: None,
            reminder: None,
            reminder_time: None,
            reminder_days_before: None,
            reminder_offsets: None,
            travel_time_minutes: None,
            priority: None,
            progress: None,
            tags: None,
            parent_item_id: None,
            depth: None,
            position: None,
            completed_at: None,
            created_at: None,
            updated_at: None,
            assigned_to: None,
            child_count: None,
            done_child_count: None,
            comment_count: None,
            color: None,
            image_url: None,
            image_filename: None,
            attachments: None,
        }
    }

    fn sample_list(id: i64, name: &str) -> ShoppingList {
        ShoppingList {
            id,
            name: name.to_string(),
            icon: Some("bi-cart-fill".to_string()),
            color: Some("#112233".to_string()),
            folder_id: Some(9),
            folder_name: Some("Shop / Weekly".to_string()),
            archived: Some(false),
            archive_mode: None,
            view_mode: Some("board".to_string()),
            role: Some("editor".to_string()),
            item_count: Some(5),
            done_count: Some(2),
            state_config: None,
            states: Some(vec![
                ListState {
                    name: Some("Open".to_string()),
                    color: None,
                    is_done: Some(false),
                },
                ListState {
                    name: Some("Done".to_string()),
                    color: None,
                    is_done: Some(true),
                },
            ]),
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
        }
    }

    #[test]
    fn schedule_lines_keep_due_planned_repeat_reminder_travel_order() {
        let mut item = minimal_item();
        item.due_date = Some("2026-07-20".to_string());
        item.due_time = Some("09:30".to_string());
        item.planned_date = Some("2026-07-19".to_string());
        item.planned_time = Some("08:00".to_string());
        item.repeat_label = Some("Wöchentlich".to_string());
        item.reminder = Some(true);
        item.reminder_time = Some("09:00".to_string());
        item.reminder_offsets = Some(vec![60, 1440]);
        item.travel_time_minutes = Some(20);

        let keys: Vec<&str> = schedule_lines(&item)
            .into_iter()
            .map(|line| line.label_key)
            .collect();

        assert_eq!(
            keys,
            vec![
                "label-due",
                "label-planned",
                "label-repeat",
                "label-reminder",
                "label-reminder-time",
                "label-reminder-offsets",
                "label-travel-time",
            ]
        );
    }

    #[test]
    fn schedule_lines_show_travel_without_reminder() {
        let mut item = minimal_item();
        item.reminder = Some(false);
        item.travel_time_minutes = Some(15);

        let lines = schedule_lines(&item);
        let keys: Vec<&str> = lines.iter().map(|line| line.label_key).collect();

        assert_eq!(keys, vec!["label-reminder", "label-travel-time"]);
        assert_eq!(lines[1].value, "15 min");
    }

    #[test]
    fn reminder_offsets_label_formats_units() {
        assert_eq!(
            reminder_offsets_label(&[5, 60, 180, 1440]),
            "5m, 1h, 3h, 1d"
        );
    }

    #[test]
    fn strip_html_keeps_line_breaks_and_unescapes() {
        let raw = "Hi<br><b>there</b> &amp; &lt;ok&gt;";
        assert_eq!(strip_html(raw), "Hi\nthere & <ok>");
    }

    #[test]
    fn parse_hex_color_accepts_valid_rgb_and_rejects_invalid() {
        assert_eq!(parse_hex_color("#12AbEf"), Some((0x12, 0xAB, 0xEF)));
        assert_eq!(parse_hex_color("12abef"), Some((0x12, 0xAB, 0xEF)));
        assert_eq!(parse_hex_color("#xyzxyz"), None);
        assert_eq!(parse_hex_color("#12345"), None);
    }

    #[test]
    fn date_with_time_appends_non_empty_time() {
        assert_eq!(
            date_with_time("2026-08-01", Some(&"09:30".to_string())),
            "2026-08-01 09:30"
        );
        assert_eq!(
            date_with_time("2026-08-01", Some(&"  ".to_string())),
            "2026-08-01"
        );
        assert_eq!(date_with_time("2026-08-01", None), "2026-08-01");
    }

    #[test]
    fn human_size_formats_units_progressively() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
    }

    #[test]
    fn activity_detail_text_prefers_text_then_changes_then_pairs() {
        let with_text = json!({"text": "Milch"});
        let with_changes = json!({"changes": ["prio", "datum"]});
        let with_pairs = json!({"alpha": 1, "beta": "x"});

        assert!(activity_detail_text(Some(&with_text)).contains("Milch"));

        let changes_rendered = activity_detail_text(Some(&with_changes));
        assert!(changes_rendered.contains("prio"));
        assert!(changes_rendered.contains("datum"));

        let pairs_rendered = activity_detail_text(Some(&with_pairs));
        assert!(pairs_rendered.contains("alpha=1"));
        assert!(pairs_rendered.contains("beta=x"));
    }

    #[test]
    fn icon_wrapping_and_label_helpers_cover_branch_variants() {
        assert_eq!(
            bootstrap_icon_asset_name("bi-cart-fill"),
            Some("cart-fill".to_string())
        );
        assert_eq!(
            bootstrap_icon_asset_name("bootstrap-icons:egg_fried"),
            Some("egg-fried".to_string())
        );
        assert_eq!(bootstrap_icon_asset_name("../cart-fill"), None);

        with_icon_style("label", || {
            assert_eq!(display_icon(None, "folder"), "[folder]");
            assert_eq!(display_icon(Some("bi-cart-fill"), "list"), "[cart]");
            assert_eq!(display_icon(Some("custom"), "list"), "[custom]");
        });

        assert_eq!(char_display_width('\t'), 4);
        assert_eq!(char_display_width('\n'), 0);
        assert_eq!(visible_width_ansi("\u{1b}[31mred\u{1b}[0m"), 3);
        assert_eq!(wrap_ansi_with_prefix("abc", "> ", "  ", 4), "> abc");

        assert!(colorize_text(Some("#010203"), "x").contains('x'));
        assert!(colorize_bold_text(Some("#010203"), "x").contains('x'));
        assert_eq!(colorize_text(Some("bad"), "x"), "x");

        assert_eq!(role_label("owner"), crate::i18n::tr("role-owner"));
        assert_eq!(role_label("custom"), "custom");
        assert_eq!(
            view_mode_label("calendar"),
            crate::i18n::tr("view-calendar")
        );
        assert_eq!(view_mode_label("custom"), "custom");
        assert_eq!(
            member_type_label("invite"),
            crate::i18n::tr("member-type-invite")
        );
        assert_eq!(member_type_label("bot"), "bot");
        assert_eq!(
            activity_action_label("item_created"),
            crate::i18n::tr("activity-item-created")
        );
        assert_eq!(activity_action_label("custom"), "custom");
    }

    #[test]
    fn icon_style_helpers_cover_raw_emoji_and_fallback_branches() {
        assert_eq!(fallback_icon(IconStyle::Emoji, "folder"), "📁");
        assert_eq!(fallback_icon(IconStyle::Emoji, "list"), "📋");
        assert_eq!(fallback_icon(IconStyle::Raw, "list"), "[list]");
        assert_eq!(map_bootstrap_icon_label("", "list"), "[list]");

        for (icon, emoji) in [
            ("cart-fill", "🛒"),
            ("egg-fried", "🍳"),
            ("people-fill", "👥"),
            ("tag", "🏷️"),
            ("tools", "🛠️"),
            ("paperclip", "📎"),
            ("book-fill", "📚"),
            ("check-circle-fill", "✅"),
            ("fire", "🔥"),
            ("cup-hot", "☕"),
            ("folder2", "📁"),
        ] {
            assert_eq!(map_bootstrap_icon_emoji(icon, "fallback"), emoji);
        }
        assert_eq!(map_bootstrap_icon_emoji("unknown", "fallback"), "fallback");

        with_icon_style("raw", || {
            assert_eq!(display_icon(Some("bi-cart-fill"), "list"), "bi-cart-fill");
        });
        with_existing_icon_style("raw", || {
            assert_eq!(display_icon(Some("bi-fire"), "list"), "bi-fire");
        });
        with_icon_style("emoji", || {
            assert_eq!(display_icon(Some("bi-cart-fill"), "list"), "🛒");
            assert_eq!(display_icon(Some("bi-unknown-icon"), "list"), "📋");
            assert_eq!(display_icon(Some("bi-unknown-icon"), "folder"), "📁");
            assert_eq!(display_icon(None, "list"), "📋");
        });
        assert_eq!(display_icon(Some("Raw icon!"), "list"), "Raw icon!");
    }

    #[test]
    fn wrapping_helpers_cover_narrow_escape_and_newline_branches() {
        assert_eq!(visible_width_ansi("abc\u{1b}"), 3);
        assert_eq!(visible_width_ansi("abc\u{1b}["), 3);
        assert_eq!(visible_width_ansi("abc\u{1b}[31"), 3);
        assert_eq!(wrap_ansi_with_prefix("abc", "> ", "    ", 7), "> abc");
        assert_eq!(wrap_ansi_with_prefix("abc", "> ", "", 1), "> abc");
        assert_eq!(wrap_ansi_with_prefix("abcde f", "", "", 5), "abcde\nf");
        assert_eq!(wrap_ansi_with_prefix("abcde ", "", "", 5), "abcde\n");
        assert_eq!(wrap_ansi_with_prefix("a\n b", "> ", "  ", 8), "> a\n   b");
        assert_eq!(wrap_ansi_with_prefix("\u{1b}", "", "", 8), "\u{1b}");
        assert_eq!(wrap_ansi_with_prefix("\u{1b}[31", "", "", 8), "\u{1b}[31");
        print_wrapped_item_line("  ", "    ", "wrapped");
    }

    #[test]
    fn remaining_output_helpers_cover_sort_and_fallback_branches() {
        let icon_from_markup = "<svg><use href=\"#bi-cart-fill\"/></svg>";
        assert_eq!(
            bootstrap_icon_asset_name(icon_from_markup),
            Some("cart-fill".to_string())
        );

        let mut folder_named = sample_list(20, "");
        folder_named.folder_name = Some("Folder Only".to_string());
        assert_eq!(list_display_name_with_folder(&folder_named), "Folder Only");

        let owner = ShoppingList {
            role: Some("owner".to_string()),
            folder_name: None,
            folder_id: None,
            ..sample_list(30, "Owner")
        };
        let alpha = ShoppingList {
            role: Some("viewer".to_string()),
            folder_name: None,
            folder_id: None,
            ..sample_list(31, "Alpha")
        };
        let beta = ShoppingList {
            role: Some("editor".to_string()),
            folder_name: None,
            folder_id: None,
            ..sample_list(32, "Beta")
        };
        print_lists(&[beta, owner, alpha]);

        let list_with_blank_state = ShoppingList {
            states: Some(vec![ListState {
                name: Some("  ".to_string()),
                color: None,
                is_done: Some(false),
            }]),
            ..sample_list(33, "Blank state")
        };
        print_list_detail(&list_with_blank_state);

        assert_eq!(human_size(1024_i64.pow(4)), "1024.0 TB");

        let mut reminder = minimal_item();
        reminder.reminder = Some(true);
        reminder.reminder_time = Some("  ".to_string());
        reminder.reminder_offsets = Some(Vec::new());
        assert_eq!(
            schedule_lines(&reminder)
                .into_iter()
                .map(|line| line.label_key)
                .collect::<Vec<_>>(),
            vec!["label-reminder"]
        );

        let mut reminder_without_details = minimal_item();
        reminder_without_details.reminder = Some(true);
        assert_eq!(
            schedule_lines(&reminder_without_details)
                .into_iter()
                .map(|line| line.label_key)
                .collect::<Vec<_>>(),
            vec!["label-reminder"]
        );

        let mut item = minimal_item();
        item.priority = Some("other".to_string());
        item.parent_item_id = Some(999);
        item.depth = None;
        print_items(&[item]);

        let cycle_a = ListItem {
            id: 40,
            parent_item_id: Some(41),
            ..minimal_item()
        };
        let cycle_b = ListItem {
            id: 41,
            parent_item_id: Some(40),
            ..minimal_item()
        };
        print_items(&[cycle_a, cycle_b]);

        let folder_a = Folder {
            id: 5,
            name: "Beta".to_string(),
            icon: None,
            color: None,
            parent_folder_id: None,
            parent_folder_name: Some("Root".to_string()),
            position: None,
            created_at: None,
        };
        let folder_b = Folder {
            id: 4,
            name: "Alpha".to_string(),
            icon: None,
            color: None,
            parent_folder_id: None,
            parent_folder_name: Some("Root".to_string()),
            position: None,
            created_at: None,
        };
        print_folders(&[folder_a, folder_b]);

        let empty_object = json!({});
        assert_eq!(activity_detail_text(Some(&empty_object)), "");
        assert_eq!(
            activity_detail_text(Some(&json!({"changes": [1, 2]}))),
            "changes=[1,2]"
        );
        let activity_without_user = ActivityEntry {
            id: Some(2),
            list_id: None,
            user_id: None,
            action: None,
            detail: Some(empty_object),
            display_name: None,
            user_name: None,
            nickname: None,
            photo_url: None,
            item_id: None,
            created_at: None,
        };
        print_activity(&[activity_without_user]);

        print_search(&SearchResults {
            lists: Some(Vec::new()),
            items: Some(Vec::new()),
        });
        print_search(&SearchResults {
            lists: None,
            items: Some(vec![SearchItemHit {
                id: 9,
                text: "No list name".to_string(),
                list_id: None,
                list_name: None,
                is_done: Some(false),
            }]),
        });
    }

    #[test]
    fn list_folder_and_print_helpers_cover_human_output_paths() {
        let list = sample_list(1, "Groceries");
        assert_eq!(
            list_display_name_with_folder(&list),
            "Shop / Weekly / Groceries"
        );
        assert_eq!(list_folder_parts(&list), vec!["Shop", "Weekly"]);

        let fallback_folder = ShoppingList {
            folder_name: None,
            ..sample_list(2, "")
        };
        assert_eq!(list_folder_parts(&fallback_folder), vec!["#9"]);
        assert_eq!(
            list_display_name_with_folder(&fallback_folder),
            crate::i18n::tr("common-unknown")
        );

        let named_without_folder = ShoppingList {
            folder_name: None,
            ..sample_list(5, "Named")
        };
        assert_eq!(
            list_display_name_with_folder(&named_without_folder),
            "Named"
        );

        let folder = Folder {
            id: 3,
            name: "Leaf".to_string(),
            icon: Some("bi-folder2".to_string()),
            color: Some("#445566".to_string()),
            parent_folder_id: Some(2),
            parent_folder_name: Some("Root / Child".to_string()),
            position: Some(1),
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
        };
        assert_eq!(folder_path_parts(&folder), vec!["Root", "Child", "Leaf"]);

        print_lists(&[]);
        print_lists(&[list]);
        print_list_detail(&sample_list(4, "Detail"));
        print_folders(&[]);
        print_folders(&[folder]);
    }

    #[test]
    fn item_member_search_and_activity_printers_cover_output_paths() {
        let mut parent = minimal_item();
        parent.id = 10;
        parent.text = "Parent".to_string();
        parent.child_count = Some(2);
        parent.done_child_count = Some(1);
        parent.priority = Some("medium".to_string());
        parent.quantity = Some("2".to_string());
        parent.due_date = Some("2026-08-01".to_string());
        parent.tags = Some(vec!["tag".to_string()]);
        parent.progress = Some("Doing".to_string());

        let mut child = minimal_item();
        child.id = 11;
        child.parent_item_id = Some(10);
        child.is_done = Some(true);
        child.depth = None;
        child.priority = Some("low".to_string());

        print_items(&[]);
        print_items(&[parent.clone(), child]);
        print_items_for_list(Some(&sample_list(5, "Items")), &[parent]);
        print_items_for_list(None, &[]);

        print_members(&[]);
        print_members(&[
            Member {
                user_id: Some(1),
                display_name: Some("Ada".to_string()),
                email: Some("ada@example.test".to_string()),
                role: Some("owner".to_string()),
                member_type: Some("member".to_string()),
            },
            Member {
                user_id: None,
                display_name: None,
                email: None,
                role: Some("viewer".to_string()),
                member_type: Some("invite".to_string()),
            },
        ]);

        print_search(&SearchResults::default());
        print_search(&SearchResults {
            lists: Some(vec![SearchListHit {
                id: 7,
                name: "List hit".to_string(),
                icon: Some("bi-tag".to_string()),
                color: Some("#334455".to_string()),
            }]),
            items: Some(vec![SearchItemHit {
                id: 8,
                text: "Item hit".to_string(),
                list_id: Some(7),
                list_name: Some("List hit".to_string()),
                is_done: Some(true),
            }]),
        });

        assert_eq!(activity_detail_text(None), "");
        assert_eq!(activity_detail_text(Some(&json!(null))), "");
        assert_eq!(activity_detail_text(Some(&json!("raw"))), "raw");
        assert_eq!(activity_detail_text(Some(&json!([1, 2]))), "[1,2]");
        print_activity(&[]);
        print_activity(&[ActivityEntry {
            id: Some(1),
            list_id: Some(2),
            user_id: Some(3),
            action: Some("item_updated".to_string()),
            detail: Some(json!({"changes": ["a", "b"]})),
            display_name: None,
            user_name: Some("Grace".to_string()),
            nickname: None,
            photo_url: None,
            item_id: Some(4),
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
        }]);
    }

    #[test]
    fn item_detail_helpers_render_all_optional_sections() {
        let mut item = minimal_item();
        item.text = "Buy milk".to_string();
        item.is_done = Some(true);
        item.quantity = Some("2 bottles".to_string());
        item.notes = Some("<p>Keep cold</p>".to_string());
        item.tldr = Some("Dairy".to_string());
        item.due_date = Some("2026-07-20".to_string());
        item.due_time = Some("09:30".to_string());
        item.planned_date = Some("2026-07-19".to_string());
        item.planned_time = Some("08:00".to_string());
        item.repeat_label = Some("Weekly".to_string());
        item.reminder = Some(true);
        item.reminder_time = Some("09:00".to_string());
        item.reminder_offsets = Some(vec![60, 1440]);
        item.travel_time_minutes = Some(15);
        item.priority = Some("high".to_string());
        item.progress = Some("Doing".to_string());
        item.tags = Some(vec!["grocery".to_string(), "fresh".to_string()]);
        item.assigned_to = Some(vec![7, 9]);
        item.color = Some("#00aa44".to_string());
        item.image_url = Some("https://example.test/milk.png".to_string());
        item.created_at = Some("2026-07-18T10:20:30Z".to_string());
        item.updated_at = Some("2026-07-19T11:22:33Z".to_string());
        item.attachments = Some(vec![Attachment {
            id: 1,
            filename: Some("receipt.pdf".to_string()),
            original_filename: Some("Receipt.pdf".to_string()),
            mime_type: Some("application/pdf".to_string()),
            file_size: Some(2048),
            url: Some("https://example.test/receipt.pdf".to_string()),
            position: Some(0),
            sensitive: Some(false),
            context: Some("item".to_string()),
            alt_text: None,
        }]);
        let comments = vec![ItemComment {
            id: 1,
            text: Some("Remember lactose-free".to_string()),
            user_id: Some(2),
            user_name: Some("Ada".to_string()),
            user_email: Some("ada@example.test".to_string()),
            created_at: Some("2026-07-19T12:00:00Z".to_string()),
        }];

        let status = item_status_parts(&item);
        assert!(status.check.contains('✓'));
        assert!(status.priority.contains("!!!"));
        print_item_detail(&item, &comments);

        let empty = ItemComments { comments: &[] };
        empty.print();
    }

    #[test]
    fn item_detail_helpers_cover_empty_and_priority_branches() {
        assert_eq!(color_dot(None), "");
        assert_eq!(color_dot(Some("not-a-color")), "");

        let mut item = minimal_item();
        item.priority = Some("medium".to_string());
        assert!(item_status_parts(&item).priority.contains("!!"));

        item.priority = Some("low".to_string());
        assert!(item_status_parts(&item).priority.contains('!'));

        item.priority = None;
        assert_eq!(item_status_parts(&item).priority, "");

        print_item_detail(&item, &[]);

        item.notes = Some("<p>  </p>".to_string());
        print_item_detail(&item, &[]);
    }
}
