//! Privacy controls for crash/error telemetry and performance tracing.
//!
//! Telemetry follows the user's saved first-run preference or explicit env
//! overrides. Whatever is sent is scrubbed of credentials and payload details
//! first. We also disable Sentry's `send_default_pii`, so OS usernames, IP
//! addresses, and hostnames are not attached to events.

use crate::config::Config;
use sentry::protocol::{Context, Event, Frame, Map, SpanStatus, Stacktrace, Value};
use std::path::Path;
use std::sync::Mutex;

const SAFE_TAG_KEYS: &[&str] = &[
    "action",
    "api.method",
    "api.route",
    "api.status_class",
    "api.upload",
    "command",
    "error.category",
    "mode",
    "operation",
    "outcome",
    "surface",
    "view",
];
const SAFE_EXTRA_KEYS: &[&str] = &[
    "api.method",
    "api.route",
    "api.status_class",
    "cli.version",
    "command",
    "error.category",
    "mode",
    "outcome",
    "surface",
];
const SAFE_CONTEXT_KEYS: &[&str] = &["cli", "runtime", "trace"];
const KRAMLI_TRACES_SAMPLE_RATE_ENV: &str = "KRAMLI_TRACES_SAMPLE_RATE";
const KRAMLI_CAPTURE_COMMAND_ERRORS_ENV: &str = "KRAMLI_CAPTURE_COMMAND_ERRORS";

#[derive(Clone, Debug, Default)]
struct HttpCallContext {
    method: Option<String>,
    route: Option<String>,
    status_class: Option<String>,
}

static LAST_HTTP_CALL: Mutex<Option<HttpCallContext>> = Mutex::new(None);

/// Returns `true` when telemetry should be active.
///
/// Honoured signals:
/// - `DO_NOT_TRACK` (cross-tool convention) truthy -> force disable
/// - `KRAMLI_NO_TELEMETRY` truthy -> force disable
/// - `KRAMLI_TELEMETRY` can explicitly enable/disable with
///   `1`/`true`/`on`/`yes` and `0`/`false`/`off`/`no`
///
/// Default: disabled until the user answers the first-run prompt.
pub(crate) fn is_enabled() -> bool {
    Config::load().telemetry_enabled()
}

/// Return the Sentry trace sampling rate from environment configuration.
pub(crate) fn traces_sample_rate() -> f32 {
    std::env::var(KRAMLI_TRACES_SAMPLE_RATE_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .map_or(1.0, |value| value.clamp(0.0, 1.0))
}

/// Remember the most recent API call for error diagnostics (no secrets).
pub(crate) fn record_http_call(method: &str, path: &str, status: u16) {
    let Ok(mut guard) = LAST_HTTP_CALL.lock() else {
        return;
    };
    *guard = Some(HttpCallContext {
        method: Some(method.to_ascii_uppercase()),
        route: Some(route_template(path)),
        status_class: Some(status_class(status)),
    });
}

/// Classify a CLI error message into a low-cardinality category.
pub(crate) fn classify_command_error(message: &str) -> &'static str {
    let blob = message.to_ascii_lowercase();
    if blob.contains("not logged in")
        || blob.contains("nicht angemeldet")
        || blob.contains("kramli_api_key")
        || blob.contains("unauthorized")
        || blob.contains("401")
    {
        "auth"
    } else if blob.contains("listenreferenz ist leer") || blob.contains("list reference is empty") {
        "validation"
    } else if blob.contains("not found")
        || blob.contains("nicht gefunden")
        || blob.contains("404")
        || blob.contains("unknown list")
    {
        "not_found"
    } else if blob.contains("timeout")
        || blob.contains("network")
        || blob.contains("connection")
        || blob.contains("dns")
        || blob.contains("offline")
        || blob.contains("tls")
    {
        "network"
    } else if blob.contains("api-error") || blob.contains("api error") || blob.contains("http") {
        "api"
    } else {
        "internal"
    }
}

/// Capture a command failure with scrubbed text plus safe diagnostic context.
pub(crate) fn capture_command_error(message: &str) {
    let scrubbed = scrub_message(message);
    let category = classify_command_error(message);
    sentry::configure_scope(|scope| {
        scope.set_tag("error.category", category);
        scope.set_tag("outcome", "error");
        scope.set_context("cli", Context::Other(build_cli_context(category, message)));
        if let Ok(guard) = LAST_HTTP_CALL.lock() {
            if let Some(call) = guard.as_ref() {
                if let Some(method) = &call.method {
                    scope.set_tag("api.method", method.as_str());
                }
                if let Some(route) = &call.route {
                    scope.set_tag("api.route", route.as_str());
                }
                if let Some(status_class) = &call.status_class {
                    scope.set_tag("api.status_class", status_class.as_str());
                }
            }
        }
    });
    sentry::capture_message(&scrubbed, sentry::Level::Error);
}

fn build_cli_context(category: &str, message: &str) -> Map<String, Value> {
    let mut context = Map::new();
    context.insert("version".into(), Value::from(env!("CARGO_PKG_VERSION")));
    context.insert("error.category".into(), Value::from(category));
    context.insert(
        "error.summary".into(),
        Value::from(scrub_message(message).chars().take(240).collect::<String>()),
    );
    if let Ok(guard) = LAST_HTTP_CALL.lock() {
        if let Some(call) = guard.as_ref() {
            if let Some(method) = &call.method {
                context.insert("api.method".into(), Value::from(method.clone()));
            }
            if let Some(route) = &call.route {
                context.insert("api.route".into(), Value::from(route.clone()));
            }
            if let Some(status_class) = &call.status_class {
                context.insert("api.status_class".into(), Value::from(status_class.clone()));
            }
        }
    }
    context
}

/// Ordinary CLI failures are usually expected user/API outcomes (not logged in,
/// not found, validation, network errors). Keep Sentry issues focused on
/// crashes by requiring an explicit opt-in before capturing command `Err`s as
/// error events. Panic/default integrations still report real crashes.
pub(crate) fn should_capture_command_error(_message: &str) -> bool {
    std::env::var(KRAMLI_CAPTURE_COMMAND_ERRORS_ENV)
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
pub(crate) fn scrub_message(message: &str) -> String {
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

fn event_text_blob(event: &Event<'_>) -> String {
    let mut parts = Vec::new();
    if let Some(message) = &event.message {
        parts.push(message.as_str());
    }
    if let Some(entry) = &event.logentry {
        parts.push(entry.message.as_str());
    }
    for exception in &event.exception.values {
        if let Some(value) = &exception.value {
            parts.push(value.as_str());
        }
        parts.push(exception.ty.as_str());
    }
    parts.join(" ").to_ascii_lowercase()
}

/// Drop expected CLI outcomes and pipe-closed panics before scrubbing.
fn should_drop_event(event: &Event<'_>) -> bool {
    let blob = event_text_blob(event);
    if blob.contains("broken pipe") {
        return true;
    }
    if blob.contains("not logged in")
        || blob.contains("nicht angemeldet")
        || blob.contains("kramli login")
        || blob.contains("kramli_api_key")
    {
        return true;
    }
    if blob.contains("listenreferenz ist leer") || blob.contains("list reference is empty") {
        return true;
    }
    false
}

/// `before_send` hook: scrub the human-readable parts of an event.
pub(crate) fn scrub_event(mut event: Event<'static>) -> Option<Event<'static>> {
    if should_drop_event(&event) {
        return None;
    }
    event.culprit = event
        .culprit
        .take()
        .filter(|value| is_safe_metric_label(value));
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
    event.breadcrumbs.values.clear();
    event.template = None;
    event.threads.values.clear();
    event.debug_meta = std::borrow::Cow::default();
    event.sdk = None;

    event
        .contexts
        .retain(|key, _| SAFE_CONTEXT_KEYS.contains(&key.as_str()));
    sanitize_contexts(&mut event.contexts);

    event.tags.retain(|key, value| is_safe_tag(key, value));
    event
        .extra
        .retain(|key, value| is_safe_extra_key(key) && is_safe_extra_value(value));

    if let Some(stacktrace) = event.stacktrace.as_mut() {
        sanitize_stacktrace(stacktrace);
    }

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
        if let Some(module) = exception.module.take() {
            exception.module = Some(sanitize_module_path(&module));
        }
        if let Some(stacktrace) = exception.stacktrace.as_mut() {
            sanitize_stacktrace(stacktrace);
        }
        exception.raw_stacktrace = None;
        exception.thread_id = None;
    }
    Some(event)
}

/// RAII wrapper around a Sentry transaction.
pub(crate) struct TraceTransaction {
    inner: Option<sentry::Transaction>,
    previous_span: Option<sentry::TransactionOrSpan>,
    finished: bool,
}

impl TraceTransaction {
    /// Start a transaction and install it as the active span.
    pub(crate) fn start(name: &'static str, op: &'static str) -> Self {
        Self::start_with_enabled(name, op, is_enabled())
    }

    fn start_with_enabled(name: &'static str, op: &'static str, enabled: bool) -> Self {
        if !enabled {
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

    /// Attach a safe low-cardinality tag to the transaction and active scope.
    pub(crate) fn set_tag(&self, key: &str, value: impl ToString) {
        let value = value.to_string();
        if is_safe_tag(key, &value) {
            if let Some(transaction) = &self.inner {
                transaction.set_tag(key, &value);
            }
            sentry::configure_scope(|scope| scope.set_tag(key, &value));
        }
    }

    /// Attach integer measurement data to the transaction.
    pub(crate) fn set_data_i64(&self, key: &str, value: i64) {
        if let Some(transaction) = &self.inner {
            transaction.set_data(key, Value::from(value));
        }
    }

    /// Finish the transaction with an OK or error status.
    pub(crate) fn finish(mut self, ok: bool) {
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

/// RAII wrapper around a child Sentry span.
pub(crate) struct TraceSpan {
    inner: Option<sentry::Span>,
    finished: bool,
}

impl TraceSpan {
    /// Start a child span under the active transaction or span.
    pub(crate) fn child(op: &'static str, description: &'static str) -> Self {
        Self::child_with_enabled(op, description, is_enabled())
    }

    fn child_with_enabled(op: &'static str, description: &'static str, enabled: bool) -> Self {
        if !enabled {
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

    /// Attach a safe low-cardinality tag to the span.
    pub(crate) fn set_tag(&self, key: &str, value: impl ToString) {
        if is_safe_tag(key, &value.to_string()) {
            if let Some(span) = &self.inner {
                span.set_tag(key, value);
            }
        }
    }

    /// Attach integer measurement data to the span.
    pub(crate) fn set_data_i64(&self, key: &str, value: i64) {
        if let Some(span) = &self.inner {
            span.set_data(key, Value::from(value));
        }
    }

    /// Mark the span as successful or failed.
    pub(crate) fn set_status(&self, ok: bool) {
        if let Some(span) = &self.inner {
            span.set_status(if ok {
                SpanStatus::Ok
            } else {
                SpanStatus::InternalError
            });
        }
    }

    /// Finish the span successfully unless it was already finished.
    pub(crate) fn finish(mut self) {
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

/// Convert an API path into a low-cardinality route template.
pub(crate) fn route_template(path: &str) -> String {
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

/// Convert an HTTP status code to a `2xx`-style class label.
pub(crate) fn status_class(status: u16) -> String {
    format!("{}xx", status / 100)
}

fn is_safe_tag(key: &str, value: &str) -> bool {
    SAFE_TAG_KEYS.contains(&key) && is_safe_metric_label(value)
}

fn is_safe_extra_key(key: &str) -> bool {
    SAFE_EXTRA_KEYS.contains(&key)
}

fn is_safe_extra_value(value: &Value) -> bool {
    match value {
        Value::String(text) => is_safe_metric_label(text),
        Value::Bool(_) | Value::Number(_) => true,
        _ => false,
    }
}

fn sanitize_module_path(module: &str) -> String {
    module.rsplit("::").next().unwrap_or(module).to_string()
}

fn sanitize_path_filename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

fn sanitize_stacktrace(stacktrace: &mut Stacktrace) {
    for frame in &mut stacktrace.frames {
        sanitize_frame(frame);
    }
}

fn sanitize_frame(frame: &mut Frame) {
    if let Some(abs_path) = frame.abs_path.take() {
        frame.abs_path = Some(sanitize_path_filename(&abs_path));
    }
    if let Some(filename) = frame.filename.take() {
        frame.filename = Some(sanitize_path_filename(&filename));
    }
    frame.vars.clear();
}

fn sanitize_contexts(contexts: &mut Map<String, Context>) {
    for (key, context) in contexts.iter_mut() {
        if key != "cli" {
            continue;
        }
        let Context::Other(values) = context else {
            continue;
        };
        values.retain(|field, value| match field.as_str() {
            "version" | "error.category" | "api.method" | "api.route" | "api.status_class" => {
                is_safe_extra_value(value)
            }
            "error.summary" => value
                .as_str()
                .is_some_and(|text| text.len() <= 240 && !text.contains('@')),
            _ => false,
        });
        for value in values.values_mut() {
            if let Value::String(text) = value {
                *text = scrub_message(text);
            }
        }
    }
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
    use sentry::protocol::{Context, Event, Exception, Frame, LogEntry, Map, Stacktrace, Value};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    const TEST_KRAMLI_TELEMETRY_ENV: &str = "KRAMLI_TELEMETRY";

    fn with_env_var<T>(key: &str, value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().expect("telemetry env lock poisoned");
        with_env_var_locked(key, value, f)
    }

    fn with_env_var_locked<T>(key: &str, value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let previous = std::env::var(key).ok();
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
        let result = f();
        match previous {
            Some(previous) => std::env::set_var(key, previous),
            None => std::env::remove_var(key),
        }
        result
    }

    fn is_enabled_from_values(
        dnt: Option<&str>,
        no_telemetry: Option<&str>,
        telemetry: Option<&str>,
    ) -> bool {
        let env_value_is_truthy = |value: &str| {
            let v = value.trim().to_ascii_lowercase();
            !v.is_empty() && v != "0" && v != "false" && v != "off" && v != "no"
        };
        if dnt.is_some_and(env_value_is_truthy) || no_telemetry.is_some_and(env_value_is_truthy) {
            return false;
        }
        telemetry.and_then(parse_env_bool).unwrap_or(false)
    }

    #[kramli_test_macros::test]
    fn telemetry_is_disabled_until_consent_or_env_enable() {
        assert!(!is_enabled_from_values(None, None, None));
        assert!(!is_enabled_from_values(None, None, Some("invalid")));
        assert!(!is_enabled_from_values(None, None, Some("0")));
        assert!(!is_enabled_from_values(None, None, Some("false")));
        assert!(is_enabled_from_values(None, None, Some("true")));
    }

    #[kramli_test_macros::test]
    fn dnt_and_no_telemetry_override_explicit_enable() {
        assert!(!is_enabled_from_values(Some("1"), None, None));
        assert!(!is_enabled_from_values(Some("maybe"), None, None));
        assert!(!is_enabled_from_values(None, Some("yes"), None));
        assert!(!is_enabled_from_values(Some("1"), None, Some("true")));
        assert!(!is_enabled_from_values(None, Some("yes"), Some("true")));
    }

    #[kramli_test_macros::test]
    fn telemetry_env_helpers_parse_sampling_and_error_capture() {
        with_env_var(KRAMLI_TRACES_SAMPLE_RATE_ENV, None, || {
            assert_eq!(traces_sample_rate(), 1.0);
        });
        with_env_var(KRAMLI_TRACES_SAMPLE_RATE_ENV, Some("0.25"), || {
            assert_eq!(traces_sample_rate(), 0.25);
        });
        with_env_var(KRAMLI_TRACES_SAMPLE_RATE_ENV, Some("-3"), || {
            assert_eq!(traces_sample_rate(), 0.0);
        });
        with_env_var(KRAMLI_TRACES_SAMPLE_RATE_ENV, Some("3"), || {
            assert_eq!(traces_sample_rate(), 1.0);
        });
        with_env_var(KRAMLI_TRACES_SAMPLE_RATE_ENV, Some("nan"), || {
            assert_eq!(traces_sample_rate(), 1.0);
        });

        with_env_var(KRAMLI_CAPTURE_COMMAND_ERRORS_ENV, None, || {
            assert!(!should_capture_command_error("boom"));
        });
        with_env_var(KRAMLI_CAPTURE_COMMAND_ERRORS_ENV, Some("yes"), || {
            assert!(should_capture_command_error("boom"));
        });
        with_env_var(KRAMLI_CAPTURE_COMMAND_ERRORS_ENV, Some("off"), || {
            assert!(!should_capture_command_error("boom"));
        });
        with_env_var(KRAMLI_CAPTURE_COMMAND_ERRORS_ENV, Some("maybe"), || {
            assert!(!should_capture_command_error("boom"));
        });
    }

    #[kramli_test_macros::test]
    fn telemetry_env_helper_restores_existing_values() {
        let _guard = ENV_LOCK.lock().expect("telemetry env lock poisoned");
        std::env::set_var(KRAMLI_TRACES_SAMPLE_RATE_ENV, "0.5");

        with_env_var_locked(KRAMLI_TRACES_SAMPLE_RATE_ENV, Some("0.25"), || {
            assert_eq!(
                std::env::var(KRAMLI_TRACES_SAMPLE_RATE_ENV).as_deref(),
                Ok("0.25")
            );
        });

        assert_eq!(
            std::env::var(KRAMLI_TRACES_SAMPLE_RATE_ENV).as_deref(),
            Ok("0.5")
        );
        std::env::remove_var(KRAMLI_TRACES_SAMPLE_RATE_ENV);
    }

    #[kramli_test_macros::test]
    fn drops_response_body() {
        let msg = "Could not parse response: expected value\nBody: {\"email\":\"a@b.com\"}";
        let scrubbed = scrub_message(msg);
        assert!(!scrubbed.contains("a@b.com"));
        assert!(!scrubbed.contains("Body:"));
        assert!(scrubbed.starts_with("Could not parse response:"));
    }

    #[kramli_test_macros::test]
    fn drops_response_body_with_spaced_marker() {
        let msg = "Failed\n  Body : sensitive@example.com";
        assert_eq!(scrub_message(msg), "Failed");
        assert_eq!(body_prefix("Failed\nBody: sensitive@example.com"), "Failed");
    }

    #[kramli_test_macros::test]
    fn redaction_helpers_cover_empty_tokens_and_jwts() {
        assert_eq!(redact_token("   "), "   ");

        let jwt = "abcdefgh.ijklmnop.qrstuvwx";
        assert_eq!(redact_token(jwt), "[REDACTED_TOKEN]");
        assert!(looks_like_jwt(jwt));
        assert!(!looks_like_jwt("short.parts.no"));
    }

    #[kramli_test_macros::test]
    fn redacts_api_key() {
        let msg = "Invalid API key: kramli_abcDEF123456 rejected";
        let scrubbed = scrub_message(msg);
        assert!(scrubbed.contains("kramli_[REDACTED]"));
        assert!(!scrubbed.contains("abcDEF123456"));
    }

    #[kramli_test_macros::test]
    fn redacts_email() {
        let scrubbed = scrub_message("login failed for user@example.com today");
        assert!(scrubbed.contains("[REDACTED_EMAIL]"));
        assert!(!scrubbed.contains("user@example.com"));
    }

    #[kramli_test_macros::test]
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

    #[kramli_test_macros::test]
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

    #[kramli_test_macros::test]
    fn keeps_plain_messages() {
        assert_eq!(
            scrub_message("Network error: timeout"),
            "Network error: timeout"
        );
    }

    #[kramli_test_macros::test]
    fn route_templates_drop_ids_tokens_and_query_values() {
        assert_eq!(route_template("/"), "/");
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
        assert_eq!(status_class(204), "2xx");
        assert_eq!(status_class(503), "5xx");
    }

    #[kramli_test_macros::test]
    fn disabled_trace_wrappers_are_inert() {
        let transaction = TraceTransaction::start("test.transaction", "test");
        transaction.finish(true);

        let transaction = TraceTransaction::start_with_enabled("test.transaction", "test", false);
        transaction.set_tag("command", "status");
        transaction.set_tag("email", "user@example.com");
        transaction.set_data_i64("items", 3);
        transaction.finish(true);

        let transaction = TraceTransaction::start_with_enabled("test.transaction", "test", false);
        transaction.finish(false);

        let span = TraceSpan::child_with_enabled("test", "child", false);
        span.set_tag("operation", "api");
        span.set_tag("email", "user@example.com");
        span.set_data_i64("count", 2);
        span.set_status(true);
        span.finish();

        let span = TraceSpan::child_with_enabled("test", "child", false);
        span.set_status(false);
    }

    #[kramli_test_macros::test]
    fn enabled_trace_wrappers_cover_active_scope_paths() {
        with_env_var(TEST_KRAMLI_TELEMETRY_ENV, Some("true"), || {
            {
                let transaction =
                    TraceTransaction::start_with_enabled("test.transaction", "test", true);
                transaction.set_tag("command", "status");
                transaction.set_tag("email", "user@example.com");
                transaction.set_data_i64("items", 3);

                let span = TraceSpan::child_with_enabled("test.child", "child", true);
                span.set_tag("operation", "api");
                span.set_tag("email", "user@example.com");
                span.set_data_i64("count", 2);
                span.set_status(false);
                span.finish();

                transaction.finish(false);
            }

            let _dropped_transaction =
                TraceTransaction::start_with_enabled("test.transaction", "test", true);
            let _dropped_span = TraceSpan::child_with_enabled("test.child", "child", true);
        });
    }

    #[kramli_test_macros::test]
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

    #[kramli_test_macros::test]
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
            module: Some("kramli_cli::secrets".to_string()),
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
        assert_eq!(exc.module.as_deref(), Some("secrets"));
        assert!(exc.raw_stacktrace.is_none());
        assert!(exc.thread_id.is_none());
    }

    #[kramli_test_macros::test]
    fn classify_command_error_buckets_expected_outcomes() {
        assert_eq!(
            classify_command_error("Not logged in. Run `kramli login`."),
            "auth"
        );
        assert_eq!(
            classify_command_error("Listenreferenz ist leer."),
            "validation"
        );
        assert_eq!(classify_command_error("List not found"), "not_found");
        assert_eq!(classify_command_error("network timeout"), "network");
        assert_eq!(classify_command_error("API-Error 500"), "api");
        assert_eq!(classify_command_error("unexpected panic"), "internal");
    }

    #[kramli_test_macros::test]
    fn scrub_event_preserves_safe_cli_context_and_stacktrace() {
        record_http_call("GET", "/lists/42/items", 404);
        let mut event = Event {
            message: Some("List not found".to_string()),
            tags: Map::from_iter([
                (String::from("command"), String::from("items")),
                (String::from("error.category"), String::from("not_found")),
            ]),
            contexts: Map::from_iter([(
                String::from("cli"),
                Context::Other(Map::from_iter([
                    (
                        String::from("version"),
                        Value::from(env!("CARGO_PKG_VERSION")),
                    ),
                    (String::from("error.summary"), Value::from("List not found")),
                    (String::from("api.method"), Value::from("GET")),
                    (String::from("api.route"), Value::from("/lists/{id}/items")),
                    (String::from("api.status_class"), Value::from("4xx")),
                ])),
            )]),
            stacktrace: Some(Stacktrace {
                frames: vec![Frame {
                    abs_path: Some("/Users/me/kramli-cli/src/api.rs".to_string()),
                    filename: Some("api.rs".to_string()),
                    ..Frame::default()
                }],
                ..Stacktrace::default()
            }),
            ..Event::default()
        };
        event.exception.values.push(Exception {
            ty: "error".to_string(),
            value: Some("List not found".to_string()),
            module: Some("kramli_cli::api".to_string()),
            stacktrace: Some(Stacktrace {
                frames: vec![Frame {
                    abs_path: Some("/secret/home/api.rs".to_string()),
                    vars: Map::from_iter([(
                        String::from("email"),
                        Value::from("user@example.com"),
                    )]),
                    ..Frame::default()
                }],
                ..Stacktrace::default()
            }),
            ..Exception::default()
        });

        let scrubbed = scrub_event(event).expect("event should be kept");
        let cli = scrubbed
            .contexts
            .get("cli")
            .expect("cli context should remain");
        let Context::Other(values) = cli else {
            panic!("cli context should be a map");
        };
        assert_eq!(
            values.get("api.route").and_then(Value::as_str),
            Some("/lists/{id}/items")
        );
        assert_eq!(
            scrubbed
                .stacktrace
                .as_ref()
                .and_then(|trace| trace.frames.first())
                .and_then(|frame| frame.filename.as_deref()),
            Some("api.rs")
        );
        let exc = &scrubbed.exception.values[0];
        assert_eq!(exc.module.as_deref(), Some("api"));
        assert!(exc.stacktrace.is_some());
        let frame = exc
            .stacktrace
            .as_ref()
            .and_then(|trace| trace.frames.first())
            .expect("exception stacktrace frame");
        assert!(frame.vars.is_empty());
        assert_eq!(frame.abs_path.as_deref(), Some("api.rs"));
    }

    #[kramli_test_macros::test]
    fn scrub_event_drops_expected_cli_outcomes() {
        for message in [
            "Not logged in. Run `kramli login` or set KRAMLI_API_KEY.",
            "Nicht angemeldet. `kramli login` ausführen oder KRAMLI_API_KEY setzen.",
            "Listenreferenz ist leer.",
            "panic: failed printing to stdout: Broken pipe (os error 32)",
        ] {
            let event = Event {
                message: Some(message.to_string()),
                ..Event::default()
            };
            assert!(scrub_event(event).is_none(), "expected drop for {message}");
        }
    }

    #[kramli_test_macros::test]
    fn scrub_event_keeps_unexpected_failures() {
        let event = Event {
            message: Some("unexpected internal failure".to_string()),
            ..Event::default()
        };
        assert!(scrub_event(event).is_some());
    }

    #[kramli_test_macros::test]
    fn body_prefix_strips_response_body_lines() {
        assert_eq!(
            body_prefix("Request failed\nBody: {\"secret\":\"value\"}"),
            "Request failed"
        );
        assert_eq!(body_prefix("No body here"), "No body here");
    }

    #[kramli_test_macros::test]
    fn trace_span_applies_tags_data_and_status_with_active_sentry() {
        let _guard = sentry::init((
            "https://9435ede2d0d8eceedf3b3e0eb5cb6aff@o4509985277018112.ingest.de.sentry.io/4510966154002512",
            sentry::ClientOptions {
                release: sentry::release_name!(),
                send_default_pii: false,
                attach_stacktrace: false,
                max_breadcrumbs: 0,
                default_integrations: false,
                auto_session_tracking: false,
                traces_sample_rate: 1.0,
                before_send: Some(std::sync::Arc::new(scrub_event)),
                ..sentry::ClientOptions::default()
            },
        ));

        let transaction =
            TraceTransaction::start_with_enabled("test.transaction", "test.transaction", true);
        let span = TraceSpan::child_with_enabled("test.child", "child", true);
        span.set_tag("command", "status");
        span.set_data_i64("count", 1);
        span.set_status(true);
        span.finish();
        transaction.finish(true);
    }

    #[kramli_test_macros::test]
    fn trace_span_drop_applies_cancelled_status_with_active_sentry() {
        let _guard = sentry::init((
            "https://9435ede2d0d8eceedf3b3e0eb5cb6aff@o4509985277018112.ingest.de.sentry.io/4510966154002512",
            sentry::ClientOptions {
                release: sentry::release_name!(),
                send_default_pii: false,
                attach_stacktrace: false,
                max_breadcrumbs: 0,
                default_integrations: false,
                auto_session_tracking: false,
                traces_sample_rate: 1.0,
                before_send: Some(std::sync::Arc::new(scrub_event)),
                ..sentry::ClientOptions::default()
            },
        ));

        let transaction =
            TraceTransaction::start_with_enabled("test.transaction", "test.transaction", true);
        {
            let span = TraceSpan::child_with_enabled("test.child", "child", true);
            span.set_tag("operation", "demo");
            span.set_data_i64("count", 1);
            span.set_status(false);
        }
        transaction.finish(true);
    }

    #[kramli_test_macros::test]
    fn trace_span_helpers_are_safe_without_active_client() {
        let span = TraceSpan::child("test.child", "test.child");
        span.set_tag("operation", "demo");
        span.set_tag("bad tag!", "value");
        span.set_data_i64("count", 3);
        span.set_status(true);
        span.finish();
    }

    #[kramli_test_macros::test]
    fn body_prefix_stops_at_first_body_marker_in_multiline_messages() {
        assert_eq!(
            body_prefix("First line\nSecond\nBody: secret"),
            "First line\nSecond"
        );
    }

    #[kramli_test_macros::test]
    fn trace_span_drop_finishes_unfinished_spans_as_cancelled() {
        with_env_var(TEST_KRAMLI_TELEMETRY_ENV, Some("true"), || {
            let span = TraceSpan::child_with_enabled("test.child", "child", true);
            span.set_tag("operation", "demo");
            span.set_data_i64("count", 1);
            span.set_status(false);
        });
    }

    #[kramli_test_macros::test]
    fn trace_transaction_finish_handles_error_capture_branches() {
        with_env_var(KRAMLI_CAPTURE_COMMAND_ERRORS_ENV, Some("yes"), || {
            let tx = TraceTransaction::start("test.command", "test.command");
            tx.set_tag("command", "demo");
            tx.set_data_i64("cli.has_command", 0);
            tx.finish(false);
        });
    }
}
