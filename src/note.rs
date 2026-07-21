use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::i18n::tr;

static MUTATION_ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PlainNote {
    pub(crate) content: String,
    pub(crate) delta: String,
    pub(crate) version: i64,
}

pub(crate) fn normalize_list_type(list_type: Option<String>) -> Option<String> {
    let normalized = list_type?.trim().to_ascii_lowercase().trim().to_string();
    if normalized.is_empty() {
        return None;
    }

    let canonical = match normalized.as_str() {
        "task" | "tasks" | "todo" | "todos" | "list" | "items" => "tasks",
        "note" | "notes" | "markdown" | "notizzettel" => "note",
        _ => normalized.as_str(),
    };
    Some(canonical.to_string())
}

pub(crate) fn is_note_list_type(list_type: Option<&str>) -> bool {
    normalize_list_type(list_type.map(str::to_string)).as_deref() == Some("note")
}

pub(crate) fn is_note_payload(payload: &Value) -> bool {
    is_note_list_type(payload.get("list_type").and_then(Value::as_str))
}

pub(crate) fn ensure_task_list(payload: &Value) -> Result<(), String> {
    if is_note_payload(payload) {
        Err(tr("note-task-mutation-blocked"))
    } else {
        Ok(())
    }
}

pub(crate) fn note_content(payload: &Value) -> Option<&str> {
    payload.get("note_content").and_then(Value::as_str)
}

pub(crate) fn validate_plain_note(payload: &Value) -> Result<PlainNote, String> {
    if !is_note_payload(payload) {
        return Err(tr("cli-note-metadata-required"));
    }
    let delta = payload
        .get("note_delta")
        .and_then(Value::as_str)
        .ok_or_else(|| tr("cli-note-metadata-required"))?;
    let version = payload
        .get("note_version")
        .and_then(Value::as_i64)
        .ok_or_else(|| tr("cli-note-metadata-required"))?;
    let expected = note_content(payload).ok_or_else(|| tr("cli-note-metadata-required"))?;
    let operations: Vec<Value> = if delta.is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(delta).map_err(|_| tr("cli-note-formatting-unsafe"))?
    };
    let mut delta_text = String::new();
    for operation in operations {
        let object = operation
            .as_object()
            .filter(|object| object.len() == 1)
            .ok_or_else(|| tr("cli-note-formatting-unsafe"))?;
        let insert = object
            .get("insert")
            .and_then(Value::as_str)
            .ok_or_else(|| tr("cli-note-formatting-unsafe"))?;
        delta_text.push_str(insert);
    }

    let terminal_newline_is_unambiguous = !expected.chars().last().is_some_and(char::is_whitespace)
        && delta_text == format!("{expected}\n");
    if delta_text != expected && !terminal_newline_is_unambiguous {
        return Err(tr("cli-note-formatting-unsafe"));
    }

    Ok(PlainNote {
        content: expected.to_string(),
        delta: delta.to_string(),
        version,
    })
}

pub(crate) fn plain_text_delta(content: &str) -> Result<String, String> {
    let mut canonical = content.replace("\r\n", "\n").replace('\r', "\n");
    if !canonical.ends_with('\n') {
        canonical.push('\n');
    }
    serde_json::to_string(&vec![json!({"insert": canonical})]).map_err(|error| error.to_string())
}

pub(crate) fn safe_update_payload(
    current: &Value,
    content: &str,
    mut fields: Map<String, Value>,
) -> Result<Value, String> {
    let plain = validate_plain_note(current)?;
    fields.remove("list_type");
    fields.insert(
        "note_delta".to_string(),
        Value::String(plain_text_delta(content)?),
    );
    fields.insert("base_delta".to_string(), Value::String(plain.delta));
    fields.insert("base_version".to_string(), Value::from(plain.version));
    fields.insert(
        "client_mutation_id".to_string(),
        Value::String(mutation_id()),
    );
    Ok(Value::Object(fields))
}

pub(crate) fn mutation_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = MUTATION_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("cli-{}-{nanos}-{sequence}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_only_lossless_plain_deltas() {
        let plain = json!({
            "list_type": "note",
            "note_content": "first\nsecond",
            "note_delta": "[{\"insert\":\"first\"},{\"insert\":\"\\nsecond\\n\"}]",
            "note_version": 7
        });
        let parsed = validate_plain_note(&plain).expect("plain Delta should be editable");
        assert_eq!(parsed.content, "first\nsecond");
        assert_eq!(parsed.version, 7);

        for unsafe_note in [
            json!({
                "list_type": "note",
                "note_content": "bold",
                "note_delta": "[{\"insert\":\"bold\",\"attributes\":{\"bold\":true}}]",
                "note_version": 8
            }),
            json!({
                "list_type": "note",
                "note_content": "",
                "note_delta": "[{\"insert\":{\"image\":\"x\"}},{\"insert\":\"\\n\"}]",
                "note_version": 9
            }),
            json!({
                "list_type": "note",
                "note_content": "Old",
                "note_delta": "[{\"insert\":\"Old \\n\"}]",
                "note_version": 10
            }),
        ] {
            assert!(validate_plain_note(&unsafe_note).is_err());
        }
    }

    #[test]
    fn builds_versioned_payload_and_preserves_clear() {
        let current = json!({
            "list_type": "note",
            "note_content": "Old",
            "note_delta": "[{\"insert\":\"Old\\n\"}]",
            "note_version": 4
        });
        let payload = safe_update_payload(&current, "", Map::new()).unwrap();
        assert_eq!(payload["note_delta"], "[{\"insert\":\"\\n\"}]");
        assert_eq!(payload["base_delta"], "[{\"insert\":\"Old\\n\"}]");
        assert_eq!(payload["base_version"], 4);
        assert!(payload["client_mutation_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("cli-")));
    }
}
