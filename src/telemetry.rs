//! Privacy controls for crash/error telemetry.
//!
//! Telemetry is opt-out: users can disable it entirely, and whatever is sent
//! is scrubbed of credentials and response bodies first. We never enable
//! Sentry's `send_default_pii`, so OS usernames, IP addresses, and hostnames
//! are not attached to events.

use sentry::protocol::Event;

/// Returns `false` when the user has opted out of telemetry.
///
/// Honoured signals (any truthy value disables telemetry):
/// - `DO_NOT_TRACK` (the cross-tool convention, <https://consoledonottrack.com/>)
/// - `KRAMLI_NO_TELEMETRY`
/// - `KRAMLI_TELEMETRY` set to a falsy value (`0`/`false`/`off`/`no`)
pub fn is_enabled() -> bool {
    if env_is_truthy("DO_NOT_TRACK") || env_is_truthy("KRAMLI_NO_TELEMETRY") {
        return false;
    }
    if let Ok(raw) = std::env::var("KRAMLI_TELEMETRY") {
        match raw.trim().to_ascii_lowercase().as_str() {
            "0" | "false" | "off" | "no" => return false,
            _ => {}
        }
    }
    true
}

fn env_is_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(raw) => {
            let v = raw.trim().to_ascii_lowercase();
            !v.is_empty() && v != "0" && v != "false" && v != "off" && v != "no"
        }
        Err(_) => false,
    }
}

/// Remove credentials and response payloads from a free-text error/message
/// before it is sent to telemetry.
///
/// Specifically:
/// - drops everything after a `Body:` marker (raw API response bodies that may
///   contain emails, list/item contents, etc.),
/// - redacts `kramli_…` API keys, and
/// - redacts email addresses.
pub fn scrub_message(message: &str) -> String {
    // 1. Drop raw response bodies appended by the API client (e.g.
    //    "Could not parse response: …\nBody: {…}").
    let without_body = match message.find("\nBody:") {
        Some(idx) => &message[..idx],
        None => message,
    };

    let mut out = String::with_capacity(without_body.len());
    for token in split_keep_delimiters(without_body) {
        out.push_str(&redact_token(token));
    }
    out
}

/// `before_send` hook: scrub the human-readable parts of an event.
pub fn scrub_event(mut event: Event<'static>) -> Option<Event<'static>> {
    if let Some(message) = event.message.take() {
        event.message = Some(scrub_message(&message));
    }
    for exception in &mut event.exception.values {
        if let Some(value) = exception.value.take() {
            exception.value = Some(scrub_message(&value));
        }
    }
    Some(event)
}

/// Split on ASCII whitespace while keeping the delimiters, so reconstruction
/// preserves the original spacing.
fn split_keep_delimiters(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let bytes = input.as_bytes();
    let mut start = 0;
    let mut in_ws = false;
    for (idx, &b) in bytes.iter().enumerate() {
        let is_ws = b.is_ascii_whitespace();
        if idx == 0 {
            in_ws = is_ws;
            continue;
        }
        if is_ws != in_ws {
            parts.push(&input[start..idx]);
            start = idx;
            in_ws = is_ws;
        }
    }
    if start < input.len() {
        parts.push(&input[start..]);
    }
    parts
}

fn redact_token(token: &str) -> String {
    if token.trim().is_empty() {
        return token.to_string();
    }
    // API keys.
    if let Some(pos) = token.find("kramli_") {
        let (prefix, _rest) = token.split_at(pos);
        return format!("{prefix}kramli_[REDACTED]");
    }
    // Email addresses (very light heuristic: contains '@' and a '.').
    if token.contains('@') && token.contains('.') {
        return "[REDACTED_EMAIL]".to_string();
    }
    token.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_response_body() {
        let msg = "Could not parse response: expected value\nBody: {\"email\":\"a@b.com\"}";
        let scrubbed = scrub_message(msg);
        assert!(!scrubbed.contains("a@b.com"));
        assert!(!scrubbed.contains("Body:"));
        assert!(scrubbed.starts_with("Could not parse response:"));
    }

    #[test]
    fn redacts_api_key() {
        let msg = "Invalid API key: kramli_abcDEF123456 rejected";
        let scrubbed = scrub_message(msg);
        assert!(scrubbed.contains("kramli_[REDACTED]"));
        assert!(!scrubbed.contains("abcDEF123456"));
    }

    #[test]
    fn redacts_email() {
        let scrubbed = scrub_message("login failed for user@example.com today");
        assert!(scrubbed.contains("[REDACTED_EMAIL]"));
        assert!(!scrubbed.contains("user@example.com"));
    }

    #[test]
    fn keeps_plain_messages() {
        assert_eq!(
            scrub_message("Network error: timeout"),
            "Network error: timeout"
        );
    }
}
