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
use crate::models::ApiError;
use crate::telemetry;

#[derive(Clone)]
pub struct ApiClient {
    client: Client,
    base_url: String,
    api_key: String,
    min_request_interval: Duration,
}

const DEFAULT_RATE_LIMIT_MS: u64 = 120;
const MAX_429_RETRIES: u32 = 3;

fn metric_i64(value: impl TryInto<i64>) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}

static LAST_REQUEST_AT: OnceLock<tokio::sync::Mutex<Option<Instant>>> = OnceLock::new();

impl ApiClient {
    pub fn new(config: &Config) -> Result<Self, String> {
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
        let millis = std::env::var("KRAMLI_RATE_LIMIT_MS")
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

    fn request_span(&self, method: &'static str, path: &str) -> telemetry::TraceSpan {
        let span = telemetry::TraceSpan::child("http.client", "api.request");
        span.set_tag("api.method", method);
        span.set_tag("api.route", telemetry::route_template(path));
        span
    }

    fn finish_response_span(&self, span: &telemetry::TraceSpan, status: u16) {
        span.set_tag("api.status_class", telemetry::status_class(status));
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
            if let Ok(err) = serde_json::from_str::<ApiError>(&text) {
                Err(tr_args(
                    "api-error",
                    &[
                        ("status", status.as_u16().to_string()),
                        ("message", err.error.unwrap_or(text)),
                    ],
                ))
            } else {
                Err(tr_args(
                    "api-error",
                    &[
                        ("status", status.as_u16().to_string()),
                        ("message", format!("[{} bytes]", text.len())),
                    ],
                ))
            }
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
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

    pub async fn get_query<T: DeserializeOwned>(
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

    pub async fn post<B: Serialize, T: DeserializeOwned>(
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

    pub async fn put<B: Serialize, T: DeserializeOwned>(
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

    pub async fn patch_json<B: Serialize, T: DeserializeOwned>(
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

    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
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

    pub async fn delete_ok(&self, path: &str) -> Result<bool, String> {
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
                    ("message", format!("[{} bytes]", text.len())),
                ],
            ))
        }
    }

    pub async fn get_bytes(&self, path_or_url: &str) -> Result<Vec<u8>, String> {
        let url = self.resource_url(path_or_url);
        let span = telemetry::TraceSpan::child("http.client", "api.resource");
        span.set_tag("api.method", "GET");
        span.set_tag("api.route", "resource");
        let headers = if self.is_same_origin(&url) {
            span.set_tag("operation", "same_origin");
            self.headers()
        } else {
            span.set_tag("operation", "external_resource");
            Self::language_headers()
        };

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
            let result = resp
                .bytes()
                .await
                .map(|bytes| {
                    span.set_data_i64("api.response_bytes", metric_i64(bytes.len()));
                    bytes.to_vec()
                })
                .map_err(|e| tr_args("api-network-error", &[("error", e.to_string())]));
            span.set_status(result.is_ok());
            span.finish();
            return result;
        }

        let text = resp.text().await.unwrap_or_default();
        span.set_data_i64("api.response_bytes", metric_i64(text.len()));
        span.set_status(false);
        span.finish();
        Err(tr_args(
            "api-error",
            &[("status", status.as_u16().to_string()), ("message", text)],
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use reqwest::Client;

    use super::ApiClient;

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
}
