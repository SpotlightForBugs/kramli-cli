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
    pub telemetry_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bootstrap_icons_enabled: Option<bool>,
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

    pub fn telemetry_enabled(&self) -> bool {
        telemetry_env_override()
            .or(self.file.telemetry_enabled)
            .unwrap_or(false)
    }

    pub fn telemetry_preference_set(&self) -> bool {
        telemetry_env_override().is_some() || self.file.telemetry_enabled.is_some()
    }

    pub fn set_telemetry_enabled(&mut self, enabled: bool) {
        self.file.telemetry_enabled = Some(enabled);
    }

    pub fn bootstrap_icons_enabled(&self) -> bool {
        bootstrap_icons_env_override()
            .or(self.file.bootstrap_icons_enabled)
            .unwrap_or(false)
    }

    pub fn bootstrap_icons_preference_set(&self) -> bool {
        bootstrap_icons_env_override().is_some() || self.file.bootstrap_icons_enabled.is_some()
    }

    pub fn set_bootstrap_icons_enabled(&mut self, enabled: bool) {
        self.file.bootstrap_icons_enabled = Some(enabled);
    }

    pub fn reset_privacy_preferences(&mut self) {
        self.file.telemetry_enabled = None;
        self.file.bootstrap_icons_enabled = None;
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
        Self::keychain_api_key().unwrap_or_default()
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

pub fn parse_env_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

pub fn env_is_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(raw) => {
            let v = raw.trim().to_ascii_lowercase();
            !v.is_empty() && v != "0" && v != "false" && v != "off" && v != "no"
        }
        Err(_) => false,
    }
}

fn telemetry_env_override() -> Option<bool> {
    if env_is_truthy("DO_NOT_TRACK") || env_is_truthy("KRAMLI_NO_TELEMETRY") {
        return Some(false);
    }
    std::env::var("KRAMLI_TELEMETRY")
        .ok()
        .as_deref()
        .and_then(parse_env_bool)
}

fn bootstrap_icons_env_override() -> Option<bool> {
    [
        "KRAMLI_BOOTSTRAP_ICONS",
        "KRAMLI_TUI_BOOTSTRAP_ICONS",
        "KRAMLI_LOAD_BOOTSTRAP_ICONS",
    ]
    .into_iter()
    .find_map(|name| {
        std::env::var(name)
            .ok()
            .and_then(|raw| parse_env_bool(&raw))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_file(
        telemetry_enabled: Option<bool>,
        bootstrap_icons_enabled: Option<bool>,
    ) -> Config {
        Config {
            file: ConfigFile {
                telemetry_enabled,
                bootstrap_icons_enabled,
                ..ConfigFile::default()
            },
        }
    }

    #[test]
    fn unset_preferences_are_disabled_until_user_answers() {
        let cfg = config_file(None, None);
        assert!(!cfg.telemetry_enabled());
        assert!(!cfg.bootstrap_icons_enabled());
    }

    #[test]
    fn saved_preferences_control_telemetry_and_bootstrap_icons() {
        let cfg = config_file(Some(true), Some(true));
        assert!(cfg.telemetry_enabled());
        assert!(cfg.bootstrap_icons_enabled());

        let cfg = config_file(Some(false), Some(false));
        assert!(!cfg.telemetry_enabled());
        assert!(!cfg.bootstrap_icons_enabled());
    }

    #[test]
    fn env_bool_parser_accepts_common_forms() {
        assert_eq!(parse_env_bool("1"), Some(true));
        assert_eq!(parse_env_bool(" yes "), Some(true));
        assert_eq!(parse_env_bool("0"), Some(false));
        assert_eq!(parse_env_bool("off"), Some(false));
        assert_eq!(parse_env_bool("later"), None);
    }
}
