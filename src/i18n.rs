use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use fluent_templates::{fluent_bundle::FluentValue, static_loader, Loader};
use unic_langid::{langid, LanguageIdentifier};

static_loader! {
    pub(crate) static LOCALES = {
        locales: "./locales",
        fallback_language: "en",
    };
}

static ACTIVE_LOCALE: OnceLock<RwLock<LanguageIdentifier>> = OnceLock::new();

const SUPPORTED_LANGS: &[&str] = &[
    "en", "de", "fr", "es", "it", "nl", "pl", "pt", "ru", "tr", "uk", "ar", "ja", "ko", "zh",
];
const KRAMLI_LANG_ENV: &str = "KRAMLI_LANG";
const LOCALE_ENV_VARS: &[&str] = &[KRAMLI_LANG_ENV, "LC_ALL", "LC_MESSAGES", "LANG"];

fn normalize_candidate(raw: &str) -> Option<String> {
    let first = raw.trim().split(',').next()?.trim();
    if first.is_empty() {
        return None;
    }

    let without_charset = first.split('.').next().unwrap_or(first);
    let without_modifier = without_charset.split('@').next().unwrap_or(without_charset);
    let normalized = without_modifier.replace('_', "-").trim().to_string();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn parse_supported_locale(raw: &str) -> LanguageIdentifier {
    if let Some(locale) = try_parse_supported_locale(raw) {
        return locale;
    }

    langid!("en")
}

fn try_parse_supported_locale(raw: &str) -> Option<LanguageIdentifier> {
    if let Ok(lang) = raw.parse::<LanguageIdentifier>() {
        if SUPPORTED_LANGS.contains(&lang.language.as_str()) {
            return Some(lang);
        }
    }

    let primary = raw
        .split('-')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if SUPPORTED_LANGS.contains(&primary.as_str()) {
        return primary.parse::<LanguageIdentifier>().ok();
    }

    None
}

fn locale_lock() -> &'static RwLock<LanguageIdentifier> {
    ACTIVE_LOCALE.get_or_init(|| RwLock::new(detect_locale()))
}

fn detect_locale() -> LanguageIdentifier {
    for var in LOCALE_ENV_VARS {
        if let Ok(raw) = std::env::var(var) {
            if let Some(candidate) = normalize_candidate(&raw) {
                return parse_supported_locale(&candidate);
            }
        }
    }

    langid!("en")
}

/// Return the active locale used for translations.
pub(crate) fn current_locale() -> LanguageIdentifier {
    locale_lock()
        .read()
        .expect("locale read lock poisoned")
        .clone()
}

/// Return the active locale as a BCP-47 language tag.
pub(crate) fn current_locale_code() -> String {
    current_locale().to_string()
}

/// Return whether the locale was explicitly selected with `KRAMLI_LANG`.
pub(crate) fn is_explicit_lang_set() -> bool {
    std::env::var(KRAMLI_LANG_ENV)
        .ok()
        .and_then(|raw| normalize_candidate(&raw))
        .is_some()
}

/// Set the active locale when the input is supported.
pub(crate) fn set_locale(raw: &str) -> bool {
    let Some(candidate) = normalize_candidate(raw) else {
        return false;
    };
    let Some(locale) = try_parse_supported_locale(&candidate) else {
        return false;
    };
    *locale_lock().write().expect("locale write lock poisoned") = locale;
    true
}

/// Apply a profile locale unless the user set an explicit environment locale.
pub(crate) fn apply_profile_locale(raw: Option<&str>) -> bool {
    if is_explicit_lang_set() {
        return false;
    }
    let Some(value) = raw else {
        return false;
    };
    set_locale(value)
}

/// Translate a message key for the active locale.
pub(crate) fn tr(key: &str) -> String {
    let locale = current_locale();
    LOCALES.lookup(&locale, key).to_string()
}

/// Translate a message key with named Fluent arguments.
pub(crate) fn tr_args(key: &str, args: &[(&str, String)]) -> String {
    let mut fluent_args: HashMap<Cow<'static, str>, FluentValue<'static>> = HashMap::new();
    for (name, value) in args {
        fluent_args.insert(
            Cow::Owned((*name).to_string()),
            FluentValue::from(value.clone()),
        );
    }
    let locale = current_locale();
    LOCALES
        .lookup_with_args(&locale, key, &fluent_args)
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use super::{
        apply_profile_locale, current_locale_code, detect_locale, is_explicit_lang_set,
        normalize_candidate, parse_supported_locale, set_locale, tr, tr_args,
        try_parse_supported_locale, KRAMLI_LANG_ENV, SUPPORTED_LANGS,
    };

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn locale_file(lang: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("locales")
            .join(lang)
            .join("main.ftl")
    }

    fn extract_keys(content: &str) -> Vec<String> {
        content
            .lines()
            .filter_map(|line| {
                if line.trim().is_empty() || line.starts_with('#') {
                    return None;
                }
                if line.starts_with(' ') || line.starts_with('\t') {
                    return None;
                }

                let (key, _) = line.split_once('=')?;
                let key = key.trim();
                if key.is_empty() {
                    None
                } else {
                    Some(key.to_string())
                }
            })
            .collect()
    }

    #[test]
    fn every_locale_has_same_keys_as_english() {
        let en_path = locale_file("en");
        let en_content = fs::read_to_string(&en_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", en_path.display()));
        let en_keys = extract_keys(&en_content);
        assert!(!en_keys.is_empty(), "english locale has no keys");

        for lang in SUPPORTED_LANGS {
            let path = locale_file(lang);
            let content = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            let keys = extract_keys(&content);
            assert_eq!(
                keys, en_keys,
                "locale '{}' key set/order differs from en",
                lang
            );
        }
    }

    fn with_env_var<T>(key: &str, value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().expect("i18n env lock poisoned");
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

    #[test]
    fn locale_candidate_parsing_handles_empty_charset_modifier_and_fallbacks() {
        assert_eq!(normalize_candidate("  "), None);
        assert_eq!(
            normalize_candidate("de_DE.UTF-8@euro,fr"),
            Some("de-DE".to_string())
        );
        assert_eq!(
            try_parse_supported_locale("pt-BR").unwrap().to_string(),
            "pt-BR"
        );
        assert_eq!(
            try_parse_supported_locale("de-DE").unwrap().to_string(),
            "de-DE"
        );
        assert!(try_parse_supported_locale("xx-YY").is_none());
        assert_eq!(parse_supported_locale("xx-YY").to_string(), "en");
    }

    #[test]
    fn locale_env_and_profile_helpers_cover_explicit_and_default_paths() {
        with_env_var(KRAMLI_LANG_ENV, Some("fr_FR.UTF-8"), || {
            assert!(is_explicit_lang_set());
            assert_eq!(detect_locale().to_string(), "fr-FR");
            assert!(!apply_profile_locale(Some("de")));
        });

        with_env_var(KRAMLI_LANG_ENV, Some("  "), || {
            assert!(!is_explicit_lang_set());
        });

        with_env_var(KRAMLI_LANG_ENV, None, || {
            assert!(!apply_profile_locale(None));
            assert!(apply_profile_locale(Some("de-DE")));
            assert_eq!(
                current_locale_code(),
                parse_supported_locale("de-DE").to_string()
            );
            assert!(!set_locale("xx-YY"));
            assert!(!set_locale("  "));
        });
    }

    #[test]
    fn env_helper_restores_existing_lang_value() {
        std::env::set_var(KRAMLI_LANG_ENV, "en");

        with_env_var(KRAMLI_LANG_ENV, Some("fr"), || {
            assert_eq!(std::env::var(KRAMLI_LANG_ENV).as_deref(), Ok("fr"));
        });

        assert_eq!(std::env::var(KRAMLI_LANG_ENV).as_deref(), Ok("en"));
        std::env::remove_var(KRAMLI_LANG_ENV);
    }

    #[test]
    fn translations_with_and_without_arguments_are_returned() {
        assert!(set_locale("en"));
        assert!(!tr("label-items").is_empty());
        let rendered = tr_args("api-network-error", &[("error", "offline".to_string())]);
        assert!(rendered.contains("offline"));
    }
}
