//! Privacy controls for crash/error telemetry and performance tracing.
//!
//! Telemetry follows the user's saved first-run preference or explicit env
//! overrides. Whatever is sent is scrubbed of credentials and payload details
//! first. We also disable Sentry's `send_default_pii`, so OS usernames, IP
//! addresses, and hostnames are not attached to events.

use crate::config::Config;
use sentry::protocol::{Event, SpanStatus, Value};

const SAFE_TAG_KEYS: &[&str] = &[
    "action",
    "api.method",
    "api.route",
    "api.status_class",
    "command",
    "error.category",
    "mode",
    "operation",
    "outcome",
    "view",
];

/// Returns `true` when telemetry should be active.
///
/// Honoured signals:
/// - `DO_NOT_TRACK` (cross-tool convention) truthy -> force disable
/// - `KRAMLI_NO_TELEMETRY` truthy -> force disable
/// - `KRAMLI_TELEMETRY` can explicitly enable/disable with
///   `1`/`true`/`on`/`yes` and `0`/`false`/`off`/`no`
///
/// Default: disabled until the user answers the first-run prompt.
pub fn is_enabled() -> bool {
    Config::load().telemetry_enabled()
}

pub fn traces_sample_rate() -> f32 {
    std::env::var("KRAMLI_TRACES_SAMPLE_RATE")
        .ok()
        .and_then(|raw| raw.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 1.0))
        .unwrap_or(1.0)
}

/// Ordinary CLI failures are usually expected user/API outcomes (not logged in,
/// not found, validation, network errors). Keep Sentry issues focused on
/// crashes by requiring an explicit opt-in before capturing command `Err`s as
/// error events. Panic/default integrations still report real crashes.
pub fn should_capture_command_error(_message: &str) -> bool {
    std::env::var("KRAMLI_CAPTURE_COMMAND_ERRORS")
        .ok()
        .and_then(|raw| parse_bool(&raw))
        .unwrap_or(false)
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
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
    let without_body = body_prefix(message);

    let mut out = String::with_capacity(without_body.len());
    let mut redact_next = false;
    for token in split_keep_delimiters(without_body) {
        if token.trim().is_empty() {
            out.push_str(token);
            continue;
        }
        if redact_next {
            if is_bearer_marker(token) {
                out.push_str(token);
                redact_next = true;
                continue;
            }
            let redacted = redact_token(token);
            if redacted.starts_with("kramli_[REDACTED]") {
                out.push_str(&redacted);
            } else {
                out.push_str("[REDACTED]");
            }
            redact_next = false;
            continue;
        }
        redact_next = marks_following_secret(token);
        out.push_str(&redact_token(token));
    }
    out
}

fn body_prefix(message: &str) -> &str {
    for (offset, _) in message.match_indices('\n') {
        let line = &message[offset + 1..].lines().next().unwrap_or_default();
        let lower = line.trim_start().to_ascii_lowercase();
        if lower.starts_with("body:") || lower.starts_with("body :") {
            return &message[..offset];
        }
    }
    message
}

fn marks_following_secret(token: &str) -> bool {
    let normalized = secret_marker(token);
    matches!(
        normalized.as_str(),
        "bearer"
            | "bearer:"
            | "bearer="
            | "token"
            | "secret"
            | "authorization"
            | "authorization:"
            | "authorization="
            | "authorization:bearer"
            | "authorization=bearer"
            | "api-key"
            | "apikey"
            | "key"
    ) || normalized.ends_with(":token")
        || normalized.ends_with(":secret")
        || normalized.ends_with(":authorization")
        || normalized.ends_with(":bearer")
}

fn is_bearer_marker(token: &str) -> bool {
    matches!(
        secret_marker(token).as_str(),
        "bearer" | "bearer:" | "bearer="
    )
}

fn secret_marker(token: &str) -> String {
    token
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_')
        .to_ascii_lowercase()
}

/// `before_send` hook: scrub the human-readable parts of an event.
pub fn scrub_event(mut event: Event<'static>) -> Option<Event<'static>> {
    // Keep only minimal diagnostics.
    event.culprit = None;
    event.transaction = event
        .transaction
        .take()
        .filter(|value| is_safe_metric_label(value));
    event.logger = None;
    event.modules.clear();
    event.server_name = None;
    event.environment = None;
    event.user = None;
    event.request = None;
    event.contexts.retain(|key, _| key == "trace");
    event.breadcrumbs.values.clear();
    event.stacktrace = None;
    event.template = None;
    event.threads.values.clear();
    event.tags.retain(|key, value| is_safe_tag(key, value));
    event.extra.clear();
    event.debug_meta = Default::default();
    event.sdk = None;

    if let Some(message) = event.message.take() {
        event.message = Some(scrub_message(&message));
    }
    if let Some(mut entry) = event.logentry.take() {
        entry.message = scrub_message(&entry.message);
        entry.params.clear();
        event.logentry = Some(entry);
    }
    for exception in &mut event.exception.values {
        if let Some(value) = exception.value.take() {
            exception.value = Some(scrub_message(&value));
        }
        exception.module = None;
        exception.stacktrace = None;
        exception.raw_stacktrace = None;
        exception.thread_id = None;
        exception.mechanism = None;
    }
    Some(event)
}

pub struct TraceTransaction {
    inner: Option<sentry::Transaction>,
    previous_span: Option<sentry::TransactionOrSpan>,
    finished: bool,
}

impl TraceTransaction {
    pub fn start(name: &'static str, op: &'static str) -> Self {
        if !is_enabled() {
            return Self {
                inner: None,
                previous_span: None,
                finished: true,
            };
        }
        let transaction = sentry::start_transaction(sentry::TransactionContext::new(name, op));
        let previous_span = sentry::configure_scope(|scope| {
            let previous = scope.get_span();
            scope.set_span(Some(transaction.clone().into()));
            previous
        });
        Self {
            inner: Some(transaction),
            previous_span,
            finished: false,
        }
    }

    pub fn set_tag(&self, key: &str, value: impl ToString) {
        if is_safe_tag(key, &value.to_string()) {
            if let Some(transaction) = &self.inner {
                transaction.set_tag(key, value);
            }
        }
    }

    pub fn set_data_i64(&self, key: &str, value: i64) {
        if let Some(transaction) = &self.inner {
            transaction.set_data(key, Value::from(value));
        }
    }

    pub fn finish(mut self, ok: bool) {
        self.finish_with_status(if ok {
            SpanStatus::Ok
        } else {
            SpanStatus::InternalError
        });
    }

    fn finish_with_status(&mut self, status: SpanStatus) {
        if self.finished {
            return;
        }
        if let Some(transaction) = self.inner.take() {
            transaction.set_status(status);
            transaction.finish();
        }
        let previous_span = self.previous_span.take();
        sentry::configure_scope(|scope| scope.set_span(previous_span));
        self.finished = true;
    }
}

impl Drop for TraceTransaction {
    fn drop(&mut self) {
        self.finish_with_status(SpanStatus::Cancelled);
    }
}

pub struct TraceSpan {
    inner: Option<sentry::Span>,
    finished: bool,
}

impl TraceSpan {
    pub fn child(op: &'static str, description: &'static str) -> Self {
        if !is_enabled() {
            return Self {
                inner: None,
                finished: true,
            };
        }
        let parent = sentry::configure_scope(|scope| scope.get_span());
        let inner = parent.map(|span| span.start_child(op, description));
        Self {
            inner,
            finished: false,
        }
    }

    pub fn set_tag(&self, key: &str, value: impl ToString) {
        if is_safe_tag(key, &value.to_string()) {
            if let Some(span) = &self.inner {
                span.set_tag(key, value);
            }
        }
    }

    pub fn set_data_i64(&self, key: &str, value: i64) {
        if let Some(span) = &self.inner {
            span.set_data(key, Value::from(value));
        }
    }

    pub fn set_status(&self, ok: bool) {
        if let Some(span) = &self.inner {
            span.set_status(if ok {
                SpanStatus::Ok
            } else {
                SpanStatus::InternalError
            });
        }
    }

    pub fn finish(mut self) {
        self.finish_with_status(None);
    }

    fn finish_with_status(&mut self, status: Option<SpanStatus>) {
        if self.finished {
            return;
        }
        if let Some(span) = self.inner.take() {
            if let Some(status) = status {
                span.set_status(status);
            }
            span.finish();
        }
        self.finished = true;
    }
}

impl Drop for TraceSpan {
    fn drop(&mut self) {
        self.finish_with_status(Some(SpanStatus::Cancelled));
    }
}

pub fn route_template(path: &str) -> String {
    let without_query = path.split('?').next().unwrap_or(path);
    let mut out = Vec::new();
    for segment in without_query
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        if segment.chars().all(|ch| ch.is_ascii_digit()) {
            out.push("{id}".to_string());
        } else if is_static_route_segment(segment) {
            out.push(segment.to_ascii_lowercase());
        } else {
            out.push("{value}".to_string());
        }
    }
    if out.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", out.join("/"))
    }
}

pub fn status_class(status: u16) -> String {
    format!("{}xx", status / 100)
}

fn is_safe_tag(key: &str, value: &str) -> bool {
    SAFE_TAG_KEYS.contains(&key) && is_safe_metric_label(value)
}

fn is_safe_metric_label(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 80
        && trimmed.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(ch, '.' | '_' | '-' | ':' | '/' | '{' | '}' | '|')
        })
}

fn is_static_route_segment(segment: &str) -> bool {
    matches!(
        segment,
        "accept-terms"
            | "accept"
            | "activity"
            | "api-keys"
            | "attachments"
            | "check-all"
            | "clear"
            | "clear-done"
            | "comments"
            | "continue"
            | "continue-on-device"
            | "done"
            | "folders"
            | "handoff"
            | "invite-link"
            | "invite-links"
            | "items"
            | "keys"
            | "leave"
            | "lists"
            | "login-ack"
            | "members"
            | "ping"
            | "profile"
            | "redo"
            | "search"
            | "security"
            | "share"
            | "sort"
            | "undo"
            | "unshare"
            | "upvote"
            | "viewing"
    )
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
    let lower = token.to_ascii_lowercase();
    for marker in [
        "authorization:bearer",
        "authorization=bearer",
        "token=",
        "secret=",
        "authorization=",
        "authorization:",
        "bearer:",
        "bearer=",
        "api_key=",
        "apikey=",
        "key=",
    ] {
        if let Some(pos) = lower.find(marker) {
            let end = pos + marker.len();
            let trailing = &token[end..];
            if marker == "authorization:" && trailing.eq_ignore_ascii_case("bearer") {
                continue;
            }
            if trailing
                .trim_matches(|ch: char| !ch.is_ascii_alphanumeric())
                .is_empty()
            {
                continue;
            }
            return format!("{}[REDACTED]", &token[..end]);
        }
    }
    if let Some(pos) = lower.find("kram.li/i/") {
        let keep = pos + "kram.li/i/".len();
        return format!("{}[REDACTED]", &token[..keep]);
    }
    if looks_like_jwt(token) {
        return "[REDACTED_TOKEN]".to_string();
    }
    // Email addresses (very light heuristic: contains '@' and a '.').
    if token.contains('@') && token.contains('.') {
        return "[REDACTED_EMAIL]".to_string();
    }
    token.to_string()
}

fn looks_like_jwt(token: &str) -> bool {
    let trimmed = token.trim_matches(|ch: char| {
        !ch.is_ascii_alphanumeric() && ch != '.' && ch != '-' && ch != '_'
    });
    let parts: Vec<&str> = trimmed.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            part.len() >= 8
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_env_bool;
    use sentry::protocol::{Event, Exception, LogEntry, Map};

    fn is_enabled_from_values(
        dnt: Option<&str>,
        no_telemetry: Option<&str>,
        telemetry: Option<&str>,
    ) -> bool {
        let env_value_is_truthy = |value: &str| {
            let v = value.trim().to_ascii_lowercase();
            !v.is_empty() && v != "0" && v != "false" && v != "off" && v != "no"
        };
        if dnt.map(env_value_is_truthy).unwrap_or(false)
            || no_telemetry.map(env_value_is_truthy).unwrap_or(false)
        {
            return false;
        }
        telemetry.and_then(parse_env_bool).unwrap_or(false)
    }

    #[test]
    fn telemetry_is_disabled_until_consent_or_env_enable() {
        assert!(!is_enabled_from_values(None, None, None));
        assert!(!is_enabled_from_values(None, None, Some("invalid")));
        assert!(!is_enabled_from_values(None, None, Some("0")));
        assert!(!is_enabled_from_values(None, None, Some("false")));
        assert!(is_enabled_from_values(None, None, Some("true")));
    }

    #[test]
    fn dnt_and_no_telemetry_override_explicit_enable() {
        assert!(!is_enabled_from_values(Some("1"), None, None));
        assert!(!is_enabled_from_values(Some("maybe"), None, None));
        assert!(!is_enabled_from_values(None, Some("yes"), None));
        assert!(!is_enabled_from_values(Some("1"), None, Some("true")));
        assert!(!is_enabled_from_values(None, Some("yes"), Some("true")));
    }

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
    fn redacts_common_secret_shapes() {
        let scrubbed = scrub_message(
            "token=abc123 Authorization: Bearer redaction-test-bearer-token https://kram.li/i/invite-secret",
        );
        assert!(scrubbed.contains("token=[REDACTED]"));
        assert!(scrubbed.contains("Bearer [REDACTED]"));
        assert!(scrubbed.contains("https://kram.li/i/[REDACTED]"));
        assert!(!scrubbed.contains("abc123"));
        assert!(!scrubbed.contains("invite-secret"));
        assert!(!scrubbed.contains("redaction-test-bearer-token"));
    }

    #[test]
    fn redacts_compact_authorization_headers() {
        let scrubbed = scrub_message(
            "Authorization:Bearer compact-secret Authorization:BearerInlineSecret bearer=another-secret",
        );
        assert!(scrubbed.contains("Authorization:Bearer [REDACTED]"));
        assert!(scrubbed.contains("Authorization:Bearer[REDACTED]"));
        assert!(scrubbed.contains("bearer=[REDACTED]"));
        assert!(!scrubbed.contains("compact-secret"));
        assert!(!scrubbed.contains("BearerInlineSecret"));
        assert!(!scrubbed.contains("another-secret"));
    }

    #[test]
    fn keeps_plain_messages() {
        assert_eq!(
            scrub_message("Network error: timeout"),
            "Network error: timeout"
        );
    }

    #[test]
    fn route_templates_drop_ids_tokens_and_query_values() {
        assert_eq!(
            route_template("/lists/123/items?search=milk"),
            "/lists/{id}/items"
        );
        assert_eq!(
            route_template("/invite-links/secret-token/accept"),
            "/invite-links/{value}/accept"
        );
        assert_eq!(route_template("/api-keys/123"), "/api-keys/{id}");
        assert_eq!(
            route_template("/lists/1/check-all"),
            "/lists/{id}/check-all"
        );
        assert_eq!(route_template("/security/login-ack"), "/security/login-ack");
        assert_eq!(route_template("/profile"), "/profile");
    }

    #[test]
    fn scrub_event_keeps_only_safe_trace_tags() {
        let event = Event {
            transaction: Some("cli.command".to_string()),
            tags: Map::from_iter([
                (String::from("command"), String::from("items")),
                (String::from("email"), String::from("user@example.com")),
                (String::from("api.route"), String::from("/lists/{id}/items")),
            ]),
            ..Event::default()
        };

        let scrubbed = scrub_event(event).expect("event should be kept");
        assert_eq!(scrubbed.transaction.as_deref(), Some("cli.command"));
        assert_eq!(
            scrubbed.tags.get("command").map(String::as_str),
            Some("items")
        );
        assert_eq!(
            scrubbed.tags.get("api.route").map(String::as_str),
            Some("/lists/{id}/items")
        );
        assert!(!scrubbed.tags.contains_key("email"));
    }

    #[test]
    fn scrub_event_drops_sensitive_structured_fields() {
        let mut event = Event {
            message: Some("Failed for user@example.com".to_string()),
            server_name: Some("my-host".into()),
            tags: Map::from_iter([(String::from("email"), String::from("user@example.com"))]),
            contexts: Map::from_iter([(
                String::from("os"),
                sentry::protocol::Context::Other(Map::from_iter([(
                    String::from("name"),
                    sentry::protocol::Value::from("macOS"),
                )])),
            )]),
            logentry: Some(LogEntry {
                message: "api key kramli_abc123".to_string(),
                params: vec![sentry::protocol::Value::from("secret")],
            }),
            ..Event::default()
        };
        event.exception.values.push(Exception {
            ty: "error".to_string(),
            value: Some("boom for user@example.com".to_string()),
            module: Some("sensitive.module".to_string()),
            ..Exception::default()
        });

        let scrubbed = scrub_event(event).expect("event should be kept");

        assert_eq!(
            scrubbed.message.as_deref(),
            Some("Failed for [REDACTED_EMAIL]")
        );
        assert!(scrubbed.server_name.is_none());
        assert!(scrubbed.tags.is_empty());
        assert!(scrubbed.contexts.is_empty());
        assert!(scrubbed.user.is_none());
        assert!(scrubbed.request.is_none());

        let log = scrubbed.logentry.expect("logentry should exist");
        assert_eq!(log.message, "api key kramli_[REDACTED]");
        assert!(log.params.is_empty());

        let exc = &scrubbed.exception.values[0];
        assert_eq!(exc.value.as_deref(), Some("boom for [REDACTED_EMAIL]"));
        assert!(exc.module.is_none());
        assert!(exc.stacktrace.is_none());
        assert!(exc.raw_stacktrace.is_none());
        assert!(exc.thread_id.is_none());
        assert!(exc.mechanism.is_none());
    }
}
