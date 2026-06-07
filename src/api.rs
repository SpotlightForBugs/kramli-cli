use std::sync::OnceLock;
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_LANGUAGE, RETRY_AFTER};
use reqwest::{Client, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::config::Config;
use crate::i18n::{current_locale_code, tr_args};
use crate::models::ApiError;

pub struct ApiClient {
    client: Client,
    base_url: String,
    api_key: String,
    min_request_interval: Duration,
}

const DEFAULT_RATE_LIMIT_MS: u64 = 120;
const MAX_429_RETRIES: u32 = 3;

static LAST_REQUEST_AT: OnceLock<tokio::sync::Mutex<Option<Instant>>> = OnceLock::new();

impl ApiClient {
    pub fn new(config: &Config) -> Result<Self, String> {
        let api_key = config.require_api_key()?;
        let base_url = config.base_url().trim_end_matches('/').to_string();
        Self::ensure_secure_base_url(&base_url)?;
        let client = Client::builder()
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
        let lower = base_url.trim().to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("http://") else {
            return Ok(());
        };
        let authority = rest.split('/').next().unwrap_or("");
        // Drop any userinfo (user:pass@host) before inspecting the host.
        let host = authority.rsplit('@').next().unwrap_or(authority);
        let host_only = host.split(':').next().unwrap_or(host);
        let is_loopback = host_only == "localhost"
            || host_only.ends_with(".localhost")
            || host_only.starts_with("127.")
            || host_only == "::1"
            || host.contains("[::1]");
        if is_loopback {
            return Ok(());
        }
        Err(tr_args(
            "api-insecure-http",
            &[("host", host_only.to_string())],
        ))
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

    async fn wait_for_rate_limit(&self) {
        if self.min_request_interval.is_zero() {
            return;
        }

        let mut gate = Self::limiter().lock().await;
        if let Some(previous) = *gate {
            let elapsed = previous.elapsed();
            if elapsed < self.min_request_interval {
                tokio::time::sleep(self.min_request_interval - elapsed).await;
            }
        }
        *gate = Some(Instant::now());
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

    async fn send_with_retry<F>(&self, mut build_request: F) -> Result<Response, String>
    where
        F: FnMut() -> reqwest::RequestBuilder,
    {
        let mut attempt = 0;
        loop {
            self.wait_for_rate_limit().await;
            let resp = build_request()
                .send()
                .await
                .map_err(|e| tr_args("api-network-error", &[("error", e.to_string())]))?;

            if resp.status() != StatusCode::TOO_MANY_REQUESTS {
                return Ok(resp);
            }
            if attempt >= MAX_429_RETRIES {
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

    async fn handle<T: DeserializeOwned>(resp: Response) -> Result<T, String> {
        let status = resp.status();
        if status.is_success() {
            let text = resp.text().await.map_err(|e| e.to_string())?;
            serde_json::from_str::<T>(&text).map_err(|e| {
                tr_args(
                    "api-parse-response-error",
                    &[("error", e.to_string()), ("body", text.clone())],
                )
            })
        } else {
            let text = resp.text().await.unwrap_or_default();
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
                    &[("status", status.as_u16().to_string()), ("message", text)],
                ))
            }
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let url = self.url(path);
        let headers = self.headers();
        let resp = self
            .send_with_retry(|| self.client.get(&url).headers(headers.clone()))
            .await?;
        Self::handle(resp).await
    }

    pub async fn get_query<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, String> {
        let url = self.url(path);
        let headers = self.headers();
        let params: Vec<(String, String)> = query
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let resp = self
            .send_with_retry(|| {
                self.client
                    .get(&url)
                    .headers(headers.clone())
                    .query(&params)
            })
            .await?;
        Self::handle(resp).await
    }

    pub async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let url = self.url(path);
        let headers = self.headers();
        let payload: Value = serde_json::to_value(body).map_err(|e| e.to_string())?;
        let resp = self
            .send_with_retry(|| {
                self.client
                    .post(&url)
                    .headers(headers.clone())
                    .json(&payload)
            })
            .await?;
        Self::handle(resp).await
    }

    pub async fn put<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let url = self.url(path);
        let headers = self.headers();
        let payload: Value = serde_json::to_value(body).map_err(|e| e.to_string())?;
        let resp = self
            .send_with_retry(|| {
                self.client
                    .put(&url)
                    .headers(headers.clone())
                    .json(&payload)
            })
            .await?;
        Self::handle(resp).await
    }

    pub async fn patch_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let url = self.url(path);
        let headers = self.headers();
        let payload: Value = serde_json::to_value(body).map_err(|e| e.to_string())?;
        let resp = self
            .send_with_retry(|| {
                self.client
                    .patch(&url)
                    .headers(headers.clone())
                    .json(&payload)
            })
            .await?;
        Self::handle(resp).await
    }

    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let url = self.url(path);
        let headers = self.headers();
        let resp = self
            .send_with_retry(|| self.client.delete(&url).headers(headers.clone()))
            .await?;
        Self::handle(resp).await
    }

    pub async fn delete_ok(&self, path: &str) -> Result<bool, String> {
        let url = self.url(path);
        let headers = self.headers();
        let resp = self
            .send_with_retry(|| self.client.delete(&url).headers(headers.clone()))
            .await?;
        let status = resp.status();
        if status == StatusCode::NO_CONTENT || status.is_success() {
            Ok(true)
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(tr_args(
                "api-error",
                &[("status", status.as_u16().to_string()), ("message", text)],
            ))
        }
    }
}

#[cfg(test)]
mod tests {
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
    }
}
