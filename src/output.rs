use colored::Colorize;
use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;

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
        return String::new();
    };

    match parse_hex_color(color) {
        Some((r, g, b)) => format!(" {}", "●".truecolor(r, g, b)),
        None => String::new(),
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

pub fn print_lists(lists: &[ShoppingList]) {
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
    for line in schedule_lines(item) {
        println!("  {}  {}", tr(line.label_key).dimmed(), line.value);
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
            _ => String::new(),
        };
        let qty = match &i.quantity {
            Some(q) if !q.is_empty() => format!(" ({})", q.dimmed()),
            _ => String::new(),
        };
        let due = match &i.due_date {
            Some(d) => format!(
                "  {}: {}",
                tr("label-due-lower"),
                date_with_time(d, i.due_time.as_ref()).yellow()
            ),
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

pub fn print_items_for_list(list: Option<&ShoppingList>, items: &[ListItem]) {
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
mod tests {
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

pub fn print_folders(folders: &[Folder]) {
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

#[cfg(test)]
mod tests {
    use super::{
        activity_detail_text, date_with_time, human_size, parse_hex_color, reminder_offsets_label,
        schedule_lines, strip_html,
    };
    use crate::models::ListItem;
    use serde_json::json;

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
}
