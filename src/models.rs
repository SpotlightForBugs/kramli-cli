use serde::{Deserialize, Serialize};

use crate::i18n::{tr, tr_args};

// ── List ──

#[derive(Debug, Serialize, Deserialize)]
pub struct ShoppingList {
    pub id: i64,
    pub name: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub folder_id: Option<i64>,
    pub folder_name: Option<String>,
    pub archived: Option<bool>,
    pub archive_mode: Option<String>,
    pub view_mode: Option<String>,
    pub role: Option<String>,
    pub item_count: Option<i64>,
    pub done_count: Option<i64>,
    pub state_config: Option<String>,
    pub states: Option<Vec<ListState>>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListState {
    pub name: Option<String>,
    pub color: Option<String>,
    pub is_done: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct CreateList {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<i64>,
}

// ── Item ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListItem {
    pub id: i64,
    pub list_id: Option<i64>,
    pub text: String,
    pub is_done: Option<bool>,
    pub quantity: Option<String>,
    pub notes: Option<String>,
    pub tldr: Option<String>,
    pub due_date: Option<String>,
    pub due_time: Option<String>,
    pub planned_date: Option<String>,
    pub planned_time: Option<String>,
    pub repeat_label: Option<String>,
    pub reminder: Option<bool>,
    pub reminder_time: Option<String>,
    pub reminder_days_before: Option<i64>,
    pub reminder_offsets: Option<Vec<i64>>,
    pub travel_time_minutes: Option<i64>,
    pub priority: Option<String>,
    pub progress: Option<String>,
    pub tags: Option<Vec<String>>,
    pub parent_item_id: Option<i64>,
    pub depth: Option<i64>,
    pub position: Option<i64>,
    pub completed_at: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub assigned_to: Option<Vec<i64>>,
    pub child_count: Option<i64>,
    pub done_child_count: Option<i64>,
    pub comment_count: Option<i64>,
    pub color: Option<String>,
    pub image_url: Option<String>,
    pub image_filename: Option<String>,
    pub attachments: Option<Vec<Attachment>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: i64,
    pub filename: Option<String>,
    pub original_filename: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemComment {
    pub id: i64,
    pub text: Option<String>,
    pub user_id: Option<i64>,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub created_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct CreateItem {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planned_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planned_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reminder: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reminder_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reminder_days_before: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reminder_offsets: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub travel_time_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_item_id: Option<i64>,
}

// ── Folder ──

#[derive(Debug, Serialize, Deserialize)]
pub struct Folder {
    pub id: i64,
    pub name: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    #[serde(default, alias = "parent_id", alias = "parentFolderId")]
    pub parent_folder_id: Option<i64>,
    #[serde(default, alias = "parent_name", alias = "parentFolderName")]
    pub parent_folder_name: Option<String>,
    pub position: Option<i64>,
    pub created_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct CreateFolder {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_folder_id: Option<i64>,
}

// ── Member ──

#[derive(Debug, Serialize, Deserialize)]
pub struct Member {
    pub user_id: Option<i64>,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub role: Option<String>,
    #[serde(rename = "type")]
    pub member_type: Option<String>,
}

// ── Profile ──

#[derive(Debug, Serialize, Deserialize)]
pub struct Profile {
    pub id: Option<i64>,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub photo_url: Option<String>,
    #[serde(default, alias = "language", alias = "locale")]
    pub lang: Option<String>,
    pub is_anonymous: Option<bool>,
    pub created_at: Option<String>,
    #[serde(default)]
    pub legal: Option<ProfileLegalStatus>,
    #[serde(default)]
    pub terms_accepted: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileLegalStatus {
    #[serde(default)]
    pub pending: Vec<ProfilePendingLegalDoc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilePendingLegalDoc {
    pub key: Option<String>,
}

// ── Search ──

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SearchResults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lists: Option<Vec<SearchListHit>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<SearchItemHit>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchHit {
    #[serde(rename = "type")]
    pub hit_type: Option<String>,
    pub id: i64,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub list_id: Option<i64>,
    pub list_name: Option<String>,
    pub list_icon: Option<String>,
    pub text: Option<String>,
    pub is_done: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SearchResponse {
    Grouped(SearchResults),
    Flat(Vec<SearchHit>),
}

impl SearchResponse {
    pub fn from_value(value: serde_json::Value) -> Result<Self, String> {
        match value {
            serde_json::Value::Null => Ok(Self::Flat(Vec::new())),
            serde_json::Value::Array(array) => Self::from_array(array),
            serde_json::Value::Object(mut object) => {
                if let Some(nested) = object.remove("results").or_else(|| object.remove("data")) {
                    return Self::from_value(nested);
                }

                if object.is_empty() {
                    return Ok(Self::Grouped(SearchResults::default()));
                }

                let object_value = serde_json::Value::Object(object.clone());
                if let Ok(grouped) = serde_json::from_value::<SearchResults>(object_value.clone()) {
                    if grouped.lists.is_some() || grouped.items.is_some() {
                        return Ok(Self::Grouped(grouped));
                    }
                }

                if let Ok(hit) = serde_json::from_value::<SearchHit>(object_value) {
                    return Ok(Self::Flat(vec![hit]));
                }

                Err(tr("models-search-object-shape-unsupported"))
            }
            other => Err(format!(
                "{}: {}",
                tr("models-search-type-unsupported"),
                match other {
                    serde_json::Value::Bool(_) => "bool",
                    serde_json::Value::Number(_) => "number",
                    serde_json::Value::String(_) => "string",
                    _ => "unknown",
                }
            )),
        }
    }

    fn from_array(array: Vec<serde_json::Value>) -> Result<Self, String> {
        if array.is_empty() {
            return Ok(Self::Flat(Vec::new()));
        }

        let array_value = serde_json::Value::Array(array.clone());
        if let Ok(item_hits) = serde_json::from_value::<Vec<SearchItemHit>>(array_value.clone()) {
            let flat_hits = item_hits
                .into_iter()
                .map(|item| SearchHit {
                    hit_type: Some("item".to_string()),
                    id: item.id,
                    name: None,
                    icon: None,
                    list_id: item.list_id,
                    list_name: item.list_name,
                    list_icon: None,
                    text: Some(item.text),
                    is_done: item.is_done,
                })
                .collect();
            return Ok(Self::Flat(flat_hits));
        }

        if let Ok(list_hits) = serde_json::from_value::<Vec<SearchListHit>>(array_value.clone()) {
            let flat_hits = list_hits
                .into_iter()
                .map(|list| SearchHit {
                    hit_type: Some("list".to_string()),
                    id: list.id,
                    name: Some(list.name),
                    icon: list.icon,
                    list_id: None,
                    list_name: None,
                    list_icon: None,
                    text: None,
                    is_done: None,
                })
                .collect();
            return Ok(Self::Flat(flat_hits));
        }

        if let Ok(flat_hits) = serde_json::from_value::<Vec<SearchHit>>(array_value) {
            return Ok(Self::Flat(flat_hits));
        }

        Err(tr("models-search-array-shape-unsupported"))
    }

    pub fn into_grouped(self) -> SearchResults {
        match self {
            Self::Grouped(results) => results,
            Self::Flat(hits) => {
                let mut lists: Vec<SearchListHit> = Vec::new();
                let mut items: Vec<SearchItemHit> = Vec::new();

                for hit in hits {
                    match hit.hit_type.as_deref() {
                        Some("list") => lists.push(SearchListHit {
                            id: hit.id,
                            name: hit.name.unwrap_or_else(|| {
                                tr_args("models-list-fallback-name", &[("id", hit.id.to_string())])
                            }),
                            icon: hit.icon.or(hit.list_icon),
                            color: None,
                        }),
                        Some("item") => items.push(SearchItemHit {
                            id: hit.id,
                            text: hit.text.unwrap_or_else(|| {
                                tr_args("models-item-fallback-name", &[("id", hit.id.to_string())])
                            }),
                            list_id: hit.list_id,
                            list_name: hit.list_name,
                            is_done: hit.is_done,
                        }),
                        _ => {}
                    }
                }

                SearchResults {
                    lists: (!lists.is_empty()).then_some(lists),
                    items: (!items.is_empty()).then_some(items),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiKey, ApiKeyScopes, Profile, SearchResponse};

    #[test]
    fn parse_search_response_accepts_empty_array() {
        let value = serde_json::json!([]);
        let parsed = SearchResponse::from_value(value).expect("should parse empty array");
        let grouped = parsed.into_grouped();
        assert!(grouped.lists.is_none());
        assert!(grouped.items.is_none());
    }

    #[test]
    fn parse_search_response_accepts_flat_hits() {
        let value = serde_json::json!([
            {
                "type": "item",
                "id": 42,
                "text": "Sample",
                "list_id": 7,
                "list_name": "Inbox",
                "is_done": false
            }
        ]);
        let parsed = SearchResponse::from_value(value).expect("should parse flat hits");
        let grouped = parsed.into_grouped();
        let items = grouped.items.expect("items expected");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, 42);
        assert_eq!(items[0].text, "Sample");
    }

    #[test]
    fn parse_search_response_infers_list_arrays_without_type() {
        let value = serde_json::json!([
            {
                "id": 7,
                "name": "Groceries",
                "icon": "basket"
            }
        ]);

        let parsed = SearchResponse::from_value(value).expect("should parse list hit array");
        let grouped = parsed.into_grouped();
        let lists = grouped.lists.expect("lists expected");
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].id, 7);
        assert_eq!(lists[0].name, "Groceries");
    }

    #[test]
    fn parse_search_response_infers_item_arrays_without_type() {
        let value = serde_json::json!([
            {
                "id": 42,
                "text": "Milk",
                "list_id": 7,
                "list_name": "Groceries",
                "is_done": false
            }
        ]);

        let parsed = SearchResponse::from_value(value).expect("should parse item hit array");
        let grouped = parsed.into_grouped();
        let items = grouped.items.expect("items expected");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, 42);
        assert_eq!(items[0].text, "Milk");
    }

    #[test]
    fn parse_profile_lang_aliases() {
        let from_lang: Profile = serde_json::from_value(serde_json::json!({"lang": "fr"}))
            .expect("profile with lang should parse");
        assert_eq!(from_lang.lang.as_deref(), Some("fr"));

        let from_language: Profile =
            serde_json::from_value(serde_json::json!({"language": "pt-BR"}))
                .expect("profile with language should parse");
        assert_eq!(from_language.lang.as_deref(), Some("pt-BR"));

        let from_locale: Profile = serde_json::from_value(serde_json::json!({"locale": "de_DE"}))
            .expect("profile with locale should parse");
        assert_eq!(from_locale.lang.as_deref(), Some("de_DE"));
    }

    #[test]
    fn parse_api_key_scopes_array() {
        let key: ApiKey = serde_json::from_value(serde_json::json!({
            "id": 1,
            "name": "CLI",
            "scopes": ["all", "lists:read"],
            "is_active": true
        }))
        .expect("api key with array scopes should parse");

        match key.scopes {
            Some(ApiKeyScopes::Multiple(values)) => {
                assert_eq!(values, vec!["all", "lists:read"]);
            }
            _ => panic!("expected multiple scopes"),
        }
    }

    #[test]
    fn parse_api_key_scopes_string() {
        let key: ApiKey = serde_json::from_value(serde_json::json!({
            "id": 2,
            "name": "Legacy",
            "scopes": "all",
            "is_active": true
        }))
        .expect("api key with string scopes should parse");

        match key.scopes {
            Some(ApiKeyScopes::Single(value)) => assert_eq!(value, "all"),
            _ => panic!("expected single scope"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchListHit {
    pub id: i64,
    pub name: String,
    pub icon: Option<String>,
    pub color: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchItemHit {
    pub id: i64,
    pub text: String,
    pub list_id: Option<i64>,
    pub list_name: Option<String>,
    pub is_done: Option<bool>,
}

// ── Activity ──

#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub id: Option<i64>,
    pub list_id: Option<i64>,
    pub user_id: Option<i64>,
    pub action: Option<String>,
    pub detail: Option<serde_json::Value>,
    pub display_name: Option<String>,
    #[serde(alias = "user_name")]
    pub user_name: Option<String>,
    pub nickname: Option<String>,
    pub photo_url: Option<String>,
    pub item_id: Option<i64>,
    pub created_at: Option<String>,
}

// ── Generic OK ──

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize)]
pub struct OkResponse {
    pub ok: Option<bool>,
    pub undo_token: Option<String>,
}

// ── API Key ──

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: i64,
    pub name: Option<String>,
    pub scopes: Option<ApiKeyScopes>,
    pub is_active: Option<bool>,
    pub last_used_at: Option<String>,
    pub usage_count: Option<i64>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ApiKeyScopes {
    Single(String),
    Multiple(Vec<String>),
}
