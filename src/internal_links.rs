use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use reqwest::Url;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::api::ApiClient;

const CANONICAL_ORIGIN: &str = "https://kramli.de";
const SHORT_ORIGIN: &str = "https://kram.li";
const INVITE_TOKEN_MIN_LEN: usize = 8;
const INVITE_TOKEN_MAX_LEN: usize = 64;
const SHARE_TOKEN_LEN: usize = 22;
const LIST_SLUG_MAX_LEN: usize = 32;
const LIST_ID_XOR_KEY: u64 = 0x5A3F_1D7E;
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
pub(crate) const MAX_EXTRACTED_LINKS: usize = 16;
const MAX_PREVIEW_CONCURRENCY: usize = 4;
const PREVIEW_TOTAL_BUDGET: Duration = Duration::from_millis(1_200);

const SETTINGS_FRAGMENTS: &[&str] = &[
    "",
    "accessibility",
    "advanced",
    "appearance",
    "devices",
    "features",
    "info",
    "language",
    "notifications",
    "profile",
    "security",
    "sessions",
];

const PAGE_PATHS: &[&str] = &[
    "/",
    "/accessibility",
    "/achievements",
    "/agb",
    "/barrierefreiheit",
    "/beta",
    "/cli",
    "/credits",
    "/datenschutz",
    "/download",
    "/impressum",
    "/legal-notice",
    "/licenses",
    "/lizenzen",
    "/login",
    "/privacy",
    "/register",
    "/terms",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InternalLinkKind {
    Invite,
    PublicList,
    PrivateList,
    Dashboard,
    Folder,
    MyDay,
    Search,
    Settings,
    Page,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InternalKramliLink {
    pub(crate) kind: InternalLinkKind,
    pub(crate) canonical_url: String,
    pub(crate) display_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) list_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) item_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) folder_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) token: Option<String>,
}

impl InternalKramliLink {
    fn new(kind: InternalLinkKind, canonical_url: String) -> Self {
        Self {
            kind,
            display_url: canonical_url.clone(),
            canonical_url,
            list_id: None,
            item_id: None,
            folder_id: None,
            token: None,
        }
    }

    pub(crate) fn canonical_key(&self) -> &str {
        &self.canonical_url
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LinkPreviewActionKind {
    Open,
    Accept,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LinkPreviewAction {
    pub(crate) kind: LinkPreviewActionKind,
    pub(crate) target_url: String,
    pub(crate) requires_confirmation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LinkPreview {
    pub(crate) kind: InternalLinkKind,
    pub(crate) canonical_url: String,
    pub(crate) display_url: String,
    pub(crate) resolved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) list_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) item_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) list_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) list_icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) list_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) list_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) item_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) folder_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) folder_icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) folder_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) invited_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) action: Option<LinkPreviewAction>,
}

impl LinkPreview {
    fn unresolved(link: &InternalKramliLink) -> Self {
        Self {
            kind: link.kind,
            canonical_url: link.canonical_url.clone(),
            display_url: link.display_url.clone(),
            resolved: false,
            list_id: link.list_id,
            item_id: link.item_id,
            list_name: None,
            list_icon: None,
            list_color: None,
            list_type: None,
            item_text: None,
            folder_name: None,
            folder_icon: None,
            folder_color: None,
            role: None,
            invited_by: None,
            action: None,
        }
    }

    fn open(link: &InternalKramliLink) -> Self {
        let mut preview = Self::unresolved(link);
        preview.resolved = true;
        preview.action = Some(LinkPreviewAction {
            kind: LinkPreviewActionKind::Open,
            target_url: link.canonical_url.clone(),
            requires_confirmation: false,
        });
        preview
    }
}

#[derive(Debug, Deserialize)]
struct InternalPreviewResponse {
    #[serde(default)]
    resolved: bool,
    #[serde(default)]
    canonical_url: Option<String>,
    #[serde(default)]
    list_name: Option<String>,
    #[serde(default)]
    list_icon: Option<String>,
    #[serde(default)]
    list_color: Option<String>,
    #[serde(default)]
    list_type: Option<String>,
    #[serde(default)]
    item_text: Option<String>,
    #[serde(default)]
    folder_name: Option<String>,
    #[serde(default)]
    folder_icon: Option<String>,
    #[serde(default)]
    folder_color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct InvitePreviewResponse {
    pub(crate) list_id: u64,
    #[serde(default)]
    pub(crate) list_name: Option<String>,
    #[serde(default)]
    pub(crate) list_icon: Option<String>,
    #[serde(default)]
    pub(crate) list_color: Option<String>,
    #[serde(default)]
    pub(crate) list_type: Option<String>,
    #[serde(default)]
    pub(crate) role: Option<String>,
    #[serde(default)]
    pub(crate) invited_by: Option<String>,
    #[serde(default)]
    pub(crate) already_member: bool,
    #[serde(default)]
    pub(crate) invite_url: Option<String>,
}

pub(crate) struct LinkPreviewResolver {
    api: ApiClient,
    cache: HashMap<String, LinkPreview>,
}

impl LinkPreviewResolver {
    pub(crate) fn new(api: ApiClient) -> Self {
        Self {
            api,
            cache: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) async fn resolve_url(&mut self, value: &str) -> Option<LinkPreview> {
        let link = parse_internal_kramli_url(value)?;
        Some(self.resolve(&link).await)
    }

    pub(crate) async fn resolve_url_strict(
        &mut self,
        value: &str,
    ) -> Result<Option<LinkPreview>, String> {
        let Some(link) = parse_internal_kramli_url(value) else {
            return Ok(None);
        };
        self.resolve_strict(&link).await.map(Some)
    }

    pub(crate) async fn resolve(&mut self, link: &InternalKramliLink) -> LinkPreview {
        self.resolve_strict(link)
            .await
            .unwrap_or_else(|_| LinkPreview::unresolved(link))
    }

    async fn resolve_strict(&mut self, link: &InternalKramliLink) -> Result<LinkPreview, String> {
        if let Some(preview) = self.cache.get(link.canonical_key()) {
            return Ok(preview.clone());
        }

        let preview = match link.kind {
            InternalLinkKind::Dashboard
            | InternalLinkKind::MyDay
            | InternalLinkKind::Search
            | InternalLinkKind::Settings
            | InternalLinkKind::Page => LinkPreview::open(link),
            InternalLinkKind::Invite => self.resolve_invite(link).await?,
            InternalLinkKind::PublicList
            | InternalLinkKind::PrivateList
            | InternalLinkKind::Folder => self.resolve_metadata(link).await?,
        };
        if preview.resolved {
            self.cache
                .insert(link.canonical_key().to_string(), preview.clone());
        }
        Ok(preview)
    }

    pub(crate) async fn resolve_texts<'a>(
        &mut self,
        texts: impl IntoIterator<Item = &'a str>,
    ) -> Vec<LinkPreview> {
        let mut seen = HashSet::new();
        let mut links = Vec::new();
        for text in texts {
            for link in extract_internal_kramli_links(text) {
                if links.len() >= MAX_EXTRACTED_LINKS {
                    break;
                }
                if seen.insert(link.canonical_url.clone()) {
                    links.push(link);
                }
            }
            if links.len() >= MAX_EXTRACTED_LINKS {
                break;
            }
        }

        let mut previews = vec![None; links.len()];
        let semaphore = Arc::new(Semaphore::new(MAX_PREVIEW_CONCURRENCY));
        let mut tasks = JoinSet::new();
        for (index, link) in links.iter().cloned().enumerate() {
            if let Some(preview) = self.cache.get(link.canonical_key()) {
                previews[index] = Some(preview.clone());
                continue;
            }
            if matches!(
                link.kind,
                InternalLinkKind::Dashboard
                    | InternalLinkKind::MyDay
                    | InternalLinkKind::Search
                    | InternalLinkKind::Settings
                    | InternalLinkKind::Page
            ) {
                previews[index] = Some(LinkPreview::open(&link));
                continue;
            }
            let api = self.api.clone();
            let semaphore = Arc::clone(&semaphore);
            tasks.spawn(async move {
                let _permit = semaphore.acquire_owned().await.ok();
                let mut resolver = LinkPreviewResolver::new(api);
                let preview = resolver.resolve(&link).await;
                (index, link, preview)
            });
        }

        let completed = tokio::time::timeout(PREVIEW_TOTAL_BUDGET, async {
            let mut completed = Vec::new();
            while let Some(result) = tasks.join_next().await {
                if let Ok(result) = result {
                    completed.push(result);
                }
            }
            completed
        })
        .await;
        let completed = match completed {
            Ok(completed) => completed,
            Err(_) => {
                tasks.abort_all();
                Vec::new()
            }
        };
        for (index, link, preview) in completed {
            if preview.resolved {
                self.cache
                    .insert(link.canonical_key().to_string(), preview.clone());
            }
            previews[index] = Some(preview);
        }
        previews
            .into_iter()
            .zip(links.iter())
            .map(|(preview, link)| preview.unwrap_or_else(|| LinkPreview::unresolved(link)))
            .collect()
    }

    async fn resolve_metadata(&self, link: &InternalKramliLink) -> Result<LinkPreview, String> {
        let response = self
            .api
            .get_internal_link_preview::<InternalPreviewResponse>(&link.canonical_url)
            .await?;
        if !response.resolved {
            return Ok(LinkPreview::unresolved(link));
        }
        let canonical_url = response
            .canonical_url
            .as_deref()
            .and_then(parse_internal_kramli_url)
            .filter(|parsed| parsed.kind == link.kind)
            .map_or_else(|| link.canonical_url.clone(), |parsed| parsed.canonical_url);
        Ok(LinkPreview {
            kind: link.kind,
            display_url: canonical_url.clone(),
            canonical_url: canonical_url.clone(),
            resolved: true,
            list_id: link.list_id,
            item_id: link.item_id,
            list_name: response.list_name,
            list_icon: response.list_icon,
            list_color: response.list_color,
            list_type: response.list_type,
            item_text: response.item_text,
            folder_name: response.folder_name,
            folder_icon: response.folder_icon,
            folder_color: response.folder_color,
            role: None,
            invited_by: None,
            action: Some(LinkPreviewAction {
                kind: LinkPreviewActionKind::Open,
                target_url: canonical_url,
                requires_confirmation: false,
            }),
        })
    }

    async fn resolve_invite(&self, link: &InternalKramliLink) -> Result<LinkPreview, String> {
        let Some(token) = link.token.as_deref() else {
            return Ok(LinkPreview::unresolved(link));
        };
        let response = self
            .api
            .get_invite_link::<InvitePreviewResponse>(token)
            .await?;
        let display_url = response
            .invite_url
            .as_deref()
            .and_then(parse_internal_kramli_url)
            .filter(|parsed| parsed.kind == InternalLinkKind::Invite)
            .map_or_else(|| link.display_url.clone(), |parsed| parsed.display_url);
        let (kind, target_url, requires_confirmation) = if response.already_member {
            (
                LinkPreviewActionKind::Open,
                private_list_url(response.list_id, None),
                false,
            )
        } else {
            (LinkPreviewActionKind::Accept, display_url.clone(), true)
        };
        Ok(LinkPreview {
            kind: link.kind,
            canonical_url: link.canonical_url.clone(),
            display_url,
            resolved: true,
            list_id: Some(response.list_id),
            item_id: None,
            list_name: response.list_name,
            list_icon: response.list_icon,
            list_color: response.list_color,
            list_type: response.list_type,
            item_text: None,
            folder_name: None,
            folder_icon: None,
            folder_color: None,
            role: response.role,
            invited_by: response.invited_by,
            action: Some(LinkPreviewAction {
                kind,
                target_url,
                requires_confirmation,
            }),
        })
    }
}

pub(crate) fn parse_internal_kramli_url(value: &str) -> Option<InternalKramliLink> {
    let raw = value.trim();
    if raw.contains('\\') || raw.len() < "https://x".len() {
        return None;
    }
    let scheme = raw.get(..8)?;
    if !scheme.eq_ignore_ascii_case("https://") {
        return None;
    }
    let authority_end = raw[8..]
        .find(['/', '?', '#'])
        .map_or(raw.len(), |offset| offset + 8);
    let authority = raw.get(8..authority_end)?;
    if !matches!(
        authority.to_ascii_lowercase().as_str(),
        "kram.li" | "kramli.de"
    ) {
        return None;
    }

    let parsed = Url::parse(raw).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    if parsed.scheme() != "https"
        || !matches!(host.as_str(), "kram.li" | "kramli.de")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
    {
        return None;
    }

    let suffix = raw.get(authority_end..).unwrap_or_default();
    let raw_path_end = suffix.find(['?', '#']).unwrap_or(suffix.len());
    let raw_path = suffix.get(..raw_path_end).unwrap_or_default();
    let path = if raw_path.is_empty() { "/" } else { raw_path };
    if contains_encoded_separator(path) {
        return None;
    }
    let segments: Vec<&str> = if path == "/" {
        Vec::new()
    } else {
        path.trim_matches('/').split('/').collect()
    };
    let fragment = parsed.fragment().unwrap_or_default();

    if host == "kram.li" {
        if segments.len() == 2
            && segments[0] == "i"
            && valid_token(segments[1], INVITE_TOKEN_MIN_LEN, INVITE_TOKEN_MAX_LEN)
        {
            return Some(invite_link(segments[1]));
        }
        if matches!(segments.len(), 1 | 2)
            && (segments.len() == 1 || segments[1] == "embed")
            && valid_token(segments[0], SHARE_TOKEN_LEN, SHARE_TOKEN_LEN)
        {
            return public_list_link(segments[0], fragment);
        }
        return None;
    }

    if segments.len() == 3
        && segments[..2] == ["lists", "join"]
        && valid_token(segments[2], INVITE_TOKEN_MIN_LEN, INVITE_TOKEN_MAX_LEN)
    {
        return Some(invite_link(segments[2]));
    }
    if matches!(segments.len(), 3 | 4)
        && segments[..2] == ["lists", "s"]
        && (segments.len() == 3 || segments[3] == "embed")
        && valid_token(segments[2], SHARE_TOKEN_LEN, SHARE_TOKEN_LEN)
    {
        return public_list_link(segments[2], fragment);
    }
    if segments.len() == 2
        && segments[0] == "lists"
        && segments[1].bytes().all(|byte| byte.is_ascii_digit())
    {
        let list_id = segments[1].parse::<u64>().ok()?;
        if list_id == 0 {
            return None;
        }
        return private_list_link(list_id, fragment);
    }
    if segments.len() == 3
        && segments[..2] == ["lists", "l"]
        && !segments[2].is_empty()
        && segments[2].len() <= LIST_SLUG_MAX_LEN
        && segments[2].bytes().all(is_base62)
    {
        let list_id = decode_list_id(segments[2])?;
        if list_id == 0 {
            return None;
        }
        return private_list_link(list_id, fragment);
    }
    if path == "/lists" {
        if fragment.is_empty() {
            return Some(InternalKramliLink::new(
                InternalLinkKind::Dashboard,
                format!("{CANONICAL_ORIGIN}/lists"),
            ));
        }
        let folder_id = positive_fragment_id(fragment, "folder-")?;
        let mut link = InternalKramliLink::new(
            InternalLinkKind::Folder,
            format!("{CANONICAL_ORIGIN}/lists#folder-{folder_id}"),
        );
        link.folder_id = Some(folder_id);
        return Some(link);
    }
    if path == "/lists/my-day" && fragment.is_empty() {
        return Some(InternalKramliLink::new(
            InternalLinkKind::MyDay,
            format!("{CANONICAL_ORIGIN}/lists/my-day"),
        ));
    }
    if path == "/lists/search" && fragment.is_empty() {
        let mut query = None;
        for (key, value) in parsed.query_pairs() {
            if key != "q" || query.is_some() {
                return None;
            }
            query = Some(value.chars().take(200).collect::<String>());
        }
        let mut canonical = format!("{CANONICAL_ORIGIN}/lists/search");
        if let Some(query) = query.filter(|query| !query.is_empty()) {
            let mut url = Url::parse(&canonical).ok()?;
            url.query_pairs_mut().append_pair("q", &query);
            canonical = url.into();
        }
        return Some(InternalKramliLink::new(InternalLinkKind::Search, canonical));
    }
    if path == "/settings" {
        let section = fragment.trim().to_ascii_lowercase();
        if !SETTINGS_FRAGMENTS.contains(&section.as_str()) {
            return None;
        }
        let suffix = if section.is_empty() {
            String::new()
        } else {
            format!("#{section}")
        };
        return Some(InternalKramliLink::new(
            InternalLinkKind::Settings,
            format!("{CANONICAL_ORIGIN}/settings{suffix}"),
        ));
    }
    if PAGE_PATHS.contains(&path) && fragment.is_empty() {
        return Some(InternalKramliLink::new(
            InternalLinkKind::Page,
            format!("{CANONICAL_ORIGIN}{path}"),
        ));
    }
    None
}

pub(crate) fn extract_internal_kramli_links(text: &str) -> Vec<InternalKramliLink> {
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    let mut offset = 0;
    while offset < text.len() && links.len() < MAX_EXTRACTED_LINKS {
        let Some(start) = find_http_candidate(&text[offset..]).map(|start| start + offset) else {
            break;
        };
        let tail = &text[start..];
        let end = tail
            .char_indices()
            .find_map(|(index, ch)| {
                (ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\'')).then_some(index)
            })
            .unwrap_or(tail.len());
        let candidate = trim_sentence_punctuation(&tail[..end]);
        if let Some(link) = parse_internal_kramli_url(candidate) {
            if seen.insert(link.canonical_key().to_string()) {
                links.push(link);
            }
        }
        offset = start + end.max(1);
    }
    links
}

fn find_http_candidate(text: &str) -> Option<usize> {
    text.char_indices().map(|(index, _)| index).find(|&index| {
        text.get(index..index + 7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
            || text
                .get(index..index + 8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
    })
}

fn trim_sentence_punctuation(value: &str) -> &str {
    value.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']'])
}

fn contains_encoded_separator(path: &str) -> bool {
    path.as_bytes().windows(3).any(|window| {
        window[0] == b'%' && window[1] == b'2' && window[2].eq_ignore_ascii_case(&b'f')
            || window[0] == b'%' && window[1] == b'5' && window[2].eq_ignore_ascii_case(&b'c')
    })
}

fn valid_token(value: &str, min_len: usize, max_len: usize) -> bool {
    (min_len..=max_len).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn positive_fragment_id(fragment: &str, prefix: &str) -> Option<u64> {
    let value = fragment.strip_prefix(prefix)?;
    if value.is_empty()
        || value.starts_with('0')
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok()
}

fn item_id(fragment: &str) -> Option<Option<u64>> {
    if fragment.is_empty() {
        Some(None)
    } else {
        positive_fragment_id(fragment, "item-").map(Some)
    }
}

fn invite_link(token: &str) -> InternalKramliLink {
    let mut link = InternalKramliLink::new(
        InternalLinkKind::Invite,
        format!("{CANONICAL_ORIGIN}/lists/join/{token}"),
    );
    link.display_url = format!("{SHORT_ORIGIN}/i/{token}");
    link.token = Some(token.to_string());
    link
}

fn public_list_link(token: &str, fragment: &str) -> Option<InternalKramliLink> {
    let item_id = item_id(fragment)?;
    let canonical_url = with_item_fragment(format!("{CANONICAL_ORIGIN}/lists/s/{token}"), item_id);
    let mut link = InternalKramliLink::new(InternalLinkKind::PublicList, canonical_url);
    link.item_id = item_id;
    link.token = Some(token.to_string());
    Some(link)
}

fn private_list_link(list_id: u64, fragment: &str) -> Option<InternalKramliLink> {
    let item_id = item_id(fragment)?;
    let mut link = InternalKramliLink::new(
        InternalLinkKind::PrivateList,
        private_list_url(list_id, item_id),
    );
    link.list_id = Some(list_id);
    link.item_id = item_id;
    Some(link)
}

pub(crate) fn private_list_url(list_id: u64, item_id: Option<u64>) -> String {
    with_item_fragment(
        format!("{CANONICAL_ORIGIN}/lists/l/{}", encode_list_id(list_id)),
        item_id,
    )
}

fn with_item_fragment(mut url: String, item_id: Option<u64>) -> String {
    if let Some(item_id) = item_id {
        url.push_str(&format!("#item-{item_id}"));
    }
    url
}

fn encode_list_id(list_id: u64) -> String {
    let mut value = list_id ^ LIST_ID_XOR_KEY;
    if value == 0 {
        return "0".to_string();
    }
    let mut encoded = Vec::new();
    while value > 0 {
        encoded.push(BASE62[(value % 62) as usize]);
        value /= 62;
    }
    encoded.reverse();
    String::from_utf8(encoded).expect("base62 alphabet is utf-8")
}

fn decode_list_id(slug: &str) -> Option<u64> {
    let mut value = 0_u64;
    for byte in slug.bytes() {
        let digit = BASE62.iter().position(|candidate| *candidate == byte)? as u64;
        value = value.checked_mul(62)?.checked_add(digit)?;
    }
    Some(value ^ LIST_ID_XOR_KEY)
}

fn is_base62(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{json, Value};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    struct TestResponse {
        status: u16,
        body: Value,
    }

    async fn resolver_with_responses(
        responses: Vec<TestResponse>,
    ) -> (LinkPreviewResolver, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server address");
        let base_url = format!("http://{addr}");
        let ready = (!responses.is_empty())
            .then(|| crate::test_env::register_mock_server(base_url.clone()));
        let server = tokio::spawn(async move {
            if let Some(ready) = ready {
                let _ = ready.await;
            }
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept())
                        .await
                        .expect("test server accept timed out")
                        .expect("accept request");
                let mut buffer = vec![0_u8; 8192];
                let size = stream.read(&mut buffer).await.expect("read request");
                let request = String::from_utf8_lossy(&buffer[..size]).to_string();
                requests.push(request.lines().next().unwrap_or_default().to_string());
                let body = response.body.to_string();
                let header = format!(
                    "HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.status,
                    body.len()
                );
                stream
                    .write_all(header.as_bytes())
                    .await
                    .expect("write header");
                stream.write_all(body.as_bytes()).await.expect("write body");
            }
            requests
        });
        (
            LinkPreviewResolver::new(ApiClient::for_tests(&base_url)),
            server,
        )
    }

    async fn resolver_with_delayed_metadata(
        count: usize,
        delay: Duration,
    ) -> (LinkPreviewResolver, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server address");
        let base_url = format!("http://{addr}");
        let ready = (count > 0).then(|| crate::test_env::register_mock_server(base_url.clone()));
        let server = tokio::spawn(async move {
            if let Some(ready) = ready {
                let _ = ready.await;
            }
            let mut requests = Vec::new();
            let mut responses = JoinSet::new();
            for _ in 0..count {
                let (mut stream, _) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept())
                        .await
                        .expect("test server accept timed out")
                        .expect("accept request");
                let mut buffer = vec![0_u8; 8192];
                let size = stream.read(&mut buffer).await.expect("read request");
                let request = String::from_utf8_lossy(&buffer[..size]).to_string();
                requests.push(request.lines().next().unwrap_or_default().to_string());
                responses.spawn(async move {
                    tokio::time::sleep(delay).await;
                    let body = json!({"resolved": true}).to_string();
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    stream.write_all(header.as_bytes()).await.unwrap();
                    stream.write_all(body.as_bytes()).await.unwrap();
                });
            }
            while responses.join_next().await.is_some() {}
            requests
        });
        (
            LinkPreviewResolver::new(ApiClient::for_tests(&base_url)),
            server,
        )
    }

    #[test]
    fn parser_accepts_and_canonicalizes_supported_routes() {
        let slug = encode_list_id(42);
        let public_token = "A".repeat(SHARE_TOKEN_LEN);
        let cases = [
            (
                "https://kramli.de/lists/42#item-7".to_string(),
                InternalLinkKind::PrivateList,
                format!("{CANONICAL_ORIGIN}/lists/l/{slug}#item-7"),
            ),
            (
                format!("https://kramli.de/lists/l/{slug}"),
                InternalLinkKind::PrivateList,
                format!("{CANONICAL_ORIGIN}/lists/l/{slug}"),
            ),
            (
                format!("{SHORT_ORIGIN}/{public_token}"),
                InternalLinkKind::PublicList,
                format!("{CANONICAL_ORIGIN}/lists/s/{public_token}"),
            ),
            (
                format!("{CANONICAL_ORIGIN}/lists/s/{public_token}#item-8"),
                InternalLinkKind::PublicList,
                format!("{CANONICAL_ORIGIN}/lists/s/{public_token}#item-8"),
            ),
            (
                "https://kramli.de/lists#folder-9".to_string(),
                InternalLinkKind::Folder,
                "https://kramli.de/lists#folder-9".to_string(),
            ),
            (
                "https://kramli.de/lists/my-day".to_string(),
                InternalLinkKind::MyDay,
                "https://kramli.de/lists/my-day".to_string(),
            ),
            (
                "https://kramli.de/lists/search?q=Milch".to_string(),
                InternalLinkKind::Search,
                "https://kramli.de/lists/search?q=Milch".to_string(),
            ),
            (
                "https://kramli.de/settings#security".to_string(),
                InternalLinkKind::Settings,
                "https://kramli.de/settings#security".to_string(),
            ),
            (
                "https://kramli.de/privacy".to_string(),
                InternalLinkKind::Page,
                "https://kramli.de/privacy".to_string(),
            ),
        ];

        for (raw, kind, canonical) in cases {
            let parsed = parse_internal_kramli_url(&raw).expect("accepted route");
            assert_eq!(parsed.kind, kind, "{raw}");
            assert_eq!(parsed.canonical_url, canonical, "{raw}");
        }
    }

    #[test]
    fn parser_supports_invites_embeds_settings_pages_and_search() {
        let invite = parse_internal_kramli_url("https://kram.li/i/Abcdef_123-xy").unwrap();
        assert_eq!(invite.kind, InternalLinkKind::Invite);
        assert_eq!(invite.display_url, "https://kram.li/i/Abcdef_123-xy");
        assert_eq!(
            invite.canonical_url,
            "https://kramli.de/lists/join/Abcdef_123-xy"
        );

        let token = "PublicToken1234567890A";
        assert_eq!(token.len(), SHARE_TOKEN_LEN);
        let public =
            parse_internal_kramli_url(&format!("https://kram.li/{token}/embed#item-2")).unwrap();
        assert_eq!(public.item_id, Some(2));
        assert_eq!(
            public.canonical_url,
            format!("https://kramli.de/lists/s/{token}#item-2")
        );

        for section in SETTINGS_FRAGMENTS {
            assert!(parse_internal_kramli_url(&format!(
                "https://kramli.de/settings{}",
                if section.is_empty() {
                    String::new()
                } else {
                    format!("#{section}")
                }
            ))
            .is_some());
        }
        for path in PAGE_PATHS {
            assert!(parse_internal_kramli_url(&format!("https://kramli.de{path}")).is_some());
        }
        assert_eq!(
            parse_internal_kramli_url("https://kramli.de/lists/search?q=Milch+%26+Brot")
                .unwrap()
                .canonical_url,
            "https://kramli.de/lists/search?q=Milch+%26+Brot"
        );
        assert_eq!(
            parse_internal_kramli_url("https://kramli.de/lists/search?q=")
                .unwrap()
                .canonical_url,
            "https://kramli.de/lists/search"
        );
    }

    #[test]
    fn parser_rejects_unverified_ambiguous_and_malformed_urls() {
        let rejected = [
            "kramli://lists/42",
            "http://kramli.de/lists/42",
            "https://kramli.de.evil.test/lists/42",
            "https://kramli.de@evil.test/lists/42",
            "https://user@kramli.de/lists/42",
            "https://kramli.de:443/lists/42",
            "https://kramli.de/lists%2F42",
            "https://kramli.de/lists%5c42",
            "https://kramli.de/lists\\42",
            "https://kramli.de/lists/42#item-0",
            "https://kramli.de/lists/42#item-01",
            "https://kramli.de/lists/42#item-x",
            "https://kramli.de/settings#unknown",
            "https://kramli.de/lists/search?q=x&next=/settings",
            "https://kramli.de/lists/search?q=x&q=y",
            "https://kramli.de/lists/search#item-1",
            "https://kram.li/lists/42",
            "https://auth.kram.li/lists/42",
            "https://kramli.de/lists/0",
            "https://kramli.de/lists/l/!",
            "https://kramli.de/lists/s/short",
            "https://kramli.de/lists/s/AAAAAAAAAAAAAAAAAAAAAA/other",
            "https://kram.li/i/short",
            "https://kramli.de/unknown",
            "https://kramli.de/privacy#x",
            "https://kramli.de/lists#folder-0",
        ];
        for raw in rejected {
            assert!(parse_internal_kramli_url(raw).is_none(), "{raw}");
        }
    }

    #[test]
    fn extraction_preserves_order_trims_punctuation_and_deduplicates_aliases() {
        let slug = encode_list_id(42);
        let text = format!(
            "First (https://kramli.de/lists/42#item-7). Then https://example.test/x, \
             alias https://kramli.de/lists/l/{slug}#item-7; invite \
             https://kram.li/i/Abcdef_123-xy! canonical \
             https://kramli.de/lists/join/Abcdef_123-xy and https://kramli.de/privacy]."
        );
        let links = extract_internal_kramli_links(&text);
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].kind, InternalLinkKind::PrivateList);
        assert_eq!(links[1].kind, InternalLinkKind::Invite);
        assert_eq!(links[1].display_url, "https://kram.li/i/Abcdef_123-xy");
        assert_eq!(links[2].kind, InternalLinkKind::Page);
    }

    #[test]
    fn extraction_caps_results_and_ignores_malformed_kramli_candidates() {
        let mut text = String::from("https://kramli.de:443/lists/1 https://kramli.de/lists%2F2 ");
        for id in 1..=(MAX_EXTRACTED_LINKS + 5) {
            text.push_str(&format!("https://kramli.de/lists/{id} "));
        }
        let links = extract_internal_kramli_links(&text);
        assert_eq!(links.len(), MAX_EXTRACTED_LINKS);
        assert_eq!(links.first().and_then(|link| link.list_id), Some(1));
        assert_eq!(
            links.last().and_then(|link| link.list_id),
            Some(MAX_EXTRACTED_LINKS as u64)
        );
    }

    #[test]
    fn preview_models_serialize_action_contract() {
        let link = parse_internal_kramli_url("https://kramli.de/privacy").unwrap();
        let value = serde_json::to_value(LinkPreview::open(&link)).unwrap();
        assert_eq!(value["action"]["kind"], "open");
        assert_eq!(value["action"]["requires_confirmation"], false);
        assert!(value.get("list_name").is_none());
    }

    #[tokio::test]
    async fn resolver_uses_get_endpoints_actions_and_cache_without_posting() {
        let (mut resolver, server) = resolver_with_responses(vec![
            TestResponse {
                status: 200,
                body: json!({
                    "resolved": true,
                    "canonical_url": "https://kramli.de/lists/42#item-7",
                    "list_name": "Wochenende",
                    "item_text": "Milch"
                }),
            },
            TestResponse {
                status: 200,
                body: json!({
                    "list_id": 42,
                    "list_name": "Wochenende",
                    "already_member": false,
                    "invite_url": "https://kram.li/i/Abcdef_123-xy"
                }),
            },
            TestResponse {
                status: 200,
                body: json!({
                    "list_id": 43,
                    "list_name": "Mitgliedsliste",
                    "already_member": true,
                    "invite_url": "https://kram.li/i/MemberToken_1"
                }),
            },
        ])
        .await;

        let private = resolver
            .resolve_url("https://kramli.de/lists/42#item-7")
            .await
            .unwrap();
        assert!(private.resolved);
        assert_eq!(private.item_id, Some(7));
        assert_eq!(private.item_text.as_deref(), Some("Milch"));
        assert_eq!(private.action.unwrap().kind, LinkPreviewActionKind::Open);
        let _cached_alias = resolver
            .resolve_url(&format!(
                "https://kramli.de/lists/l/{}#item-7",
                encode_list_id(42)
            ))
            .await
            .unwrap();

        let invite = resolver
            .resolve_url("https://kramli.de/lists/join/Abcdef_123-xy")
            .await
            .unwrap();
        let invite_action = invite.action.unwrap();
        assert_eq!(invite.display_url, "https://kram.li/i/Abcdef_123-xy");
        assert_eq!(invite_action.kind, LinkPreviewActionKind::Accept);
        assert!(invite_action.requires_confirmation);

        let member = resolver
            .resolve_url("https://kram.li/i/MemberToken_1")
            .await
            .unwrap();
        let member_action = member.action.unwrap();
        assert_eq!(member_action.kind, LinkPreviewActionKind::Open);
        assert!(!member_action.requires_confirmation);
        assert_eq!(member_action.target_url, private_list_url(43, None));

        let requests = server.await.expect("server finished");
        assert_eq!(requests.len(), 3);
        assert!(requests[0].starts_with("GET /api/internal-links/preview?"));
        assert_eq!(requests[1], "GET /api/invite-links/Abcdef_123-xy HTTP/1.1");
        assert_eq!(requests[2], "GET /api/invite-links/MemberToken_1 HTTP/1.1");
        assert!(requests.iter().all(|request| request.starts_with("GET ")));
    }

    #[tokio::test]
    async fn best_effort_errors_are_not_cached_and_strict_errors_propagate() {
        let (mut resolver, server) = resolver_with_responses(vec![
            TestResponse {
                status: 403,
                body: json!({"error": "forbidden"}),
            },
            TestResponse {
                status: 410,
                body: json!({"error": "expired"}),
            },
        ])
        .await;
        let first = resolver
            .resolve_url("https://kram.li/i/InviteToken_1")
            .await
            .unwrap();
        let strict = resolver
            .resolve_url_strict("https://kram.li/i/InviteToken_1")
            .await
            .expect_err("strict inspection must propagate the HTTP failure");
        assert!(!first.resolved);
        assert!(first.action.is_none());
        assert!(strict.contains("expired") || strict.contains("410"));
        assert_eq!(server.await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn resolver_deduplicates_aliases_across_displayed_fields_in_source_order() {
        let (mut resolver, server) = resolver_with_responses(vec![TestResponse {
            status: 200,
            body: json!({"resolved": true, "list_name": "List 42"}),
        }])
        .await;
        let slug = encode_list_id(42);
        let fields = [
            "before https://kramli.de/lists/42".to_string(),
            format!("alias https://kramli.de/lists/l/{slug} then https://kramli.de/privacy"),
        ];
        let previews = resolver
            .resolve_texts(fields.iter().map(String::as_str))
            .await;
        assert_eq!(previews.len(), 2);
        assert_eq!(previews[0].kind, InternalLinkKind::PrivateList);
        assert_eq!(previews[1].kind, InternalLinkKind::Page);
        assert_eq!(server.await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn resolver_enriches_concurrently_within_budget_and_preserves_source_order() {
        let (mut resolver, server) =
            resolver_with_delayed_metadata(4, Duration::from_millis(200)).await;
        let text = (1..=4)
            .map(|id| format!("https://kramli.de/lists/{id}"))
            .collect::<Vec<_>>()
            .join(" ");
        let started = tokio::time::Instant::now();
        let previews = resolver.resolve_texts([text.as_str()]).await;
        assert!(started.elapsed() < Duration::from_millis(700));
        assert_eq!(
            previews
                .iter()
                .map(|preview| preview.list_id)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(3), Some(4)]
        );
        assert!(previews.iter().all(|preview| preview.resolved));
        assert_eq!(server.await.unwrap().len(), 4);
    }

    #[tokio::test]
    async fn resolver_returns_unresolved_placeholders_when_total_budget_expires() {
        let (mut resolver, server) =
            resolver_with_delayed_metadata(1, Duration::from_secs(2)).await;
        let started = tokio::time::Instant::now();
        let previews = resolver.resolve_texts(["https://kramli.de/lists/42"]).await;
        assert!(started.elapsed() < Duration::from_millis(1_700));
        assert_eq!(previews.len(), 1);
        assert!(!previews[0].resolved);
        server.abort();
    }

    #[tokio::test]
    async fn resolver_never_fetches_external_urls_and_static_routes_need_no_api() {
        let api = ApiClient::for_tests("http://127.0.0.1:9");
        let mut resolver = LinkPreviewResolver::new(api);
        assert!(resolver
            .resolve_url("https://example.test/lists/42")
            .await
            .is_none());
        let page = resolver
            .resolve_url("https://kramli.de/privacy")
            .await
            .unwrap();
        assert!(page.resolved);
        assert_eq!(page.action.unwrap().kind, LinkPreviewActionKind::Open);
    }
}
