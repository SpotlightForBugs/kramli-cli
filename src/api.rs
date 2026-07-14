use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_LANGUAGE, RETRY_AFTER};
use reqwest::{Client, Response, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::config::Config;
use crate::i18n::{current_locale_code, tr_args};
use crate::telemetry;

/// Authenticated HTTP client for the Kramli API.
#[derive(Clone)]
pub(crate) struct ApiClient {
    client: Client,
    base_url: String,
    api_key: String,
    min_request_interval: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceRequestKind {
    SameOrigin,
    ExternalPublicHttps,
    External,
}

impl ResourceRequestKind {
    fn operation_tag(self) -> &'static str {
        match self {
            Self::SameOrigin => "same_origin",
            Self::ExternalPublicHttps => "external_public_https_resource",
            Self::External => "external_resource",
        }
    }
}

const DEFAULT_RATE_LIMIT_MS: u64 = 120;
const MAX_429_RETRIES: u32 = 3;
const MAX_RESOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_ERROR_MESSAGE_CHARS: usize = 500;
const KRAMLI_RATE_LIMIT_MS_ENV: &str = "KRAMLI_RATE_LIMIT_MS";
const KRAMLI_ALLOW_EXTERNAL_RESOURCES_ENV: &str = "KRAMLI_ALLOW_EXTERNAL_RESOURCES";

fn metric_i64(value: impl TryInto<i64>) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}

static LAST_REQUEST_AT: OnceLock<tokio::sync::Mutex<Option<Instant>>> = OnceLock::new();

impl ApiClient {
    #[cfg(test)]
    pub(crate) fn for_tests(base_url: &str) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.to_string(),
            api_key: "kramli_test".to_string(),
            min_request_interval: Duration::from_millis(0),
        }
    }

    #[cfg(test)]
    pub(crate) fn base_url_for_tests(&self) -> &str {
        &self.base_url
    }

    /// Build an API client from persisted configuration and keychain credentials.
    pub(crate) fn new(config: &Config) -> Result<Self, String> {
        let api_key = config.require_api_key()?;
        let base_url = config.base_url().trim_end_matches('/').to_string();
        Self::ensure_secure_base_url(&base_url)?;
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| tr_args("api-http-client-error", &[("error", e.to_string())]))?;
        Ok(Self {
            client,
            base_url,
            api_key,
            min_request_interval: Self::read_rate_limit_interval(),
        })
    }

    /// Reject plaintext `http://` to non-loopback hosts so the API key
    /// (sent in the `X-API-Key` header) is never transmitted in the clear.
    /// `http://localhost`/`127.0.0.1`/`::1` stay allowed for local development.
    fn ensure_secure_base_url(base_url: &str) -> Result<(), String> {
        let parsed = Url::parse(base_url).map_err(|_| {
            tr_args(
                "api-insecure-http",
                &[("host", base_url.trim().to_string())],
            )
        })?;
        if parsed.scheme() != "http" {
            return Ok(());
        };
        let host = parsed.host_str().unwrap_or_default();
        let ip_host = host.trim_start_matches('[').trim_end_matches(']');
        let is_loopback = host == "localhost"
            || host.ends_with(".localhost")
            || ip_host
                .parse::<IpAddr>()
                .map(|addr| addr.is_loopback())
                .unwrap_or(false);
        if is_loopback {
            return Ok(());
        }
        Err(tr_args("api-insecure-http", &[("host", host.to_string())]))
    }

    fn preferred_language() -> String {
        current_locale_code()
    }

    fn read_rate_limit_interval() -> Duration {
        let millis = std::env::var(KRAMLI_RATE_LIMIT_MS_ENV)
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_RATE_LIMIT_MS);
        Duration::from_millis(millis)
    }

    fn limiter() -> &'static tokio::sync::Mutex<Option<Instant>> {
        LAST_REQUEST_AT.get_or_init(|| tokio::sync::Mutex::new(None))
    }

    async fn wait_for_rate_limit(&self) -> u128 {
        if self.min_request_interval.is_zero() {
            return 0;
        }

        let mut gate = Self::limiter().lock().await;
        let mut waited_ms = 0;
        if let Some(previous) = *gate {
            let elapsed = previous.elapsed();
            if elapsed < self.min_request_interval {
                let wait = self.min_request_interval - elapsed;
                waited_ms = wait.as_millis();
                tokio::time::sleep(wait).await;
            }
        }
        *gate = Some(Instant::now());
        waited_ms
    }

    fn retry_delay(resp: &Response, attempt: u32) -> Duration {
        if let Some(raw) = resp.headers().get(RETRY_AFTER) {
            if let Ok(value) = raw.to_str() {
                if let Ok(seconds) = value.trim().parse::<u64>() {
                    return Duration::from_secs(seconds.clamp(1, 120));
                }
            }
        }

        let multiplier = 1_u64 << attempt.min(5);
        Duration::from_millis(250_u64.saturating_mul(multiplier))
    }

    async fn send_with_retry<F>(
        &self,
        mut build_request: F,
        span: Option<&telemetry::TraceSpan>,
    ) -> Result<Response, String>
    where
        F: FnMut() -> reqwest::RequestBuilder,
    {
        let mut attempt = 0;
        let mut wait_ms = 0_u128;
        loop {
            wait_ms = wait_ms.saturating_add(self.wait_for_rate_limit().await);
            let resp = match build_request().send().await {
                Ok(resp) => resp,
                Err(e) => {
                    if let Some(span) = span {
                        span.set_data_i64("api.retry_count", metric_i64(attempt));
                        span.set_data_i64("api.rate_limit_wait_ms", metric_i64(wait_ms));
                        span.set_status(false);
                    }
                    return Err(tr_args("api-network-error", &[("error", e.to_string())]));
                }
            };

            if resp.status() != StatusCode::TOO_MANY_REQUESTS {
                if let Some(span) = span {
                    span.set_data_i64("api.retry_count", metric_i64(attempt));
                    span.set_data_i64("api.rate_limit_wait_ms", metric_i64(wait_ms));
                }
                return Ok(resp);
            }
            if attempt >= MAX_429_RETRIES {
                if let Some(span) = span {
                    span.set_data_i64("api.retry_count", metric_i64(attempt));
                    span.set_data_i64("api.rate_limit_wait_ms", metric_i64(wait_ms));
                }
                return Ok(resp);
            }

            let delay = Self::retry_delay(&resp, attempt);
            attempt += 1;
            tokio::time::sleep(delay).await;
        }
    }

    fn headers(&self) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&self.api_key) {
            h.insert("X-API-Key", v);
        }
        if let Ok(v) = HeaderValue::from_str(&Self::preferred_language()) {
            h.insert(ACCEPT_LANGUAGE, v);
        }
        h
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api{}", self.base_url, path)
    }

    fn resource_url(&self, path_or_url: &str) -> String {
        let value = path_or_url.trim();
        if value.starts_with("http://") || value.starts_with("https://") {
            return value.to_string();
        }
        if value.starts_with('/') {
            return format!("{}{}", self.base_url, value);
        }
        format!("{}/{}", self.base_url, value)
    }

    fn is_same_origin(&self, url: &str) -> bool {
        let Ok(target) = Url::parse(url) else {
            return false;
        };
        let Ok(base) = Url::parse(&self.base_url) else {
            return false;
        };
        target.scheme() == base.scheme()
            && target.host_str() == base.host_str()
            && target.port_or_known_default() == base.port_or_known_default()
    }

    fn language_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&Self::preferred_language()) {
            h.insert(ACCEPT_LANGUAGE, v);
        }
        h
    }

    fn external_resources_enabled() -> bool {
        Self::external_resources_enabled_from(
            std::env::var(KRAMLI_ALLOW_EXTERNAL_RESOURCES_ENV)
                .ok()
                .as_deref(),
        )
    }

    fn external_resources_enabled_from(raw: Option<&str>) -> bool {
        raw.and_then(|raw| match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" => Some(true),
            "0" | "false" | "off" | "no" => Some(false),
            _ => None,
        })
        .unwrap_or(false)
    }

    fn public_https_resource_allowed(url: &str) -> bool {
        let Ok(parsed) = Url::parse(url) else {
            return false;
        };
        if parsed.scheme() != "https" {
            return false;
        }
        let Some(host) = parsed.host_str() else {
            return false;
        };
        Self::public_resource_host_allowed(host)
    }

    fn public_resource_host_allowed(host: &str) -> bool {
        let host = host
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase();
        if host.is_empty() || host == "localhost" || host.ends_with(".localhost") {
            return false;
        }
        match host.parse::<IpAddr>() {
            Ok(ip) => Self::public_resource_ip_allowed(ip),
            Err(_) => true,
        }
    }

    fn public_resource_ip_allowed(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(addr) => {
                let [first, second, third, _] = addr.octets();
                !(addr.is_private()
                    || addr.is_loopback()
                    || addr.is_link_local()
                    || addr.is_broadcast()
                    || addr.is_documentation()
                    || addr.is_unspecified()
                    || addr.is_multicast()
                    || first == 0
                    || first >= 240
                    || (first == 100 && (64..=127).contains(&second))
                    || (first == 198 && (18..=19).contains(&second))
                    || (first == 192 && second == 0 && third == 0))
            }
            IpAddr::V6(addr) => {
                let segments = addr.segments();
                !(addr.is_loopback()
                    || addr.is_unspecified()
                    || addr.is_multicast()
                    || (segments[0] & 0xfe00) == 0xfc00
                    || (segments[0] & 0xffc0) == 0xfe80
                    || (segments[0] == 0x2001 && segments[1] == 0x0db8))
            }
        }
    }

    fn resource_request_kind(
        same_origin: bool,
        public_https_resource: bool,
        explicit_external_resources: bool,
    ) -> ResourceRequestKind {
        if same_origin {
            ResourceRequestKind::SameOrigin
        } else if public_https_resource && !explicit_external_resources {
            ResourceRequestKind::ExternalPublicHttps
        } else {
            ResourceRequestKind::External
        }
    }

    fn append_limited_bytes(out: &mut Vec<u8>, chunk: &[u8], limit: usize) -> Result<(), String> {
        if out.len().saturating_add(chunk.len()) > limit {
            return Err(format!("resource exceeds {limit} bytes"));
        }
        out.extend_from_slice(chunk);
        Ok(())
    }

    fn resource_headers(&self, kind: ResourceRequestKind) -> HeaderMap {
        match kind {
            ResourceRequestKind::SameOrigin => self.headers(),
            ResourceRequestKind::ExternalPublicHttps | ResourceRequestKind::External => {
                Self::language_headers()
            }
        }
    }

    async fn read_limited_bytes(mut resp: Response, limit: usize) -> Result<Vec<u8>, String> {
        if resp
            .content_length()
            .is_some_and(|length| length > limit as u64)
        {
            return Err(format!("resource exceeds {limit} bytes"));
        }

        let mut out = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| tr_args("api-network-error", &[("error", e.to_string())]))?
        {
            Self::append_limited_bytes(&mut out, &chunk, limit)?;
        }
        Ok(out)
    }

    fn request_span(&self, method: &'static str, path: &str) -> telemetry::TraceSpan {
        let span = telemetry::TraceSpan::child("http.client", "api.request");
        span.set_tag("api.method", method);
        span.set_tag("api.route", telemetry::route_template(path));
        span
    }

    fn finish_response_span(&self, span: &telemetry::TraceSpan, status: u16) {
        span.set_tag("api.status_class", telemetry::status_class(status));
    }

    fn format_api_error_message(body: &[u8]) -> String {
        let Ok(text) = std::str::from_utf8(body) else {
            return format!("[{} bytes]", body.len());
        };

        let trimmed = text.trim();
        if trimmed.is_empty() {
            return "(empty response)".to_string();
        }

        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            if let Some(message) = Self::extract_json_error_message(&value) {
                return Self::truncate_error_message(&telemetry::scrub_message(&message));
            }
        }

        let collapsed = trimmed
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if !collapsed.is_empty() {
            return Self::truncate_error_message(&telemetry::scrub_message(&collapsed));
        }

        format!("[{} bytes]", body.len())
    }

    fn extract_json_error_message(value: &Value) -> Option<String> {
        for key in ["error", "message", "detail", "title", "description"] {
            if let Some(message) = value
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
            {
                return Some(message.to_string());
            }
        }

        if let Some(errors) = value.get("errors").and_then(Value::as_array) {
            let parts = errors
                .iter()
                .filter_map(|entry| match entry {
                    Value::String(text) => Some(text.trim().to_string()),
                    Value::Object(obj) => obj
                        .get("message")
                        .or_else(|| obj.get("error"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .map(str::to_string),
                    _ => None,
                })
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>();
            if !parts.is_empty() {
                return Some(parts.join("; "));
            }
        }

        None
    }

    fn truncate_error_message(message: &str) -> String {
        if message.chars().count() <= MAX_ERROR_MESSAGE_CHARS {
            message.to_string()
        } else {
            let truncated: String = message.chars().take(MAX_ERROR_MESSAGE_CHARS).collect();
            format!("{truncated}…")
        }
    }

    async fn handle<T: DeserializeOwned>(
        resp: Response,
        span: Option<&telemetry::TraceSpan>,
    ) -> Result<T, String> {
        let status = resp.status();
        if status.is_success() {
            let text = resp.text().await.map_err(|e| e.to_string())?;
            if let Some(span) = span {
                span.set_data_i64("api.response_bytes", metric_i64(text.len()));
            }
            serde_json::from_str::<T>(&text).map_err(|e| {
                tr_args(
                    "api-parse-response-error",
                    &[
                        ("error", e.to_string()),
                        ("body", format!("[{} bytes]", text.len())),
                    ],
                )
            })
        } else {
            let text = resp.text().await.unwrap_or_default();
            if let Some(span) = span {
                span.set_data_i64("api.response_bytes", metric_i64(text.len()));
            }
            Err(tr_args(
                "api-error",
                &[
                    ("status", status.as_u16().to_string()),
                    ("message", Self::format_api_error_message(text.as_bytes())),
                ],
            ))
        }
    }

    /// Send an authenticated GET request and deserialize the JSON response.
    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let span = self.request_span("GET", path);
        let url = self.url(path);
        let headers = self.headers();
        let resp = match self
            .send_with_retry(
                || self.client.get(&url).headers(headers.clone()),
                Some(&span),
            )
            .await
        {
            Ok(resp) => resp,
            Err(error) => {
                span.set_status(false);
                span.finish();
                return Err(error);
            }
        };
        self.finish_response_span(&span, resp.status().as_u16());
        let result = Self::handle(resp, Some(&span)).await;
        span.set_status(result.is_ok());
        span.finish();
        result
    }

    /// Send an authenticated GET request with query parameters.
    pub(crate) async fn get_query<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, String> {
        let span = self.request_span("GET", path);
        span.set_data_i64("api.query_count", metric_i64(query.len()));
        let url = self.url(path);
        let headers = self.headers();
        let params: Vec<(String, String)> = query
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let resp = self
            .send_with_retry(
                || {
                    self.client
                        .get(&url)
                        .headers(headers.clone())
                        .query(&params)
                },
                Some(&span),
            )
            .await;
        let resp = match resp {
            Ok(resp) => resp,
            Err(error) => {
                span.set_status(false);
                span.finish();
                return Err(error);
            }
        };
        self.finish_response_span(&span, resp.status().as_u16());
        let result = Self::handle(resp, Some(&span)).await;
        span.set_status(result.is_ok());
        span.finish();
        result
    }

    /// Send an authenticated POST request with a JSON body.
    pub(crate) async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let span = self.request_span("POST", path);
        let url = self.url(path);
        let headers = self.headers();
        let payload: Value = serde_json::to_value(body).map_err(|e| e.to_string())?;
        span.set_data_i64("api.request_bytes", metric_i64(payload.to_string().len()));
        let resp = self
            .send_with_retry(
                || {
                    self.client
                        .post(&url)
                        .headers(headers.clone())
                        .timeout(Duration::from_secs(300))
                        .json(&payload)
                },
                Some(&span),
            )
            .await;
        let resp = match resp {
            Ok(resp) => resp,
            Err(error) => {
                span.set_status(false);
                span.finish();
                return Err(error);
            }
        };
        self.finish_response_span(&span, resp.status().as_u16());
        let result = Self::handle(resp, Some(&span)).await;
        span.set_status(result.is_ok());
        span.finish();
        result
    }

    /// Send an authenticated multipart upload and deserialize its JSON response.
    pub(crate) async fn post_multipart<T: DeserializeOwned>(
        &self,
        path: &str,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
        fields: &[(String, String)],
    ) -> Result<T, String> {
        let span = self.request_span("POST", path);
        span.set_tag("api.upload", "image");
        span.set_data_i64("api.request_bytes", metric_i64(bytes.len()));
        let url = self.url(path);
        let headers = self.headers();
        let file_name = file_name.to_string();
        let mime_type = mime_type.to_string();
        let resp = self
            .send_with_retry(
                || {
                    let mut form = reqwest::multipart::Form::new().part(
                        "file",
                        reqwest::multipart::Part::bytes(bytes.clone())
                            .file_name(file_name.clone())
                            .mime_str(&mime_type)
                            .expect("validated image MIME type"),
                    );
                    for (name, value) in fields {
                        form = form.text(name.clone(), value.clone());
                    }
                    self.client
                        .post(&url)
                        .headers(headers.clone())
                        .timeout(Duration::from_secs(300))
                        .multipart(form)
                },
                Some(&span),
            )
            .await;
        let resp = match resp {
            Ok(resp) => resp,
            Err(error) => {
                span.set_status(false);
                span.finish();
                return Err(error);
            }
        };
        self.finish_response_span(&span, resp.status().as_u16());
        let result = Self::handle(resp, Some(&span)).await;
        span.set_status(result.is_ok());
        span.finish();
        result
    }

    /// Send an authenticated PUT request with a JSON body.
    pub(crate) async fn put<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let span = self.request_span("PUT", path);
        let url = self.url(path);
        let headers = self.headers();
        let payload: Value = serde_json::to_value(body).map_err(|e| e.to_string())?;
        span.set_data_i64("api.request_bytes", metric_i64(payload.to_string().len()));
        let resp = self
            .send_with_retry(
                || {
                    self.client
                        .put(&url)
                        .headers(headers.clone())
                        .json(&payload)
                },
                Some(&span),
            )
            .await;
        let resp = match resp {
            Ok(resp) => resp,
            Err(error) => {
                span.set_status(false);
                span.finish();
                return Err(error);
            }
        };
        self.finish_response_span(&span, resp.status().as_u16());
        let result = Self::handle(resp, Some(&span)).await;
        span.set_status(result.is_ok());
        span.finish();
        result
    }

    /// Send an authenticated PATCH request with a JSON body.
    pub(crate) async fn patch_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let span = self.request_span("PATCH", path);
        let url = self.url(path);
        let headers = self.headers();
        let payload: Value = serde_json::to_value(body).map_err(|e| e.to_string())?;
        span.set_data_i64("api.request_bytes", metric_i64(payload.to_string().len()));
        let resp = self
            .send_with_retry(
                || {
                    self.client
                        .patch(&url)
                        .headers(headers.clone())
                        .json(&payload)
                },
                Some(&span),
            )
            .await;
        let resp = match resp {
            Ok(resp) => resp,
            Err(error) => {
                span.set_status(false);
                span.finish();
                return Err(error);
            }
        };
        self.finish_response_span(&span, resp.status().as_u16());
        let result = Self::handle(resp, Some(&span)).await;
        span.set_status(result.is_ok());
        span.finish();
        result
    }

    /// Send an authenticated DELETE request and deserialize the JSON response.
    pub(crate) async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let span = self.request_span("DELETE", path);
        let url = self.url(path);
        let headers = self.headers();
        let resp = match self
            .send_with_retry(
                || self.client.delete(&url).headers(headers.clone()),
                Some(&span),
            )
            .await
        {
            Ok(resp) => resp,
            Err(error) => {
                span.set_status(false);
                span.finish();
                return Err(error);
            }
        };
        self.finish_response_span(&span, resp.status().as_u16());
        let result = Self::handle(resp, Some(&span)).await;
        span.set_status(result.is_ok());
        span.finish();
        result
    }

    /// Send an authenticated DELETE request that may return an empty success body.
    pub(crate) async fn delete_ok(&self, path: &str) -> Result<bool, String> {
        let span = self.request_span("DELETE", path);
        let url = self.url(path);
        let headers = self.headers();
        let resp = match self
            .send_with_retry(
                || self.client.delete(&url).headers(headers.clone()),
                Some(&span),
            )
            .await
        {
            Ok(resp) => resp,
            Err(error) => {
                span.set_status(false);
                span.finish();
                return Err(error);
            }
        };
        let status = resp.status();
        self.finish_response_span(&span, status.as_u16());
        if status == StatusCode::NO_CONTENT || status.is_success() {
            span.set_status(true);
            span.finish();
            Ok(true)
        } else {
            let text = resp.text().await.unwrap_or_default();
            span.set_data_i64("api.response_bytes", metric_i64(text.len()));
            span.set_status(false);
            span.finish();
            Err(tr_args(
                "api-error",
                &[
                    ("status", status.as_u16().to_string()),
                    ("message", Self::format_api_error_message(text.as_bytes())),
                ],
            ))
        }
    }

    /// Fetch a same-origin or explicitly allowed public resource as bytes.
    pub(crate) async fn get_bytes(&self, path_or_url: &str) -> Result<Vec<u8>, String> {
        let url = self.resource_url(path_or_url);
        let span = telemetry::TraceSpan::child("http.client", "api.resource");
        span.set_tag("api.method", "GET");
        span.set_tag("api.route", "resource");
        let same_origin = self.is_same_origin(&url);
        let explicit_external_resources = Self::external_resources_enabled();
        let public_https_resource = Self::public_https_resource_allowed(&url);
        if !same_origin && !explicit_external_resources && !public_https_resource {
            span.set_tag("operation", "external_resource_blocked");
            span.set_status(false);
            span.finish();
            return Err(
                "external resource is not a public HTTPS URL; set KRAMLI_ALLOW_EXTERNAL_RESOURCES=1 to allow it"
                    .to_string(),
            );
        }
        let request_kind = Self::resource_request_kind(
            same_origin,
            public_https_resource,
            explicit_external_resources,
        );
        span.set_tag("operation", request_kind.operation_tag());
        let headers = self.resource_headers(request_kind);

        let resp = match self
            .send_with_retry(
                || self.client.get(&url).headers(headers.clone()),
                Some(&span),
            )
            .await
        {
            Ok(resp) => resp,
            Err(error) => {
                span.set_status(false);
                span.finish();
                return Err(error);
            }
        };

        let status = resp.status();
        self.finish_response_span(&span, status.as_u16());
        if status.is_success() {
            let result = Self::read_limited_bytes(resp, MAX_RESOURCE_BYTES).await;
            if let Ok(bytes) = &result {
                span.set_data_i64("api.response_bytes", metric_i64(bytes.len()));
            }
            span.set_status(result.is_ok());
            span.finish();
            return result;
        }

        let bytes = Self::read_limited_bytes(resp, MAX_RESOURCE_BYTES)
            .await
            .unwrap_or_default();
        span.set_data_i64("api.response_bytes", metric_i64(bytes.len()));
        span.set_status(false);
        span.finish();
        Err(tr_args(
            "api-error",
            &[
                ("status", status.as_u16().to_string()),
                ("message", Self::format_api_error_message(&bytes)),
            ],
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use reqwest::Client;
    use serde_json::{json, Value};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::{ApiClient, ResourceRequestKind, MAX_RESOURCE_BYTES};

    struct TestResponse {
        status: u16,
        headers: Vec<(&'static str, &'static str)>,
        body: Vec<u8>,
    }

    impl TestResponse {
        fn json(value: Value) -> Self {
            Self {
                status: 200,
                headers: vec![("Content-Type", "application/json")],
                body: value.to_string().into_bytes(),
            }
        }

        fn status(status: u16, body: impl Into<Vec<u8>>) -> Self {
            Self {
                status,
                headers: Vec::new(),
                body: body.into(),
            }
        }

        fn bytes(body: impl Into<Vec<u8>>) -> Self {
            Self {
                status: 200,
                headers: Vec::new(),
                body: body.into(),
            }
        }
    }

    async fn api_with_responses(
        responses: Vec<TestResponse>,
    ) -> (ApiClient, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server address");
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept())
                        .await
                        .expect("test server accept timed out")
                        .expect("accept request");
                let mut buf = vec![0_u8; 8192];
                let n = stream.read(&mut buf).await.expect("read request");
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                requests.push(request.lines().next().unwrap_or_default().to_string());

                let reason = "OK";
                let mut header = format!(
                    "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response.status,
                    reason,
                    response.body.len()
                );
                for (name, value) in response.headers {
                    header.push_str(name);
                    header.push_str(": ");
                    header.push_str(value);
                    header.push_str("\r\n");
                }
                header.push_str("\r\n");
                let _ = stream.write_all(header.as_bytes()).await;
                let _ = stream.write_all(&response.body).await;
            }
            requests
        });
        (test_client(&format!("http://{addr}")), handle)
    }

    #[test]
    fn allows_https() {
        assert!(ApiClient::ensure_secure_base_url("https://kramli.de").is_ok());
        assert!(ApiClient::ensure_secure_base_url("https://self-hosted.example.com").is_ok());
    }

    #[test]
    fn allows_http_loopback() {
        assert!(ApiClient::ensure_secure_base_url("http://localhost:8000").is_ok());
        assert!(ApiClient::ensure_secure_base_url("http://127.0.0.1:5000").is_ok());
        assert!(ApiClient::ensure_secure_base_url("http://[::1]:8000").is_ok());
        assert!(ApiClient::ensure_secure_base_url("http://api.localhost").is_ok());
    }

    #[test]
    fn rejects_http_remote() {
        assert!(ApiClient::ensure_secure_base_url("http://kramli.de").is_err());
        assert!(ApiClient::ensure_secure_base_url("http://192.0.2.10:8080").is_err());
        // userinfo must not be mistaken for a loopback host
        assert!(ApiClient::ensure_secure_base_url("http://localhost@evil.example.com").is_err());
        assert!(ApiClient::ensure_secure_base_url("http://127.evil.com").is_err());
        assert!(ApiClient::ensure_secure_base_url("http://127.0.0.1.evil.example").is_err());
    }

    fn test_client(base_url: &str) -> ApiClient {
        ApiClient {
            client: Client::new(),
            base_url: base_url.to_string(),
            api_key: "kramli_test".to_string(),
            min_request_interval: Duration::from_millis(0),
        }
    }

    #[test]
    fn resource_url_uses_base_for_relative_paths() {
        let client = test_client("https://kramli.de");
        assert_eq!(
            client.resource_url("/uploads/file.jpg"),
            "https://kramli.de/uploads/file.jpg"
        );
        assert_eq!(
            client.resource_url("uploads/file.jpg"),
            "https://kramli.de/uploads/file.jpg"
        );
    }

    #[test]
    fn same_origin_detection_rejects_external_hosts() {
        let client = test_client("https://kramli.de");
        assert!(client.is_same_origin("https://kramli.de/uploads/file.jpg"));
        assert!(!client.is_same_origin("https://example.com/file.jpg"));
    }

    #[test]
    fn external_resources_are_opt_in() {
        assert!(!ApiClient::external_resources_enabled_from(None));
        assert!(!ApiClient::external_resources_enabled_from(Some("invalid")));
        assert!(!ApiClient::external_resources_enabled_from(Some("0")));
        assert!(ApiClient::external_resources_enabled_from(Some("true")));
    }

    #[test]
    fn public_https_resources_are_allowed_by_default() {
        assert!(ApiClient::public_https_resource_allowed(
            "https://cdn.example.com/avatar.png"
        ));
        assert!(ApiClient::public_https_resource_allowed(
            "https://93.184.216.34/avatar.png"
        ));
    }

    #[test]
    fn unsafe_external_resources_are_not_allowed_by_default() {
        assert!(!ApiClient::public_https_resource_allowed(
            "http://cdn.example.com/avatar.png"
        ));
        assert!(!ApiClient::public_https_resource_allowed(
            "https://localhost/avatar.png"
        ));
        assert!(!ApiClient::public_https_resource_allowed(
            "https://api.localhost/avatar.png"
        ));
        assert!(!ApiClient::public_https_resource_allowed(
            "https://127.0.0.1/avatar.png"
        ));
        assert!(!ApiClient::public_https_resource_allowed(
            "https://10.0.0.5/avatar.png"
        ));
        assert!(!ApiClient::public_https_resource_allowed(
            "https://[::1]/avatar.png"
        ));
        assert!(!ApiClient::public_https_resource_allowed(
            "https://[fd00::1]/avatar.png"
        ));
    }

    #[test]
    fn api_error_message_extracts_common_json_fields() {
        assert_eq!(
            ApiClient::format_api_error_message(br#"{"error":"List not found"}"#),
            "List not found"
        );
        assert_eq!(
            ApiClient::format_api_error_message(br#"{"message":"Forbidden"}"#),
            "Forbidden"
        );
        assert_eq!(
            ApiClient::format_api_error_message(br#"{"detail":"Invalid token"}"#),
            "Invalid token"
        );
    }

    #[test]
    fn api_error_message_extracts_validation_errors() {
        assert_eq!(
            ApiClient::format_api_error_message(
                br#"{"errors":[{"message":"name is required"},{"message":"icon is invalid"}]}"#
            ),
            "name is required; icon is invalid"
        );
    }

    #[test]
    fn api_error_message_uses_plain_text_fallback() {
        assert_eq!(
            ApiClient::format_api_error_message(b"Service unavailable"),
            "Service unavailable"
        );
    }

    #[test]
    fn api_error_message_scrubs_sensitive_values() {
        let scrubbed =
            ApiClient::format_api_error_message(b"Invalid API key: kramli_secretvalue rejected");
        assert!(scrubbed.contains("kramli_[REDACTED]"));
        assert!(!scrubbed.contains("kramli_secretvalue"));
    }

    #[test]
    fn api_error_message_reports_empty_response() {
        assert_eq!(
            ApiClient::format_api_error_message(b"  \n  "),
            "(empty response)"
        );
    }

    #[test]
    fn api_error_message_reports_non_utf8_as_byte_count() {
        assert_eq!(
            ApiClient::format_api_error_message(&[0xff, 0xfe]),
            "[2 bytes]"
        );
    }

    #[test]
    fn private_resource_and_header_helpers_cover_edge_branches() {
        let client = test_client("https://kramli.de:8443");

        assert_eq!(
            client.resource_url("https://cdn.example.com/a.png"),
            "https://cdn.example.com/a.png"
        );
        assert!(!client.is_same_origin("not a url"));
        assert!(!test_client("not a base").is_same_origin("https://kramli.de/a.png"));
        assert!(client.headers().contains_key("X-API-Key"));
        assert!(ApiClient::language_headers().contains_key(reqwest::header::ACCEPT_LANGUAGE));
    }

    #[test]
    fn public_resource_host_filter_covers_private_ranges() {
        assert!(!ApiClient::public_resource_host_allowed(""));
        assert!(!ApiClient::public_resource_host_allowed("0.1.2.3"));
        assert!(!ApiClient::public_resource_host_allowed("100.64.0.1"));
        assert!(!ApiClient::public_resource_host_allowed("198.18.0.1"));
        assert!(!ApiClient::public_resource_host_allowed("192.0.0.1"));
        assert!(!ApiClient::public_resource_host_allowed("240.0.0.1"));
        assert!(!ApiClient::public_resource_host_allowed("ff02::1"));
        assert!(!ApiClient::public_resource_host_allowed("fe80::1"));
        assert!(!ApiClient::public_resource_host_allowed("2001:db8::1"));
        assert!(ApiClient::public_resource_host_allowed("example.com"));
        assert!(!ApiClient::public_https_resource_allowed("not a url"));
    }

    #[test]
    fn resource_request_kind_helpers_cover_headers_and_tags() {
        let client = test_client("https://kramli.de:8443");

        assert_eq!(
            ApiClient::resource_request_kind(true, false, false),
            ResourceRequestKind::SameOrigin
        );
        assert_eq!(
            ResourceRequestKind::SameOrigin.operation_tag(),
            "same_origin"
        );
        assert!(client
            .resource_headers(ResourceRequestKind::SameOrigin)
            .contains_key("X-API-Key"));
        assert_eq!(
            ApiClient::resource_request_kind(false, true, false),
            ResourceRequestKind::ExternalPublicHttps
        );
        assert_eq!(
            ResourceRequestKind::ExternalPublicHttps.operation_tag(),
            "external_public_https_resource"
        );
        assert!(client
            .resource_headers(ResourceRequestKind::ExternalPublicHttps)
            .contains_key(reqwest::header::ACCEPT_LANGUAGE));
        assert_eq!(
            ApiClient::resource_request_kind(false, true, true),
            ResourceRequestKind::External
        );
        assert_eq!(
            ResourceRequestKind::External.operation_tag(),
            "external_resource"
        );
        assert!(client
            .resource_headers(ResourceRequestKind::External)
            .contains_key(reqwest::header::ACCEPT_LANGUAGE));
    }

    #[test]
    fn append_limited_bytes_covers_success_and_overflow() {
        let mut out = vec![1, 2];
        ApiClient::append_limited_bytes(&mut out, &[3, 4], 4).unwrap();
        assert_eq!(out, vec![1, 2, 3, 4]);
        assert!(ApiClient::append_limited_bytes(&mut out, &[5], 4).is_err());
    }

    #[test]
    fn error_extraction_and_truncation_cover_fallbacks() {
        assert_eq!(
            ApiClient::format_api_error_message(br#"{"title":"Nope"}"#),
            "Nope"
        );
        assert_eq!(
            ApiClient::format_api_error_message(br#"{"description":"Nope again"}"#),
            "Nope again"
        );
        assert_eq!(
            ApiClient::format_api_error_message(br#"{"errors":["first",{"error":"second"},3]}"#),
            "first; second"
        );
        assert_eq!(
            ApiClient::format_api_error_message(br#"{"unknown":true}"#),
            "{\"unknown\":true}"
        );
        assert_eq!(
            ApiClient::extract_json_error_message(&json!({"errors": [3]})),
            None
        );
        assert!(ApiClient::truncate_error_message(&"x".repeat(600)).ends_with('…'));
    }

    #[tokio::test]
    async fn api_request_helpers_cover_success_and_error_paths() {
        let (api, server) = api_with_responses(vec![
            TestResponse::json(json!({"ok": true})),
            TestResponse::json(json!({"query": true})),
            TestResponse::json(json!({"posted": true})),
            TestResponse::json(json!({"put": true})),
            TestResponse::json(json!({"patched": true})),
            TestResponse::json(json!({"deleted": true})),
            TestResponse::status(204, Vec::new()),
            TestResponse::status(400, br#"{"error":"bad delete"}"#.to_vec()),
            TestResponse::status(500, b"server down".to_vec()),
        ])
        .await;

        let got: Value = api.get("/ok").await.unwrap();
        assert!(got["ok"].as_bool().unwrap_or(false));
        let queried: Value = api.get_query("/search", &[("q", "milk")]).await.unwrap();
        assert!(queried["query"].as_bool().unwrap_or(false));
        let posted: Value = api.post("/items", &json!({"text": "Milk"})).await.unwrap();
        assert!(posted["posted"].as_bool().unwrap_or(false));
        let put: Value = api.put("/items/1", &json!({"text": "Eggs"})).await.unwrap();
        assert!(put["put"].as_bool().unwrap_or(false));
        let patched: Value = api.patch_json("/items/1/done", &json!({})).await.unwrap();
        assert!(patched["patched"].as_bool().unwrap_or(false));
        let deleted: Value = api.delete("/items/1").await.unwrap();
        assert!(deleted["deleted"].as_bool().unwrap_or(false));
        assert!(api.delete_ok("/items/1").await.unwrap());
        assert!(api.delete_ok("/items/2").await.is_err());
        assert!(api.get::<Value>("/fail").await.is_err());

        let requests = server.await.expect("server finished");
        assert_eq!(requests[0], "GET /api/ok HTTP/1.1");
        assert!(requests[1].starts_with("GET /api/search?"));
        assert_eq!(requests[2], "POST /api/items HTTP/1.1");
        assert_eq!(requests[3], "PUT /api/items/1 HTTP/1.1");
        assert_eq!(requests[4], "PATCH /api/items/1/done HTTP/1.1");
        assert_eq!(requests[5], "DELETE /api/items/1 HTTP/1.1");
        assert_eq!(requests[6], "DELETE /api/items/1 HTTP/1.1");
        assert_eq!(requests[7], "DELETE /api/items/2 HTTP/1.1");
        assert_eq!(requests[8], "GET /api/fail HTTP/1.1");
    }

    #[tokio::test]
    async fn resource_fetching_covers_same_public_external_and_error_paths() {
        let (api, server) = api_with_responses(vec![
            TestResponse::bytes(b"same".to_vec()),
            TestResponse::bytes(b"public".to_vec()),
            TestResponse::status(200, vec![b'x'; MAX_RESOURCE_BYTES + 1]),
            TestResponse::status(404, b"missing".to_vec()),
        ])
        .await;
        let base_url = api.base_url_for_tests().to_string();

        assert_eq!(api.get_bytes("/asset.png").await.unwrap(), b"same".to_vec());
        assert_eq!(
            api.get_bytes(&format!("{base_url}/external.png"))
                .await
                .unwrap(),
            b"public".to_vec()
        );
        assert!(api.get_bytes("/too-large.png").await.is_err());
        assert!(api
            .get_bytes("http://example.com/insecure.png")
            .await
            .is_err());
        assert!(api.get_bytes("/missing.png").await.is_err());

        let requests = server.await.expect("server finished");
        assert_eq!(requests[0], "GET /asset.png HTTP/1.1");
        assert_eq!(requests[1], "GET /external.png HTTP/1.1");
        assert_eq!(requests[2], "GET /too-large.png HTTP/1.1");
        assert_eq!(requests[3], "GET /missing.png HTTP/1.1");
    }
}
