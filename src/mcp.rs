use serde_json::{json, Map, Value};
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::api::ApiClient;
use crate::config::Config;
use crate::i18n::{tr, tr_args};
use crate::models::{ListItem, ShoppingList};

const PROTOCOL_VERSION: &str = "2025-11-25";

#[derive(Clone, Copy)]
enum MessageFraming {
    ContentLength,
    JsonLine,
}

struct IncomingMessage {
    value: Value,
    framing: MessageFraming,
}

pub async fn run_stdio() -> Result<(), String> {
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut buffer = Vec::new();

    while let Some(message) = read_message(&mut stdin, &mut buffer).await? {
        if let Some(response) = handle_message(message.value).await {
            write_message(&mut stdout, &response, message.framing).await?;
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
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => handle_tool_call(message.get("params").unwrap_or(&Value::Null)).await,
        _ => return Some(error_response(id, -32601, &tr("mcp-method-not-found"))),
    };

    match result {
        Ok(value) => Some(json!({"jsonrpc": "2.0", "id": id, "result": value})),
        Err(message) => Some(error_response(id, -32603, &message)),
    }
}

async fn handle_tool_call(params: &Value) -> Result<Value, String> {
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

    let result = match name {
        "list_lists" => list_lists(&api).await,
        "list_items" => list_items(&api, &args).await,
        "create_item" => create_item(&api, &args).await,
        "update_item" => update_item(&api, &args).await,
        "toggle_item_done" => toggle_item_done(&api, &args).await,
        "delete_item" => delete_item(&api, &args).await,
        _ => Err(tr_args("mcp-unknown-tool", &[("name", name.to_string())])),
    };

    Ok(match result {
        Ok(value) => tool_result(value, false),
        Err(message) => tool_text_result(message, true),
    })
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
    api.patch_json(&format!("/items/{id}/done"), &json!({}))
        .await
}

async fn delete_item(api: &ApiClient, args: &Map<String, Value>) -> Result<Value, String> {
    let id = required_i64(args, "id")?;
    api.delete(&format!("/items/{id}")).await
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
        if trimmed.is_empty() {
            return Ok(None);
        }
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
                    "contains": {"type": "string"}
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
                    "due_date": {"type": "string"},
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
                    "due_date": {"type": "string"},
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
    ]
}

#[cfg(test)]
mod tests {
    use super::{content_length, try_parse_message, write_message, MessageFraming};
    use serde_json::json;

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
}
