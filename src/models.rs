use serde::{Deserialize, Serialize};

use crate::i18n::{tr, tr_args};

// ── List ──

#[derive(Debug, Serialize, Deserialize)]
/// Shopping list returned by the Kramli API.
pub(crate) struct ShoppingList {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) icon: Option<String>,
    pub(crate) color: Option<String>,
    pub(crate) folder_id: Option<i64>,
    pub(crate) folder_name: Option<String>,
    pub(crate) archived: Option<bool>,
    pub(crate) archive_mode: Option<String>,
    pub(crate) view_mode: Option<String>,
    pub(crate) role: Option<String>,
    pub(crate) item_count: Option<i64>,
    pub(crate) done_count: Option<i64>,
    pub(crate) state_config: Option<String>,
    pub(crate) states: Option<Vec<ListState>>,
    pub(crate) created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Custom workflow state configured for a list.
pub(crate) struct ListState {
    pub(crate) name: Option<String>,
    pub(crate) color: Option<String>,
    pub(crate) is_done: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
/// Request payload for creating a list.
pub(crate) struct CreateList {
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) folder_id: Option<i64>,
}

// ── Item ──

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Item returned by the Kramli API.
pub(crate) struct ListItem {
    pub(crate) id: i64,
    pub(crate) list_id: Option<i64>,
    pub(crate) text: String,
    pub(crate) is_done: Option<bool>,
    pub(crate) quantity: Option<String>,
    pub(crate) notes: Option<String>,
    pub(crate) tldr: Option<String>,
    pub(crate) due_date: Option<String>,
    pub(crate) due_time: Option<String>,
    pub(crate) planned_date: Option<String>,
    pub(crate) planned_time: Option<String>,
    pub(crate) repeat_label: Option<String>,
    pub(crate) reminder: Option<bool>,
    pub(crate) reminder_time: Option<String>,
    pub(crate) reminder_days_before: Option<i64>,
    pub(crate) reminder_offsets: Option<Vec<i64>>,
    pub(crate) travel_time_minutes: Option<i64>,
    pub(crate) priority: Option<String>,
    pub(crate) progress: Option<String>,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) parent_item_id: Option<i64>,
    pub(crate) depth: Option<i64>,
    pub(crate) position: Option<i64>,
    pub(crate) completed_at: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) assigned_to: Option<Vec<i64>>,
    pub(crate) child_count: Option<i64>,
    pub(crate) done_child_count: Option<i64>,
    pub(crate) comment_count: Option<i64>,
    pub(crate) color: Option<String>,
    pub(crate) image_url: Option<String>,
    pub(crate) image_filename: Option<String>,
    pub(crate) attachments: Option<Vec<Attachment>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// File attachment metadata for a list item.
pub(crate) struct Attachment {
    pub(crate) id: i64,
    pub(crate) filename: Option<String>,
    pub(crate) original_filename: Option<String>,
    pub(crate) mime_type: Option<String>,
    pub(crate) file_size: Option<i64>,
    pub(crate) url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Comment attached to a list item.
pub(crate) struct ItemComment {
    pub(crate) id: i64,
    pub(crate) text: Option<String>,
    pub(crate) user_id: Option<i64>,
    pub(crate) user_name: Option<String>,
    pub(crate) user_email: Option<String>,
    pub(crate) created_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
/// Request payload for creating an item.
pub(crate) struct CreateItem {
    pub(crate) text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) quantity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) due_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) due_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) planned_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) planned_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reminder: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reminder_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reminder_days_before: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reminder_offsets: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) travel_time_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) priority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parent_item_id: Option<i64>,
}

// ── Folder ──

#[derive(Debug, Serialize, Deserialize)]
/// Folder returned by the Kramli API.
pub(crate) struct Folder {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) icon: Option<String>,
    pub(crate) color: Option<String>,
    #[serde(default, alias = "parent_id", alias = "parentFolderId")]
    pub(crate) parent_folder_id: Option<i64>,
    #[serde(default, alias = "parent_name", alias = "parentFolderName")]
    pub(crate) parent_folder_name: Option<String>,
    pub(crate) position: Option<i64>,
    pub(crate) created_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
/// Request payload for creating a folder.
pub(crate) struct CreateFolder {
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parent_folder_id: Option<i64>,
}

// ── Member ──

#[derive(Debug, Serialize, Deserialize)]
/// List member or pending invite returned by the API.
pub(crate) struct Member {
    pub(crate) user_id: Option<i64>,
    pub(crate) display_name: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) role: Option<String>,
    #[serde(rename = "type")]
    pub(crate) member_type: Option<String>,
}

// ── Profile ──

#[derive(Debug, Serialize, Deserialize)]
/// Current account profile returned by the API.
pub(crate) struct Profile {
    pub(crate) id: Option<i64>,
    pub(crate) display_name: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) photo_url: Option<String>,
    #[serde(default, alias = "language", alias = "locale")]
    pub(crate) lang: Option<String>,
    pub(crate) is_anonymous: Option<bool>,
    pub(crate) created_at: Option<String>,
    #[serde(default)]
    pub(crate) legal: Option<ProfileLegalStatus>,
    #[serde(default)]
    pub(crate) terms_accepted: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Legal document acceptance state for the profile.
pub(crate) struct ProfileLegalStatus {
    #[serde(default)]
    pub(crate) pending: Vec<ProfilePendingLegalDoc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Pending legal document key for the profile.
pub(crate) struct ProfilePendingLegalDoc {
    pub(crate) key: Option<String>,
}

// ── Search ──

#[derive(Debug, Serialize, Deserialize, Default)]
/// Grouped search results.
pub(crate) struct SearchResults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lists: Option<Vec<SearchListHit>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) items: Option<Vec<SearchItemHit>>,
}

#[derive(Debug, Serialize, Deserialize)]
/// Flat search result hit used by legacy API responses.
pub(crate) struct SearchHit {
    #[serde(rename = "type")]
    pub(crate) hit_type: Option<String>,
    pub(crate) id: i64,
    pub(crate) name: Option<String>,
    pub(crate) icon: Option<String>,
    pub(crate) list_id: Option<i64>,
    pub(crate) list_name: Option<String>,
    pub(crate) list_icon: Option<String>,
    pub(crate) text: Option<String>,
    pub(crate) is_done: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
/// Search response in either grouped or flat API shape.
pub(crate) enum SearchResponse {
    Grouped(SearchResults),
    Flat(Vec<SearchHit>),
}

impl SearchResponse {
    /// Parse a flexible search response from raw JSON.
    pub(crate) fn from_value(value: serde_json::Value) -> Result<Self, String> {
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
                if other.is_boolean() {
                    "bool"
                } else if other.is_number() {
                    "number"
                } else {
                    "string"
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

    /// Convert grouped or flat search responses into grouped results.
    pub(crate) fn into_grouped(self) -> SearchResults {
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
    fn parse_search_response_covers_object_null_and_error_shapes() {
        let null = SearchResponse::from_value(serde_json::Value::Null)
            .expect("null should parse as empty flat response")
            .into_grouped();
        assert!(null.lists.is_none());
        assert!(null.items.is_none());

        let empty_object = SearchResponse::from_value(serde_json::json!({}))
            .expect("empty object should parse as empty grouped response")
            .into_grouped();
        assert!(empty_object.lists.is_none());
        assert!(empty_object.items.is_none());

        let nested = SearchResponse::from_value(serde_json::json!({
            "data": {"lists": [{"id": 8, "name": "Nested"}]}
        }))
        .expect("nested grouped response should parse")
        .into_grouped();
        assert_eq!(nested.lists.expect("nested list")[0].name, "Nested");

        let flat_object = SearchResponse::from_value(serde_json::json!({
            "type": "list",
            "id": 9,
            "name": "Flat object"
        }))
        .expect("single flat object should parse")
        .into_grouped();
        assert_eq!(flat_object.lists.expect("flat list")[0].name, "Flat object");

        let object_hit_without_type = SearchResponse::from_value(serde_json::json!({
            "id": 10,
            "unexpected": true
        }))
        .expect("object hit without type should still parse")
        .into_grouped();
        assert!(object_hit_without_type.lists.is_none());
        assert!(object_hit_without_type.items.is_none());

        assert!(SearchResponse::from_value(serde_json::json!({"unexpected": true})).is_err());
        assert!(SearchResponse::from_value(serde_json::json!(true)).is_err());
        assert!(SearchResponse::from_value(serde_json::json!(7)).is_err());
        assert!(SearchResponse::from_value(serde_json::json!("search")).is_err());
        assert!(SearchResponse::from_value(serde_json::json!([null])).is_err());
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
    fn parse_search_response_covers_fallback_names_and_unknown_hits() {
        let value = serde_json::json!([
            {"type": "list", "id": 10},
            {"type": "item", "id": 11},
            {"type": "ignored", "id": 12}
        ]);

        let grouped = SearchResponse::from_value(value)
            .expect("flat hits should parse")
            .into_grouped();
        let lists = grouped.lists.expect("list fallback expected");
        let items = grouped.items.expect("item fallback expected");
        assert_eq!(lists.len(), 1);
        assert!(lists[0].name.contains("10"));
        assert_eq!(items.len(), 1);
        assert!(items[0].text.contains("11"));

        assert!(SearchResponse::from_value(serde_json::json!([{"id": true}])).is_err());
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

        assert!(matches!(
            key.scopes,
            Some(ApiKeyScopes::Multiple(values)) if values == vec!["all", "lists:read"]
        ));
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

        assert!(matches!(key.scopes, Some(ApiKeyScopes::Single(value)) if value == "all"));
    }
}

#[derive(Debug, Serialize, Deserialize)]
/// Search hit representing a list.
pub(crate) struct SearchListHit {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) icon: Option<String>,
    pub(crate) color: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
/// Search hit representing an item.
pub(crate) struct SearchItemHit {
    pub(crate) id: i64,
    pub(crate) text: String,
    pub(crate) list_id: Option<i64>,
    pub(crate) list_name: Option<String>,
    pub(crate) is_done: Option<bool>,
}

// ── Activity ──

#[derive(Debug, Serialize, Deserialize)]
/// Activity feed entry returned by the API.
pub(crate) struct ActivityEntry {
    pub(crate) id: Option<i64>,
    pub(crate) list_id: Option<i64>,
    pub(crate) user_id: Option<i64>,
    pub(crate) action: Option<String>,
    pub(crate) detail: Option<serde_json::Value>,
    pub(crate) display_name: Option<String>,
    #[serde(alias = "user_name")]
    pub(crate) user_name: Option<String>,
    pub(crate) nickname: Option<String>,
    pub(crate) photo_url: Option<String>,
    pub(crate) item_id: Option<i64>,
    pub(crate) created_at: Option<String>,
}

// ── Generic OK ──

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize)]
/// Generic success response returned by mutation endpoints.
pub(crate) struct OkResponse {
    pub(crate) ok: Option<bool>,
    pub(crate) undo_token: Option<String>,
}

// ── API Key ──

#[derive(Debug, Serialize, Deserialize)]
/// API key metadata returned by the account API.
pub(crate) struct ApiKey {
    pub(crate) id: i64,
    pub(crate) name: Option<String>,
    pub(crate) scopes: Option<ApiKeyScopes>,
    pub(crate) is_active: Option<bool>,
    pub(crate) last_used_at: Option<String>,
    pub(crate) usage_count: Option<i64>,
    pub(crate) created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
/// API key scopes returned as either a string or an array.
pub(crate) enum ApiKeyScopes {
    Single(String),
    Multiple(Vec<String>),
}
