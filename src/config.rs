use std::fs;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

use keyring::Entry;
use serde::{Deserialize, Serialize};

use crate::i18n::{tr, tr_args};

const DEFAULT_BASE_URL: &str = "https://kramli.de";
const KEYRING_SERVICE: &str = "kramli-cli";
const KEYRING_API_KEY: &str = "api-key";

// ── On-disk config: non-sensitive settings only ──

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ConfigFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_check_last: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_check_latest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_check_url: Option<String>,
}

// ── Public Config handle ──

pub struct Config {
    file: ConfigFile,
}

impl Config {
    /// Path to the config file: ~/.config/kramli/config.json
    pub fn path() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("kramli").join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        let file = if path.exists() {
            let data = fs::read_to_string(&path).unwrap_or_default();
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            ConfigFile::default()
        };
        Self { file }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| tr_args("config-create-dir-error", &[("error", e.to_string())]))?;
        }
        let data = serde_json::to_string_pretty(&self.file).map_err(|e| e.to_string())?;
        fs::write(&path, &data)
            .map_err(|e| tr_args("config-save-error", &[("error", e.to_string())]))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            fs::set_permissions(&path, perms)
                .map_err(|e| tr_args("config-permissions-error", &[("error", e.to_string())]))?;
        }
        Ok(())
    }

    // ── Non-sensitive getters / setters ──

    /// Base URL: env `KRAMLI_URL` > config file > default.
    pub fn base_url(&self) -> String {
        if let Ok(url) = std::env::var("KRAMLI_URL") {
            if !url.is_empty() {
                return url;
            }
        }
        self.file
            .base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
    }

    pub fn set_base_url(&mut self, url: Option<String>) {
        self.file.base_url = url;
    }

    pub fn update_check_last(&self) -> Option<i64> {
        self.file.update_check_last
    }

    pub fn update_check_latest(&self) -> Option<String> {
        self.file.update_check_latest.clone()
    }

    pub fn update_check_url(&self) -> Option<String> {
        self.file.update_check_url.clone()
    }

    pub fn set_update_check_state(
        &mut self,
        checked_at: i64,
        latest: Option<String>,
        url: Option<String>,
    ) {
        self.file.update_check_last = Some(checked_at);
        self.file.update_check_latest = latest;
        self.file.update_check_url = url;
    }

    // ── Keychain-backed API key (env override: KRAMLI_API_KEY) ──

    fn keyring_entry(key: &str) -> Result<Entry, String> {
        Entry::new(KEYRING_SERVICE, key)
            .map_err(|e| tr_args("config-keychain-error", &[("error", e.to_string())]))
    }

    fn keychain_api_key() -> Result<Option<String>, String> {
        let entry = Self::keyring_entry(KEYRING_API_KEY)?;
        match entry.get_password() {
            Ok(key) => {
                if key.trim().is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(key))
                }
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => {
                #[cfg(target_os = "macos")]
                {
                    if let Some(key) = Self::api_key_via_security_cli() {
                        return Ok(Some(key));
                    }
                }
                Err(tr_args(
                    "config-read-key-error",
                    &[("error", error.to_string())],
                ))
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn api_key_via_security_cli() -> Option<String> {
        let output = Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                KEYRING_SERVICE,
                "-a",
                KEYRING_API_KEY,
                "-w",
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let key = String::from_utf8(output.stdout).ok()?;
        let trimmed = key.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    /// API key: env `KRAMLI_API_KEY` > OS keychain.
    pub fn api_key(&self) -> Option<String> {
        if let Ok(key) = std::env::var("KRAMLI_API_KEY") {
            if !key.is_empty() {
                return Some(key);
            }
        }
        Self::keychain_api_key().ok().flatten()
    }

    pub fn set_api_key(&self, key: &str) -> Result<(), String> {
        Self::keyring_entry(KEYRING_API_KEY)?
            .set_password(key)
            .map_err(|e| tr_args("config-store-key-error", &[("error", e.to_string())]))
    }

    pub fn delete_api_key(&self) -> Result<(), String> {
        if let Ok(entry) = Self::keyring_entry(KEYRING_API_KEY) {
            let _ = entry.delete_credential();
        }
        Ok(())
    }

    pub fn require_api_key(&self) -> Result<String, String> {
        if let Ok(key) = std::env::var("KRAMLI_API_KEY") {
            if !key.is_empty() {
                return Ok(key);
            }
        }
        match Self::keychain_api_key()? {
            Some(key) => Ok(key),
            None => Err(tr("config-not-logged-in")),
        }
    }

    pub fn has_api_key(&self) -> bool {
        self.api_key().is_some()
    }

    /// True when the API key was provided via env var (not keychain).
    pub fn api_key_from_env(&self) -> bool {
        std::env::var("KRAMLI_API_KEY")
            .map(|k| !k.is_empty())
            .unwrap_or(false)
    }
}
