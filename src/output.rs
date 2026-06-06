use colored::Colorize;

use crate::models::{
    ActivityEntry, Folder, ItemComment, ListItem, Member, SearchResults, ShoppingList,
};

// ── Lists ──

pub fn print_lists(lists: &[ShoppingList]) {
    if lists.is_empty() {
        println!("{}", "No lists found.".dimmed());
        return;
    }
    for l in lists {
        let icon = l.icon.as_deref().unwrap_or("📋");
        let done = l.done_count.unwrap_or(0);
        let total = l.item_count.unwrap_or(0);
        let role = l.role.as_deref().unwrap_or("");
        let role_badge = if role == "owner" {
            "".to_string()
        } else {
            format!(" ({})", role.dimmed())
        };
        let color_dot = match l.color.as_deref() {
            Some(c) => format!("[{}] ", c),
            None => String::new(),
        };
        println!(
            "  {icon} {color_dot}{:<36} {done:>3}/{total:<3}  #{}{role_badge}",
            l.name.bold(),
            l.id,
        );
    }
}

pub fn print_list_detail(l: &ShoppingList) {
    let icon = l.icon.as_deref().unwrap_or("📋");
    println!("{icon}  {} (#{}) ", l.name.bold(), l.id);
    if let Some(ref c) = l.color {
        println!("   Color:     {c}");
    }
    if let Some(ref vm) = l.view_mode {
        println!("   View:      {vm}");
    }
    if let Some(ref r) = l.role {
        println!("   Role:      {r}");
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
            println!("   States:    {}", names.join(" | "));
        }
    }
    if let Some(fid) = l.folder_id {
        println!("   Folder ID: {fid}");
    }
    let done = l.done_count.unwrap_or(0);
    let total = l.item_count.unwrap_or(0);
    println!("   Items:     {done}/{total} done");
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
            println!("  {}  {q}", "Quantity:".dimmed());
        }
    }
    if let Some(ref d) = item.due_date {
        println!("  {}  {}", "Due:".dimmed(), d.yellow());
    }
    if let Some(ref d) = item.planned_date {
        println!("  {}  {d}", "Planned:".dimmed());
    }
    if let Some(ref r) = item.repeat_label {
        println!("  {}  {r}", "Repeat:".dimmed());
    }
    if let Some(ref p) = item.progress {
        println!("  {}  {p}", "State:".dimmed());
    }
    if let Some(ref c) = item.color {
        println!("  {}  {c}", "Color:".dimmed());
    }
    if let Some(ref tags) = item.tags {
        if !tags.is_empty() {
            let t: Vec<String> = tags
                .iter()
                .map(|t| format!("#{t}").cyan().to_string())
                .collect();
            println!("  {}  {}", "Tags:".dimmed(), t.join("  "));
        }
    }
    if let Some(ref assigned) = item.assigned_to {
        if !assigned.is_empty() {
            let ids: Vec<String> = assigned.iter().map(|id| id.to_string()).collect();
            println!("  {}  {}", "Assigned:".dimmed(), ids.join(", "));
        }
    }
    if let Some(ref notes) = item.notes {
        let plain = strip_html(notes);
        if !plain.trim().is_empty() {
            println!("  {}", "Notes:".magenta());
            for line in plain.lines() {
                println!("    {line}");
            }
        }
    }
    if let Some(ref tldr) = item.tldr {
        if !tldr.is_empty() {
            println!("  {}  {tldr}", "TLDR:".blue());
        }
    }
    if let Some(ref url) = item.image_url {
        println!("  {}  {url}", "Image:".dimmed());
    }
    if let Some(ref atts) = item.attachments {
        if !atts.is_empty() {
            println!("  {} ({})", "Attachments:".dimmed(), atts.len());
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
        println!("  {}  {display}", "Created:".dimmed());
    }
    if let Some(ref updated) = item.updated_at {
        let display = &updated[..updated.len().min(16)];
        println!("  {}  {display}", "Updated:".dimmed());
    }
    // Comments
    if !comments.is_empty() {
        println!();
        println!("  {} ({})", "Comments:".bold(), comments.len());
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
        println!("{}", "No items.".dimmed());
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
            Some(d) => format!("  due: {}", d.yellow()),
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
            i.text.to_string()
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
        println!("{}", "No folders.".dimmed());
        return;
    }
    for f in folders {
        let icon = f.icon.as_deref().unwrap_or("📁");
        println!("  {icon} {:<36} #{}", f.name.bold(), f.id);
    }
}

// ── Members ──

pub fn print_members(members: &[Member]) {
    if members.is_empty() {
        println!("{}", "No members.".dimmed());
        return;
    }
    for m in members {
        let name = m.display_name.as_deref().unwrap_or("(unknown)");
        let email = m.email.as_deref().unwrap_or("");
        let role = m.role.as_deref().unwrap_or("?");
        let kind = m.member_type.as_deref().unwrap_or("member");
        let badge = if kind == "invite" {
            " (invited)".yellow().to_string()
        } else {
            String::new()
        };
        println!("  {name:<24} {email:<30} [{role}]{badge}");
    }
}

// ── Search ──

pub fn print_search(results: &SearchResults) {
    let mut any = false;
    if let Some(ref lists) = results.lists {
        if !lists.is_empty() {
            any = true;
            println!("{}", "Lists:".bold().underline());
            for l in lists {
                let icon = l.icon.as_deref().unwrap_or("📋");
                println!("  {icon} {} (#{}) ", l.name, l.id);
            }
        }
    }
    if let Some(ref items) = results.items {
        if !items.is_empty() {
            any = true;
            println!("{}", "Items:".bold().underline());
            for i in items {
                let check = if i.is_done.unwrap_or(false) {
                    "✓"
                } else {
                    "○"
                };
                let ln = i.list_name.as_deref().unwrap_or("?");
                println!("  {check} {} (#{}) in {ln}", i.text, i.id);
            }
        }
    }
    if !any {
        println!("{}", "No results.".dimmed());
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
        _ => serde_json::to_string(detail).unwrap_or_default(),
    }
}

pub fn print_activity(entries: &[ActivityEntry]) {
    if entries.is_empty() {
        println!("{}", "No activity.".dimmed());
        return;
    }
    for a in entries {
        let who = a
            .display_name
            .as_deref()
            .or(a.user_name.as_deref())
            .unwrap_or("?");
        let action = a.action.as_deref().unwrap_or("?");
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
