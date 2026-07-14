use serde_json::{json, Map, Value};
use std::path::PathBuf;
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::api::ApiClient;
use crate::attachments::{
    ensure_mcp_upload_allowed, initialize_mcp_file_policy, mcp_file_uploads_enabled,
    upload_item_attachment, AttachmentUpload,
};
use crate::config::Config;
use crate::i18n::{tr, tr_args};
use crate::models::{ListItem, ShoppingList};
use crate::telemetry;

const PROTOCOL_VERSION: &str = "2025-11-25";

fn empty_json_object() -> Value {
    Value::Object(Map::new())
}

#[derive(Clone, Copy)]
enum MessageFraming {
    ContentLength,
    JsonLine,
}

struct IncomingMessage {
    value: Value,
    framing: MessageFraming,
}

/// Run the MCP server over standard input and output.
pub(crate) async fn run_stdio() -> Result<(), String> {
    initialize_mcp_file_policy();
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();
    run_with_io(&mut stdin, &mut stdout).await
}

async fn run_with_io<R, W>(stdin: &mut R, stdout: &mut W) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = Vec::new();

    while let Some(message) = read_message(stdin, &mut buffer).await? {
        if let Some(response) = handle_message(message.value).await {
            write_message(stdout, &response, message.framing).await?;
        }
    }

    Ok(())
}

async fn handle_message(message: Value) -> Option<Value> {
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");

    if id.is_none() || method.starts_with("notifications/") {
        return None;
    }

    let span = telemetry::TraceSpan::child("mcp.request", "mcp.request");
    span.set_tag("operation", mcp_method_trace_name(method));

    let id = id.unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "kramli-cli",
                "version": env!("CARGO_PKG_VERSION")
            }
        })),
        "ping" => Ok(empty_json_object()),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => handle_tool_call(message.get("params").unwrap_or(&Value::Null)).await,
        _ => {
            span.set_status(false);
            span.finish();
            return Some(error_response(id, -32601, &tr("mcp-method-not-found")));
        }
    };

    span.set_status(result.is_ok());
    span.finish();
    match result {
        Ok(value) => Some(json!({"jsonrpc": "2.0", "id": id, "result": value})),
        Err(message) => Some(error_response(id, -32603, &message)),
    }
}

async fn handle_tool_call(params: &Value) -> Result<Value, String> {
    initialize_mcp_file_policy();
    let config = Config::load();
    let api = ApiClient::new(&config)?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| tr("mcp-tool-name-missing"))?;
    let args = params
        .get("arguments")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    if name == "upload_item_attachment" && !mcp_file_uploads_enabled() {
        return Err(tr("mcp-file-uploads-disabled"));
    }

    let span = telemetry::TraceSpan::child("mcp.tool", "mcp.tool");
    span.set_tag("action", mcp_tool_trace_name(name));
    span.set_data_i64("argument.count", args.len() as i64);

    let result = match name {
        "list_lists" => list_lists(&api).await,
        "list_items" => list_items(&api, &args).await,
        "create_item" => create_item(&api, &args).await,
        "update_item" => update_item(&api, &args).await,
        "toggle_item_done" => toggle_item_done(&api, &args).await,
        "delete_item" => delete_item(&api, &args).await,
        "upload_item_attachment" => upload_item_attachment_tool(&api, &args).await,
        _ => Err(tr_args("mcp-unknown-tool", &[("name", name.to_string())])),
    };

    span.set_status(result.is_ok());
    span.finish();
    Ok(match result {
        Ok(value) => tool_result(value, false),
        Err(message) => tool_text_result(message, true),
    })
}

fn mcp_method_trace_name(method: &str) -> &'static str {
    match method {
        "initialize" => "initialize",
        "ping" => "ping",
        "tools/list" => "tools_list",
        "tools/call" => "tools_call",
        _ => "unknown",
    }
}

fn mcp_tool_trace_name(name: &str) -> &'static str {
    match name {
        "list_lists" => "list_lists",
        "list_items" => "list_items",
        "create_item" => "create_item",
        "update_item" => "update_item",
        "toggle_item_done" => "toggle_item_done",
        "delete_item" => "delete_item",
        "upload_item_attachment" => "upload_item_attachment",
        _ => "unknown",
    }
}

async fn list_lists(api: &ApiClient) -> Result<Value, String> {
    let lists: Vec<ShoppingList> = api.get("/lists").await?;
    serde_json::to_value(lists).map_err(|e| e.to_string())
}

async fn list_items(api: &ApiClient, args: &Map<String, Value>) -> Result<Value, String> {
    let list_id = required_i64(args, "list_id")?;
    let items: Vec<ListItem> = api.get(&format!("/lists/{list_id}/items")).await?;
    let open = optional_bool(args, "open")?.unwrap_or(false);
    let completed = optional_bool(args, "completed")?.unwrap_or(false);
    let state = optional_string(args, "state")?.map(|value| value.to_ascii_lowercase());
    let contains = optional_string(args, "contains")?.map(|value| value.to_ascii_lowercase());
    let newest = optional_bool(args, "newest")?.unwrap_or(false);
    let oldest = optional_bool(args, "oldest")?.unwrap_or(false);
    let limit = optional_i64(args, "limit")?
        .map(|value| value.max(0) as usize)
        .filter(|value| *value > 0);

    let mut filtered: Vec<ListItem> = items
        .into_iter()
        .filter(|item| {
            let is_done = item.is_done.unwrap_or(false);
            if open && is_done {
                return false;
            }
            if completed && !is_done {
                return false;
            }
            if let Some(state_value) = state.as_deref() {
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
            if let Some(query) = contains.as_deref() {
                if !item.text.to_ascii_lowercase().contains(query) {
                    return false;
                }
            }
            true
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

    serde_json::to_value(filtered).map_err(|e| e.to_string())
}

async fn create_item(api: &ApiClient, args: &Map<String, Value>) -> Result<Value, String> {
    let list_id = required_i64(args, "list_id")?;
    let text = required_string(args, "text")?;
    let mut body = Map::new();
    body.insert("text".to_string(), Value::String(text));
    insert_optional_string(args, &mut body, "quantity", "quantity")?;
    insert_optional_string(args, &mut body, "notes", "notes")?;
    insert_optional_string(args, &mut body, "due_date", "due_date")?;
    insert_optional_string(args, &mut body, "due_time", "due_time")?;
    insert_optional_string(args, &mut body, "planned_date", "planned_date")?;
    insert_optional_string(args, &mut body, "planned_time", "planned_time")?;
    insert_reminder_fields(args, &mut body)?;
    insert_optional_string(args, &mut body, "priority", "priority")?;
    insert_optional_string(args, &mut body, "progress", "progress")?;
    if let Some(tags) = optional_string_array(args, "tags")? {
        body.insert("tags".to_string(), Value::Array(tags));
    }
    if let Some(parent_item_id) = optional_i64(args, "parent_item_id")? {
        body.insert("parent_item_id".to_string(), Value::from(parent_item_id));
    }

    api.post(&format!("/lists/{list_id}/items"), &Value::Object(body))
        .await
}

async fn update_item(api: &ApiClient, args: &Map<String, Value>) -> Result<Value, String> {
    let id = required_i64(args, "id")?;
    let mut body = Map::new();
    insert_optional_string(args, &mut body, "text", "text")?;
    insert_optional_string(args, &mut body, "quantity", "quantity")?;
    insert_optional_string(args, &mut body, "notes", "notes")?;
    insert_optional_string(args, &mut body, "due_date", "due_date")?;
    insert_optional_string(args, &mut body, "due_time", "due_time")?;
    insert_optional_string(args, &mut body, "planned_date", "planned_date")?;
    insert_optional_string(args, &mut body, "planned_time", "planned_time")?;
    insert_reminder_fields(args, &mut body)?;
    insert_optional_string(args, &mut body, "priority", "priority")?;
    insert_optional_string(args, &mut body, "color", "color")?;
    insert_optional_string(args, &mut body, "progress", "progress")?;
    if let Some(tags) = optional_string_array(args, "tags")? {
        body.insert("tags".to_string(), Value::Array(tags));
    }
    if let Some(assigned_to) = optional_i64_array(args, "assigned_to")? {
        body.insert("assigned_to".to_string(), Value::Array(assigned_to));
    }
    if body.is_empty() {
        return Err(tr("mcp-no-changes"));
    }

    api.put(&format!("/items/{id}"), &Value::Object(body)).await
}

async fn toggle_item_done(api: &ApiClient, args: &Map<String, Value>) -> Result<Value, String> {
    let id = required_i64(args, "id")?;
    api.patch_json(&format!("/items/{id}/done"), &empty_json_object())
        .await
}

async fn delete_item(api: &ApiClient, args: &Map<String, Value>) -> Result<Value, String> {
    let id = required_i64(args, "id")?;
    api.delete(&format!("/items/{id}")).await
}

async fn upload_item_attachment_tool(
    api: &ApiClient,
    args: &Map<String, Value>,
) -> Result<Value, String> {
    let id = required_i64(args, "id")?;
    let path = PathBuf::from(required_string(args, "path")?);
    ensure_mcp_upload_allowed(&path)?;
    let attachment = upload_item_attachment(
        api,
        id,
        &AttachmentUpload {
            path,
            sensitive: optional_bool(args, "sensitive")?.unwrap_or(false),
            context: optional_string(args, "context")?,
            alt_text: optional_string(args, "alt_text")?,
        },
    )
    .await?;
    serde_json::to_value(attachment).map_err(|error| error.to_string())
}

fn insert_reminder_fields(
    args: &Map<String, Value>,
    body: &mut Map<String, Value>,
) -> Result<(), String> {
    let reminder = optional_bool(args, "reminder")?;
    let reminder_time = optional_string(args, "reminder_time")?;
    let reminder_days_before = optional_i64(args, "reminder_days_before")?;
    let reminder_offsets = optional_i64_array(args, "reminder_offsets")?;
    let travel_time_minutes = optional_i64(args, "travel_time_minutes")?;
    let has_reminder_details = reminder_time.is_some()
        || reminder_days_before.is_some()
        || reminder_offsets
            .as_ref()
            .is_some_and(|offsets| !offsets.is_empty());

    if let Some(reminder) = reminder.or_else(|| has_reminder_details.then_some(true)) {
        body.insert("reminder".to_string(), Value::Bool(reminder));
    }
    if let Some(reminder_time) = reminder_time {
        body.insert("reminder_time".to_string(), Value::String(reminder_time));
    }
    if let Some(days) = reminder_days_before {
        body.insert("reminder_days_before".to_string(), Value::from(days));
    }
    if let Some(offsets) = reminder_offsets {
        body.insert("reminder_offsets".to_string(), Value::Array(offsets));
    }
    if let Some(minutes) = travel_time_minutes {
        body.insert("travel_time_minutes".to_string(), Value::from(minutes));
    }
    Ok(())
}

fn required_i64(args: &Map<String, Value>, name: &str) -> Result<i64, String> {
    optional_i64(args, name)
        .and_then(|v| v.ok_or_else(|| tr_args("mcp-required-argument", &[("name", name.into())])))
}

fn optional_i64(args: &Map<String, Value>, name: &str) -> Result<Option<i64>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_i64()
            .map(Some)
            .ok_or_else(|| tr_args("mcp-argument-must-int", &[("name", name.into())])),
        Some(Value::String(value)) => value
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(|_| tr_args("mcp-argument-must-int", &[("name", name.into())])),
        _ => Err(tr_args("mcp-argument-must-int", &[("name", name.into())])),
    }
}

fn required_string(args: &Map<String, Value>, name: &str) -> Result<String, String> {
    optional_string(args, name)
        .and_then(|v| v.ok_or_else(|| tr_args("mcp-required-argument", &[("name", name.into())])))
}

fn optional_string(args: &Map<String, Value>, name: &str) -> Result<Option<String>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.trim().to_string()).filter(|v| !v.is_empty())),
        _ => Err(tr_args(
            "mcp-argument-must-string",
            &[("name", name.into())],
        )),
    }
}

fn optional_bool(args: &Map<String, Value>, name: &str) -> Result<Option<bool>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err(tr_args("mcp-argument-must-bool", &[("name", name.into())])),
    }
}

fn optional_string_array(
    args: &Map<String, Value>,
    name: &str,
) -> Result<Option<Vec<Value>>, String> {
    let Some(value) = args.get(name) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let array = value
        .as_array()
        .ok_or_else(|| tr_args("mcp-argument-must-string-array", &[("name", name.into())]))?;
    let mut values = Vec::new();
    for item in array {
        let Some(text) = item.as_str() else {
            return Err(tr_args(
                "mcp-argument-must-string-array",
                &[("name", name.into())],
            ));
        };
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            values.push(Value::String(trimmed.to_string()));
        }
    }
    Ok(Some(values))
}

fn optional_i64_array(args: &Map<String, Value>, name: &str) -> Result<Option<Vec<Value>>, String> {
    let Some(value) = args.get(name) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let array = value
        .as_array()
        .ok_or_else(|| tr_args("mcp-argument-must-int-array", &[("name", name.into())]))?;
    let mut values = Vec::new();
    for item in array {
        let Some(id) = item.as_i64() else {
            return Err(tr_args(
                "mcp-argument-must-int-array",
                &[("name", name.into())],
            ));
        };
        values.push(Value::from(id));
    }
    Ok(Some(values))
}

fn insert_optional_string(
    args: &Map<String, Value>,
    body: &mut Map<String, Value>,
    arg_name: &str,
    body_name: &str,
) -> Result<(), String> {
    if let Some(value) = optional_string(args, arg_name)? {
        body.insert(body_name.to_string(), Value::String(value));
    }
    Ok(())
}

fn tool_result(value: Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    tool_text_result(text, is_error)
}

fn tool_text_result(text: String, is_error: bool) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": text
        }],
        "isError": is_error
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

async fn read_message<R: AsyncRead + Unpin>(
    reader: &mut R,
    buffer: &mut Vec<u8>,
) -> Result<Option<IncomingMessage>, String> {
    loop {
        if let Some(message) = try_parse_message(buffer)? {
            return Ok(Some(message));
        }

        let mut chunk = [0_u8; 8192];
        let bytes_read = reader
            .read(&mut chunk)
            .await
            .map_err(|e| tr_args("mcp-read-message-error", &[("error", e.to_string())]))?;
        if bytes_read == 0 {
            if buffer.iter().all(u8::is_ascii_whitespace) {
                buffer.clear();
                return Ok(None);
            }
            return Err(tr("mcp-incomplete-message"));
        }
        buffer.extend_from_slice(&chunk[..bytes_read]);
    }
}

fn try_parse_message(buffer: &mut Vec<u8>) -> Result<Option<IncomingMessage>, String> {
    trim_leading_ascii_whitespace(buffer);
    if buffer.is_empty() {
        return Ok(None);
    }

    if buffer.first() != Some(&b'{') {
        let Some((header_end, separator_len)) = find_header_end(buffer) else {
            return Ok(None);
        };
        let headers =
            std::str::from_utf8(&buffer[..header_end]).map_err(|_| tr("mcp-header-not-utf8"))?;
        let length = content_length(headers)?;
        let body_start = header_end + separator_len;
        let body_end = body_start + length;
        if buffer.len() < body_end {
            return Ok(None);
        }
        let body = buffer[body_start..body_end].to_vec();
        buffer.drain(..body_end);
        let value = serde_json::from_slice(&body)
            .map_err(|e| tr_args("mcp-invalid-json-body", &[("error", e.to_string())]))?;
        return Ok(Some(IncomingMessage {
            value,
            framing: MessageFraming::ContentLength,
        }));
    }

    if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
        let line: Vec<u8> = buffer.drain(..=newline).collect();
        let trimmed = String::from_utf8_lossy(&line).trim().to_string();
        let value = serde_json::from_str(&trimmed)
            .map_err(|e| tr_args("mcp-invalid-json-line", &[("error", e.to_string())]))?;
        return Ok(Some(IncomingMessage {
            value,
            framing: MessageFraming::JsonLine,
        }));
    }

    Ok(None)
}

async fn write_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    message: &Value,
    framing: MessageFraming,
) -> Result<(), String> {
    let body = serde_json::to_vec(message).map_err(|e| e.to_string())?;
    if matches!(framing, MessageFraming::JsonLine) {
        writer
            .write_all(&body)
            .await
            .map_err(|e| tr_args("mcp-write-response-error", &[("error", e.to_string())]))?;
        writer
            .write_all(b"\n")
            .await
            .map_err(|e| tr_args("mcp-write-response-error", &[("error", e.to_string())]))?;
        return writer
            .flush()
            .await
            .map_err(|e| tr_args("mcp-flush-response-error", &[("error", e.to_string())]));
    }

    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer
        .write_all(header.as_bytes())
        .await
        .map_err(|e| tr_args("mcp-write-response-error", &[("error", e.to_string())]))?;
    writer
        .write_all(&body)
        .await
        .map_err(|e| tr_args("mcp-write-response-error", &[("error", e.to_string())]))?;
    writer
        .flush()
        .await
        .map_err(|e| tr_args("mcp-flush-response-error", &[("error", e.to_string())]))
}

fn trim_leading_ascii_whitespace(buffer: &mut Vec<u8>) {
    let count = buffer
        .iter()
        .take_while(|byte| byte.is_ascii_whitespace())
        .count();
    if count > 0 {
        buffer.drain(..count);
    }
}

fn find_header_end(buffer: &[u8]) -> Option<(usize, usize)> {
    find_bytes(buffer, b"\r\n\r\n")
        .map(|index| (index, 4))
        .or_else(|| find_bytes(buffer, b"\n\n").map(|index| (index, 2)))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn content_length(headers: &str) -> Result<usize, String> {
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value
                .trim()
                .parse::<usize>()
                .map_err(|_| tr("mcp-invalid-content-length"));
        }
    }
    Err(tr("mcp-missing-content-length"))
}

fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "list_lists",
            "description": tr("mcp-tool-list-lists"),
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }),
        json!({
            "name": "list_items",
            "description": tr("mcp-tool-list-items"),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "list_id": {"type": "integer"},
                    "open": {"type": "boolean"},
                    "completed": {"type": "boolean"},
                    "state": {"type": "string"},
                    "contains": {"type": "string"},
                    "newest": {"type": "boolean"},
                    "oldest": {"type": "boolean"},
                    "limit": {"type": "integer"}
                },
                "required": ["list_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "create_item",
            "description": tr("mcp-tool-create-item"),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "list_id": {"type": "integer"},
                    "text": {"type": "string"},
                    "quantity": {"type": "string"},
                    "notes": {"type": "string"},
                    "due_date": {"type": "string", "description": "Due date (YYYY-MM-DD or localized text)."},
                    "due_time": {"type": "string", "description": "Due time (HH:MM)."},
                    "planned_date": {"type": "string", "description": "Planned date (YYYY-MM-DD or localized text)."},
                    "planned_time": {"type": "string", "description": "Planned time (HH:MM)."},
                    "reminder": {"type": "boolean", "description": "Enable additional reminders."},
                    "reminder_time": {"type": "string", "description": "Reminder time (HH:MM)."},
                    "reminder_days_before": {"type": "integer", "description": "Days before due date for reminder."},
                    "reminder_offsets": {"type": "array", "items": {"type": "integer"}, "description": "Additional reminder offsets in minutes."},
                    "travel_time_minutes": {"type": "integer", "description": "Travel time in minutes (independent from reminders)."},
                    "priority": {"type": "string"},
                    "progress": {"type": "string"},
                    "tags": {"type": "array", "items": {"type": "string"}},
                    "parent_item_id": {"type": "integer"}
                },
                "required": ["list_id", "text"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "update_item",
            "description": tr("mcp-tool-update-item"),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "integer"},
                    "text": {"type": "string"},
                    "quantity": {"type": "string"},
                    "notes": {"type": "string"},
                    "due_date": {"type": "string", "description": "Due date (YYYY-MM-DD or localized text)."},
                    "due_time": {"type": "string", "description": "Due time (HH:MM)."},
                    "planned_date": {"type": "string", "description": "Planned date (YYYY-MM-DD or localized text)."},
                    "planned_time": {"type": "string", "description": "Planned time (HH:MM)."},
                    "reminder": {"type": "boolean", "description": "Enable additional reminders."},
                    "reminder_time": {"type": "string", "description": "Reminder time (HH:MM)."},
                    "reminder_days_before": {"type": "integer", "description": "Days before due date for reminder."},
                    "reminder_offsets": {"type": "array", "items": {"type": "integer"}, "description": "Additional reminder offsets in minutes."},
                    "travel_time_minutes": {"type": "integer", "description": "Travel time in minutes (independent from reminders)."},
                    "priority": {"type": "string"},
                    "color": {"type": "string"},
                    "progress": {"type": "string"},
                    "tags": {"type": "array", "items": {"type": "string"}},
                    "assigned_to": {"type": "array", "items": {"type": "integer"}}
                },
                "required": ["id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "toggle_item_done",
            "description": tr("mcp-tool-toggle-item"),
            "inputSchema": {
                "type": "object",
                "properties": {"id": {"type": "integer"}},
                "required": ["id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "delete_item",
            "description": tr("mcp-tool-delete-item"),
            "inputSchema": {
                "type": "object",
                "properties": {"id": {"type": "integer"}},
                "required": ["id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "upload_item_attachment",
            "description": tr("mcp-tool-upload-attachment"),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "integer"},
                    "path": {"type": "string"},
                    "sensitive": {"type": "boolean"},
                    "context": {"type": "string"},
                    "alt_text": {"type": "string"}
                },
                "required": ["id", "path"],
                "additionalProperties": false
            }
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        content_length, create_item, delete_item, error_response, handle_message, handle_tool_call,
        insert_optional_string, insert_reminder_fields, list_items, mcp_method_trace_name,
        mcp_tool_trace_name, optional_bool, optional_i64, optional_i64_array, optional_string,
        optional_string_array, read_message, required_i64, required_string, run_stdio, run_with_io,
        toggle_item_done, tool_result, tool_text_result, tools, try_parse_message, update_item,
        write_message, MessageFraming,
    };
    use crate::api::ApiClient;
    use serde_json::{json, Map, Value};
    use std::future::Future;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
                let (mut stream, _) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept())
                        .await
                        .expect("test server accept timed out")
                        .expect("request should connect");
                let mut buffer = [0_u8; 4096];
                let read = stream.read(&mut buffer).await.expect("request should read");
                requests.push(String::from_utf8_lossy(&buffer[..read]).to_string());
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

    fn schedule_properties_for_tool(tool_name: &str) -> Map<String, Value> {
        let tool = tools()
            .into_iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(tool_name))
            .expect("tool must exist");

        tool.get("inputSchema")
            .and_then(Value::as_object)
            .and_then(|schema| schema.get("properties"))
            .and_then(Value::as_object)
            .cloned()
            .expect("tool inputSchema.properties must exist")
    }

    async fn with_env_vars_async<T, Fut>(vars: &[(&str, &str)], f: impl FnOnce() -> Fut) -> T
    where
        Fut: Future<Output = T>,
    {
        crate::test_env::with_env_vars_async(vars, f).await
    }

    async fn with_env_vars_async_unlocked<T, Fut>(
        vars: &[(&str, &str)],
        f: impl FnOnce() -> Fut,
    ) -> T
    where
        Fut: Future<Output = T>,
    {
        let previous = vars
            .iter()
            .map(|(key, _)| ((*key).to_string(), std::env::var(key).ok()))
            .collect::<Vec<_>>();
        for (key, value) in vars {
            std::env::set_var(key, value);
        }

        let output = f().await;

        for (key, value) in previous {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }

        output
    }

    #[tokio::test]
    async fn env_helper_restores_existing_values() {
        const TEST_ENV: &str = "KRAMLI_MCP_TEST_TMP";
        crate::test_env::with_env_vars_async(&[(TEST_ENV, "before")], || async {
            with_env_vars_async_unlocked(&[(TEST_ENV, "during")], || async {
                assert_eq!(std::env::var(TEST_ENV).as_deref(), Ok("during"));
            })
            .await;

            assert_eq!(std::env::var(TEST_ENV).as_deref(), Ok("before"));
        })
        .await;

        assert!(std::env::var(TEST_ENV).is_err());
    }

    #[test]
    fn parses_content_length_message() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let mut buffer = format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes();
        let message = try_parse_message(&mut buffer)
            .expect("parse should not fail")
            .expect("message expected");
        assert_eq!(
            message.value,
            json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn parses_json_line_message() {
        let mut buffer = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n".to_vec();
        let message = try_parse_message(&mut buffer)
            .expect("parse should not fail")
            .expect("message expected");
        assert_eq!(message.value["method"], "ping");
    }

    #[test]
    fn reads_content_length_case_insensitively() {
        assert_eq!(content_length("content-length: 12").unwrap(), 12);
    }

    #[test]
    fn trace_names_are_low_cardinality() {
        assert_eq!(mcp_method_trace_name("tools/call"), "tools_call");
        assert_eq!(mcp_method_trace_name("initialize"), "initialize");
        assert_eq!(mcp_method_trace_name("ping"), "ping");
        assert_eq!(mcp_method_trace_name("tools/list"), "tools_list");
        assert_eq!(mcp_method_trace_name("notifications/changed"), "unknown");
        assert_eq!(mcp_tool_trace_name("list_lists"), "list_lists");
        assert_eq!(mcp_tool_trace_name("list_items"), "list_items");
        assert_eq!(mcp_tool_trace_name("create_item"), "create_item");
        assert_eq!(mcp_tool_trace_name("update_item"), "update_item");
        assert_eq!(mcp_tool_trace_name("toggle_item_done"), "toggle_item_done");
        assert_eq!(mcp_tool_trace_name("delete_item"), "delete_item");
        assert_eq!(mcp_tool_trace_name("custom_user_input"), "unknown");
    }

    #[tokio::test]
    async fn handles_protocol_messages_without_tool_api() {
        with_env_vars_async(&[("KRAMLI_API_KEY", ""), ("KRAMLI_URL", "")], || async {
            assert!(handle_message(json!({"jsonrpc": "2.0", "method": "ping"}))
                .await
                .is_none());
            assert!(handle_message(
                json!({"jsonrpc": "2.0", "id": 1, "method": "notifications/changed"})
            )
            .await
            .is_none());

            let initialized =
                handle_message(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}))
                    .await
                    .expect("initialize response");
            assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");

            let ping = handle_message(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}))
                .await
                .expect("ping response");
            assert_eq!(ping["result"], json!({}));

            let listed = handle_message(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}))
                .await
                .expect("tools/list response");
            assert!(listed["result"]["tools"]
                .as_array()
                .is_some_and(|tools| tools.len() >= 6));

            let tool_without_credentials = handle_message(json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {"name": "list_lists"}
            }))
            .await
            .expect("tools/call error response");
            assert_eq!(tool_without_credentials["error"]["code"], -32603);

            let unknown = handle_message(json!({"jsonrpc": "2.0", "id": 4, "method": "custom"}))
                .await
                .expect("unknown response");
            assert_eq!(unknown["error"]["code"], -32601);
        })
        .await;
    }

    #[test]
    fn reminder_details_enable_reminders_by_default() {
        let args = json!({"reminder_time": "09:00"})
            .as_object()
            .cloned()
            .unwrap();
        let mut body = Map::new();

        insert_reminder_fields(&args, &mut body).expect("valid reminder fields");

        assert_eq!(body.get("reminder"), Some(&Value::Bool(true)));
        assert_eq!(
            body.get("reminder_time"),
            Some(&Value::String("09:00".to_string()))
        );
    }

    #[test]
    fn explicit_false_reminder_stays_false_with_details() {
        let args = json!({"reminder": false, "travel_time_minutes": 15})
            .as_object()
            .cloned()
            .unwrap();
        let mut body = Map::new();

        insert_reminder_fields(&args, &mut body).expect("valid reminder fields");

        assert_eq!(body.get("reminder"), Some(&Value::Bool(false)));
        assert_eq!(body.get("travel_time_minutes"), Some(&Value::from(15)));
    }

    #[test]
    fn travel_time_does_not_enable_reminder_by_default() {
        let args = json!({"travel_time_minutes": 15})
            .as_object()
            .cloned()
            .unwrap();
        let mut body = Map::new();

        insert_reminder_fields(&args, &mut body).expect("valid reminder fields");

        assert_eq!(body.get("reminder"), None);
        assert_eq!(body.get("travel_time_minutes"), Some(&Value::from(15)));
    }

    #[test]
    fn create_item_schema_has_schedule_descriptions() {
        let properties = schedule_properties_for_tool("create_item");

        let due_date = properties
            .get("due_date")
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("description"))
            .and_then(Value::as_str);
        let due_time = properties
            .get("due_time")
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("description"))
            .and_then(Value::as_str);
        let planned_date = properties
            .get("planned_date")
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("description"))
            .and_then(Value::as_str);
        let planned_time = properties
            .get("planned_time")
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("description"))
            .and_then(Value::as_str);
        let reminder = properties
            .get("reminder")
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("description"))
            .and_then(Value::as_str);
        let reminder_time = properties
            .get("reminder_time")
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("description"))
            .and_then(Value::as_str);
        let reminder_days_before = properties
            .get("reminder_days_before")
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("description"))
            .and_then(Value::as_str);
        let reminder_offsets = properties
            .get("reminder_offsets")
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("description"))
            .and_then(Value::as_str);
        let travel_time_minutes = properties
            .get("travel_time_minutes")
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("description"))
            .and_then(Value::as_str);

        assert_eq!(due_date, Some("Due date (YYYY-MM-DD or localized text)."));
        assert_eq!(due_time, Some("Due time (HH:MM)."));
        assert_eq!(
            planned_date,
            Some("Planned date (YYYY-MM-DD or localized text).")
        );
        assert_eq!(planned_time, Some("Planned time (HH:MM)."));
        assert_eq!(reminder, Some("Enable additional reminders."));
        assert_eq!(reminder_time, Some("Reminder time (HH:MM)."));
        assert_eq!(
            reminder_days_before,
            Some("Days before due date for reminder.")
        );
        assert_eq!(
            reminder_offsets,
            Some("Additional reminder offsets in minutes.")
        );
        assert_eq!(
            travel_time_minutes,
            Some("Travel time in minutes (independent from reminders).")
        );
    }

    #[test]
    fn update_item_schema_has_schedule_descriptions() {
        let properties = schedule_properties_for_tool("update_item");

        let travel_time_minutes = properties
            .get("travel_time_minutes")
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("description"))
            .and_then(Value::as_str);
        let reminder = properties
            .get("reminder")
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("description"))
            .and_then(Value::as_str);
        let planned_date = properties
            .get("planned_date")
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("description"))
            .and_then(Value::as_str);

        assert_eq!(reminder, Some("Enable additional reminders."));
        assert_eq!(
            planned_date,
            Some("Planned date (YYYY-MM-DD or localized text).")
        );
        assert_eq!(
            travel_time_minutes,
            Some("Travel time in minutes (independent from reminders).")
        );
    }

    #[test]
    fn reminder_offsets_also_enable_reminders_by_default() {
        let args = json!({"reminder_offsets": [15, 60]})
            .as_object()
            .cloned()
            .unwrap();
        let mut body = Map::new();

        insert_reminder_fields(&args, &mut body).expect("valid reminder fields");

        assert_eq!(body.get("reminder"), Some(&Value::Bool(true)));
        assert_eq!(
            body.get("reminder_offsets"),
            Some(&Value::Array(vec![Value::from(15), Value::from(60)]))
        );
    }

    #[test]
    fn reminder_days_before_enable_reminders_by_default() {
        let args = json!({"reminder_days_before": 2})
            .as_object()
            .cloned()
            .unwrap();
        let mut body = Map::new();

        insert_reminder_fields(&args, &mut body).expect("valid reminder fields");

        assert_eq!(body.get("reminder"), Some(&Value::Bool(true)));
        assert_eq!(body.get("reminder_days_before"), Some(&Value::from(2)));
    }

    #[test]
    fn argument_parsers_cover_valid_null_and_error_paths() {
        let args = json!({
            "int": "42",
            "bad_int": "x",
            "bool": true,
            "text": "  hello  ",
            "blank": "   ",
            "strings": [" one ", "", "two"],
            "ints": [1, 2],
            "null": null
        })
        .as_object()
        .cloned()
        .unwrap();

        assert_eq!(required_i64(&args, "int").unwrap(), 42);
        assert_eq!(optional_i64(&args, "missing").unwrap(), None);
        assert_eq!(optional_i64(&args, "null").unwrap(), None);
        assert!(optional_i64(&args, "bad_int").is_err());
        assert!(optional_i64(&json!({"int": true}).as_object().unwrap().clone(), "int").is_err());
        assert!(required_i64(&args, "missing").is_err());

        assert_eq!(required_string(&args, "text").unwrap(), "hello");
        assert_eq!(optional_string(&args, "blank").unwrap(), None);
        assert_eq!(optional_string(&args, "null").unwrap(), None);
        assert!(optional_string(&json!({"text": 1}).as_object().unwrap().clone(), "text").is_err());
        assert!(required_string(&args, "missing").is_err());

        assert_eq!(optional_bool(&args, "bool").unwrap(), Some(true));
        assert_eq!(optional_bool(&args, "missing").unwrap(), None);
        assert!(
            optional_bool(&json!({"bool": "yes"}).as_object().unwrap().clone(), "bool").is_err()
        );

        assert_eq!(
            optional_string_array(&args, "strings").unwrap(),
            Some(vec![
                Value::String("one".into()),
                Value::String("two".into())
            ])
        );
        assert_eq!(optional_string_array(&args, "missing").unwrap(), None);
        assert_eq!(optional_string_array(&args, "null").unwrap(), None);
        assert!(optional_string_array(
            &json!({"strings": "one"}).as_object().unwrap().clone(),
            "strings"
        )
        .is_err());
        assert!(optional_string_array(
            &json!({"strings": [1]}).as_object().unwrap().clone(),
            "strings"
        )
        .is_err());

        assert_eq!(
            optional_i64_array(&args, "ints").unwrap(),
            Some(vec![Value::from(1), Value::from(2)])
        );
        assert_eq!(optional_i64_array(&args, "missing").unwrap(), None);
        assert_eq!(optional_i64_array(&args, "null").unwrap(), None);
        assert!(
            optional_i64_array(&json!({"ints": 1}).as_object().unwrap().clone(), "ints").is_err()
        );
        assert!(
            optional_i64_array(&json!({"ints": ["x"]}).as_object().unwrap().clone(), "ints")
                .is_err()
        );
    }

    #[test]
    fn optional_body_helpers_cover_insert_and_reminder_errors() {
        let args = json!({"name": " Kramli ", "bad_reminder": "yes"})
            .as_object()
            .cloned()
            .unwrap();
        let mut body = Map::new();

        insert_optional_string(&args, &mut body, "name", "title").unwrap();
        insert_optional_string(&args, &mut body, "missing", "missing").unwrap();

        assert_eq!(body.get("title"), Some(&Value::String("Kramli".into())));
        assert!(!body.contains_key("missing"));

        let bad = json!({"reminder": "yes"}).as_object().cloned().unwrap();
        assert!(insert_reminder_fields(&bad, &mut Map::new()).is_err());
    }

    #[test]
    fn response_helpers_shape_json_rpc_and_tool_results() {
        let tool = tool_result(json!({"ok": true}), false);
        assert!(!tool["isError"].as_bool().unwrap_or(true));
        assert!(tool["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("ok")));

        let text = tool_text_result("failed".to_string(), true);
        assert!(text["isError"].as_bool().unwrap_or(false));
        assert_eq!(text["content"][0]["text"], "failed");

        let err = error_response(Value::from(9), -1, "nope");
        assert_eq!(err["jsonrpc"], "2.0");
        assert_eq!(err["id"], 9);
        assert_eq!(err["error"]["code"], -1);
        assert_eq!(err["error"]["message"], "nope");
    }

    #[test]
    fn create_item_schema_required_fields_are_stable() {
        let tool = tools()
            .into_iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some("create_item"))
            .expect("create_item tool must exist");

        let required = tool
            .get("inputSchema")
            .and_then(Value::as_object)
            .and_then(|schema| schema.get("required"))
            .and_then(Value::as_array)
            .cloned()
            .expect("required array must exist");

        assert_eq!(
            required,
            vec![
                Value::String("list_id".into()),
                Value::String("text".into())
            ]
        );
    }

    #[test]
    fn update_item_schema_required_fields_are_stable() {
        let tool = tools()
            .into_iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some("update_item"))
            .expect("update_item tool must exist");

        let required = tool
            .get("inputSchema")
            .and_then(Value::as_object)
            .and_then(|schema| schema.get("required"))
            .and_then(Value::as_array)
            .cloned()
            .expect("required array must exist");

        assert_eq!(required, vec![Value::String("id".into())]);
    }

    #[tokio::test]
    async fn writes_json_line_response_for_json_line_message() {
        let mut output = Vec::new();
        write_message(
            &mut output,
            &json!({"jsonrpc": "2.0", "id": 1, "result": {}}),
            MessageFraming::JsonLine,
        )
        .await
        .expect("write should not fail");

        assert!(output.ends_with(b"\n"));
        assert!(!output.starts_with(b"Content-Length:"));
    }

    #[tokio::test]
    async fn reads_messages_until_complete_or_eof() {
        let mut reader = b"  \n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n".as_slice();
        let mut buffer = Vec::new();
        let message = read_message(&mut reader, &mut buffer)
            .await
            .expect("read should succeed")
            .expect("message expected");

        assert_eq!(message.value["method"], "ping");
        assert!(matches!(message.framing, MessageFraming::JsonLine));

        let mut empty_reader = b" \n\t".as_slice();
        let mut empty_buffer = Vec::new();
        assert!(read_message(&mut empty_reader, &mut empty_buffer)
            .await
            .expect("whitespace eof")
            .is_none());

        let mut incomplete_reader = b"{\"jsonrpc\":".as_slice();
        let mut incomplete_buffer = Vec::new();
        assert!(read_message(&mut incomplete_reader, &mut incomplete_buffer)
            .await
            .is_err());
    }

    #[test]
    fn parser_reports_header_and_json_errors() {
        let body = b"not-json";
        let mut invalid_body = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        invalid_body.extend_from_slice(body);
        assert!(try_parse_message(&mut invalid_body).is_err());

        let mut invalid_line = b"{invalid}\n".to_vec();
        assert!(try_parse_message(&mut invalid_line).is_err());

        let mut empty_line = b"\n".to_vec();
        assert!(try_parse_message(&mut empty_line)
            .expect("empty line parses as no message")
            .is_none());

        let mut partial_header = b"Content-Length: 5\r\n".to_vec();
        assert!(try_parse_message(&mut partial_header)
            .expect("partial header waits")
            .is_none());

        let mut invalid_header = vec![0xff, b':', b' ', b'1', b'\r', b'\n', b'\r', b'\n'];
        invalid_header.extend_from_slice(b"{}");
        assert!(try_parse_message(&mut invalid_header).is_err());

        assert!(content_length("Content-Type: application/json").is_err());
        assert!(content_length("Content-Length: nope").is_err());
    }

    #[tokio::test]
    async fn writes_content_length_response_for_header_framing() {
        let mut output = Vec::new();

        write_message(
            &mut output,
            &json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": true}}),
            MessageFraming::ContentLength,
        )
        .await
        .expect("write should not fail");

        let text = String::from_utf8(output).expect("utf8 output");
        assert!(text.starts_with("Content-Length: "));
        assert!(text.contains("\r\n\r\n"));
        assert!(text.contains("\"ok\":true"));
    }

    #[tokio::test]
    async fn list_items_and_item_mutations_cover_api_paths() {
        let (api, requests) = api_with_responses(vec![
            json!([
                {
                    "id": 1,
                    "list_id": 7,
                    "text": "Milk",
                    "is_done": false,
                    "progress": "Todo",
                    "created_at": "2026-01-03"
                },
                {
                    "id": 2,
                    "list_id": 7,
                    "text": "Milk done",
                    "is_done": true,
                    "progress": "Todo",
                    "created_at": "2026-01-02"
                },
                {
                    "id": 3,
                    "list_id": 7,
                    "text": "Bread",
                    "is_done": false,
                    "progress": "Todo",
                    "created_at": "2026-01-01"
                }
            ])
            .to_string(),
            json!([
                {
                    "id": 10,
                    "list_id": 7,
                    "text": "first",
                    "is_done": false,
                    "created_at": "2026-01-01"
                },
                {
                    "id": 11,
                    "list_id": 7,
                    "text": "second",
                    "is_done": false,
                    "created_at": "2026-01-02"
                }
            ])
            .to_string(),
            json!({"id": 99, "text": "Created"}).to_string(),
            json!({"id": 99, "text": "Updated"}).to_string(),
            json!({"id": 99, "is_done": true}).to_string(),
            json!({"ok": true}).to_string(),
        ])
        .await;

        let args = json!({
            "list_id": 7,
            "open": true,
            "state": "todo",
            "contains": "milk",
            "newest": true,
            "limit": 1
        })
        .as_object()
        .cloned()
        .expect("args object");
        let filtered = list_items(&api, &args)
            .await
            .expect("list_items should succeed");
        let filtered = filtered.as_array().expect("array response");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0]["id"], 1);

        let args = json!({
            "list_id": 7,
            "oldest": true,
            "completed": false
        })
        .as_object()
        .cloned()
        .expect("args object");
        let ordered = list_items(&api, &args)
            .await
            .expect("list_items should succeed");
        let ordered = ordered.as_array().expect("array response");
        assert_eq!(ordered[0]["id"], 10);

        let create_args = json!({
            "list_id": 7,
            "text": "Created",
            "quantity": "2",
            "notes": "memo",
            "due_date": "2026-02-01",
            "due_time": "09:00",
            "planned_date": "2026-01-31",
            "planned_time": "18:00",
            "reminder_time": "08:30",
            "reminder_offsets": [30, 60],
            "travel_time_minutes": 15,
            "priority": "high",
            "progress": "todo",
            "tags": ["x", "y"],
            "parent_item_id": 5
        })
        .as_object()
        .cloned()
        .expect("args object");
        let created = create_item(&api, &create_args)
            .await
            .expect("create_item should succeed");
        assert_eq!(created["id"], 99);

        let update_args = json!({
            "id": 99,
            "text": "Updated",
            "color": "#fff",
            "assigned_to": [1, 2],
            "reminder": false
        })
        .as_object()
        .cloned()
        .expect("args object");
        let updated = update_item(&api, &update_args)
            .await
            .expect("update_item should succeed");
        assert_eq!(updated["text"], "Updated");

        let no_change_args = json!({"id": 99}).as_object().cloned().expect("args object");
        assert!(update_item(&api, &no_change_args).await.is_err());

        let toggle_args = json!({"id": 99}).as_object().cloned().expect("args object");
        let toggled = toggle_item_done(&api, &toggle_args)
            .await
            .expect("toggle should succeed");
        assert_eq!(toggled["is_done"], true);

        let deleted = delete_item(&api, &toggle_args)
            .await
            .expect("delete should succeed");
        assert_eq!(deleted["ok"], true);

        let requests = requests.await.expect("server should finish");
        let first_lines = requests
            .iter()
            .map(|request| request.lines().next().unwrap_or_default().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            first_lines,
            vec![
                "GET /api/lists/7/items HTTP/1.1",
                "GET /api/lists/7/items HTTP/1.1",
                "POST /api/lists/7/items HTTP/1.1",
                "PUT /api/items/99 HTTP/1.1",
                "PATCH /api/items/99/done HTTP/1.1",
                "DELETE /api/items/99 HTTP/1.1"
            ]
        );

        assert!(requests[2].contains("\"parent_item_id\":5"));
        assert!(requests[2].contains("\"reminder\":true"));
        assert!(requests[3].contains("\"assigned_to\":[1,2]"));
        assert!(requests[3].contains("\"reminder\":false"));
    }

    #[tokio::test]
    async fn io_loop_and_tool_dispatch_cover_remaining_paths() {
        let (stream_client, stream_server) = tokio::io::duplex(4096);
        let (mut reader, mut writer) = tokio::io::split(stream_server);
        let mut client = stream_client;
        let server_task = tokio::spawn(async move { run_with_io(&mut reader, &mut writer).await });

        client
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
            .await
            .expect("request should be written");
        client
            .shutdown()
            .await
            .expect("client write side should close");
        let mut output = Vec::new();
        client
            .read_to_end(&mut output)
            .await
            .expect("response should be readable");
        server_task
            .await
            .expect("io task join should succeed")
            .expect("io loop should succeed");

        let text = String::from_utf8(output).expect("response output should be utf8");
        assert!(text.contains("\"result\":{}"));

        with_env_vars_async(
            &[
                ("KRAMLI_URL", "http://127.0.0.1:9"),
                ("KRAMLI_API_KEY", "kramli_test"),
            ],
            || async {
                let missing_name = handle_tool_call(&json!({})).await;
                assert!(missing_name.is_err());

                let unknown = handle_tool_call(&json!({"name": "unknown_tool"}))
                    .await
                    .expect("unknown tool should return tool error result");
                assert!(unknown
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false));

                let missing_args =
                    handle_tool_call(&json!({"name": "list_items", "arguments": {}}))
                        .await
                        .expect("list_items without args should return tool error result");
                assert!(missing_args
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false));
            },
        )
        .await;
    }

    #[tokio::test]
    async fn list_lists_and_handle_tool_call_cover_success_path() {
        let (api, requests) = api_with_responses(vec![
            json!([
                {
                    "id": 7,
                    "name": "Groceries",
                    "icon": "cart",
                    "color": "#00aa00"
                }
            ])
            .to_string(),
            json!([
                {
                    "id": 9,
                    "name": "Weekend",
                    "icon": "calendar",
                    "color": "#112233"
                }
            ])
            .to_string(),
        ])
        .await;

        let listed = super::list_lists(&api)
            .await
            .expect("list_lists should succeed");
        assert_eq!(listed[0]["id"], 7);

        let base_url = api.base_url_for_tests().to_string();
        let result = with_env_vars_async(
            &[
                ("KRAMLI_URL", base_url.as_str()),
                ("KRAMLI_API_KEY", "kramli_test"),
            ],
            || async {
                handle_tool_call(&json!({"name": "list_lists"}))
                    .await
                    .expect("list_lists tool should succeed")
            },
        )
        .await;
        assert!(!result["isError"].as_bool().unwrap_or(true));
        assert!(result["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("Weekend")));

        let requests = requests.await.expect("server should finish");
        let first_lines = requests
            .iter()
            .map(|request| request.lines().next().unwrap_or_default().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            first_lines,
            vec!["GET /api/lists HTTP/1.1", "GET /api/lists HTTP/1.1"]
        );
    }

    #[tokio::test]
    async fn list_items_and_update_tags_cover_remaining_filter_and_body_paths() {
        let (api, requests) = api_with_responses(vec![
            json!([
                {
                    "id": 1,
                    "list_id": 5,
                    "text": "Open",
                    "is_done": false,
                    "progress": "todo",
                    "created_at": "2026-01-01"
                },
                {
                    "id": 2,
                    "list_id": 5,
                    "text": "Done",
                    "is_done": true,
                    "progress": "done",
                    "created_at": "2026-01-02"
                }
            ])
            .to_string(),
            json!([
                {
                    "id": 1,
                    "list_id": 5,
                    "text": "Open",
                    "is_done": false,
                    "progress": "todo",
                    "created_at": "2026-01-01"
                }
            ])
            .to_string(),
            json!({"id": 9, "text": "Tagged"}).to_string(),
        ])
        .await;

        let completed_args = json!({"list_id": 5, "completed": true})
            .as_object()
            .cloned()
            .expect("args object");
        let completed = list_items(&api, &completed_args)
            .await
            .expect("completed list should succeed");
        assert_eq!(completed.as_array().expect("array").len(), 1);
        assert_eq!(completed[0]["id"], 2);

        let mismatched_state_args = json!({"list_id": 5, "state": "in_progress", "oldest": true})
            .as_object()
            .cloned()
            .expect("args object");
        let mismatched = list_items(&api, &mismatched_state_args)
            .await
            .expect("state mismatch query should succeed");
        assert!(mismatched.as_array().expect("array").is_empty());

        let update_args = json!({"id": 9, "tags": ["  one  ", "two", ""]})
            .as_object()
            .cloned()
            .expect("args object");
        let updated = update_item(&api, &update_args)
            .await
            .expect("update with tags should succeed");
        assert_eq!(updated["id"], 9);

        let requests = requests.await.expect("server should finish");
        assert!(requests[2].contains("\"tags\":[\"one\",\"two\"]"));
    }

    #[test]
    fn parser_waits_for_full_content_length_body_and_skips_header_lines_without_colon() {
        let body = b"{}";
        let mut incomplete = b"HeaderWithoutColon\r\nContent-Length: 2\r\n\r\n{".to_vec();
        assert!(try_parse_message(&mut incomplete)
            .expect("incomplete body should wait")
            .is_none());

        let mut complete = b"HeaderWithoutColon\r\nContent-Length: 2\r\n\r\n{}".to_vec();
        let parsed = try_parse_message(&mut complete)
            .expect("complete body should parse")
            .expect("message expected");
        assert_eq!(parsed.value, json!({}));
        assert!(matches!(parsed.framing, MessageFraming::ContentLength));
        assert!(complete.is_empty());

        let mut no_content_length = b"HeaderWithoutColon\r\nAnotherBadLine\r\n\r\n{}".to_vec();
        assert!(try_parse_message(&mut no_content_length).is_err());

        assert_eq!(body.len(), 2);
    }

    #[tokio::test]
    async fn run_stdio_entrypoint_is_reachable_under_timeout() {
        let _ = tokio::time::timeout(Duration::from_millis(5), run_stdio()).await;
    }
}
