use super::*;

pub(super) async fn run_lists(cmd: ListCmd, as_json: bool) -> Result<(), String> {
    let api = get_api()?;
    match cmd {
        ListCmd::List => {
            let lists: Vec<ShoppingList> = api.get("/lists").await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&lists).unwrap_or_default()
                );
            } else {
                output::print_lists(&lists);
            }
        }
        ListCmd::Resolve { reference } => {
            let id = resolve_list_reference(&reference)?;
            let payload = json!({
                "reference": reference,
                "list_id": id,
                "canonical_path": format!("/lists/{id}"),
            });
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).unwrap_or_default()
                );
            } else {
                println!("{} {}", "✓".green(), tr("cli-list-reference-resolved"));
                println!("  {}: {id}", tr("label-list-id"));
                println!("  {}: /lists/{id}", tr("label-canonical"));
            }
        }
        ListCmd::Show { id } => {
            run_lists_show(&api, id, as_json).await?;
        }
        ListCmd::Create {
            name,
            icon,
            color,
            folder,
            list_type,
            note_content,
            states,
        } => {
            run_lists_create(
                &api,
                as_json,
                CreateListArgs {
                    name,
                    icon,
                    color,
                    folder,
                    list_type,
                    note_content,
                    states,
                },
            )
            .await?
        }
        ListCmd::Update {
            id,
            name,
            icon,
            color,
            note_content,
            states,
        } => {
            run_lists_update(
                &api,
                as_json,
                UpdateListArgs {
                    id,
                    name,
                    icon,
                    color,
                    note_content,
                    states,
                },
            )
            .await?
        }
        ListCmd::Delete { id } => run_lists_delete(&api, as_json, id).await?,
        ListCmd::Move { id, folder_id } => run_lists_move(&api, as_json, id, folder_id).await?,
    }
    Ok(())
}

pub(super) async fn run_lists_show(api: &ApiClient, id: i64, as_json: bool) -> Result<(), String> {
    let payload: Value = api.get(&format!("/lists/{id}")).await?;
    let list = list_from_payload(payload.clone())?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
    } else {
        output::print_list_detail(&list);
        if is_note_list_type(list_type_value(&payload)) {
            output::print_note_content(list_note_content(&payload));
        }
        let mut texts = vec![list.name.as_str()];
        if let Some(content) = list_note_content(&payload) {
            texts.push(content);
        }
        print_link_previews_for_texts(api, texts).await;
    }
    maybe_auto_handoff(api, id, Some(&list.name), as_json).await;
    Ok(())
}

pub(super) async fn run_lists_create(
    api: &ApiClient,
    as_json: bool,
    args: CreateListArgs,
) -> Result<(), String> {
    let CreateListArgs {
        name,
        icon,
        color,
        folder,
        list_type,
        note_content,
        states,
    } = args;
    let payload =
        build_list_create_payload(name, icon, color, folder, list_type, note_content, states)?;
    let payload: Value = api.post("/lists", &payload).await?;
    let list = list_from_payload(payload.clone())?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
    } else {
        println!(
            "{} {}",
            "✓".green(),
            tr_args("cli-list-created", &[("id", list.id.to_string())])
        );
        output::print_list_detail(&list);
        if is_note_list_type(list_type_value(&payload)) {
            output::print_note_content(list_note_content(&payload));
        }
    }
    Ok(())
}

pub(super) async fn run_lists_update(
    api: &ApiClient,
    as_json: bool,
    args: UpdateListArgs,
) -> Result<(), String> {
    let UpdateListArgs {
        id,
        name,
        icon,
        color,
        note_content,
        states,
    } = args;
    let mut body = update_list_body(name, icon, color, note_content.clone(), states)?;
    body.remove("note_content");
    if let Some(note_content) = note_content {
        let current: Value = api.get(&format!("/lists/{id}")).await?;
        apply_safe_note_update(&mut body, &current, &note_content)?;
    }
    let payload: Value = api.put(&format!("/lists/{id}"), &body).await?;
    let list = list_from_payload(payload.clone())?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
    } else {
        println!("{} {}", "✓".green(), tr("cli-list-updated"));
        output::print_list_detail(&list);
        if is_note_list_type(list_type_value(&payload)) {
            output::print_note_content(list_note_content(&payload));
        }
    }
    Ok(())
}

pub(super) async fn run_lists_delete(
    api: &ApiClient,
    as_json: bool,
    id: i64,
) -> Result<(), String> {
    let resp: OkResponse = api.delete(&format!("/lists/{id}")).await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&resp).unwrap_or_default()
        );
    } else {
        println!("{} {}", "✓".green(), tr("cli-list-deleted"));
        if let Some(t) = resp.undo_token {
            println!("  {}: {t}", tr("label-undo-token"));
        }
    }
    Ok(())
}

pub(super) async fn run_lists_move(
    api: &ApiClient,
    as_json: bool,
    id: i64,
    folder_id: Option<i64>,
) -> Result<(), String> {
    let body = json!({"folder_id": folder_id});
    let list: ShoppingList = api.put(&format!("/lists/{id}"), &body).await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&list).unwrap_or_default()
        );
    } else {
        print_list_move_result(id, folder_id);
    }
    Ok(())
}

fn print_list_move_result(id: i64, folder_id: Option<i64>) {
    match folder_id {
        Some(fid) => println!(
            "{} {}",
            "✓".green(),
            tr_args(
                "cli-list-moved-folder",
                &[("id", id.to_string()), ("folder_id", fid.to_string())],
            )
        ),
        None => println!(
            "{} {}",
            "✓".green(),
            tr_args("cli-list-removed-folder", &[("id", id.to_string())])
        ),
    }
}

pub(super) struct CreateListArgs {
    pub(super) name: String,
    pub(super) icon: Option<String>,
    pub(super) color: Option<String>,
    pub(super) folder: Option<i64>,
    pub(super) list_type: Option<String>,
    pub(super) note_content: Option<String>,
    pub(super) states: Option<String>,
}

pub(super) struct UpdateListArgs {
    pub(super) id: i64,
    pub(super) name: Option<String>,
    pub(super) icon: Option<String>,
    pub(super) color: Option<String>,
    pub(super) note_content: Option<String>,
    pub(super) states: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ApiClient;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn api_with_responses(
        responses: Vec<String>,
    ) -> (ApiClient, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("test server should have addr");
        let base_url = format!("http://{addr}");
        let ready = (!responses.is_empty())
            .then(|| crate::test_env::register_mock_server(base_url.clone()));
        let handle = tokio::spawn(async move {
            if let Some(ready) = ready {
                let _ = ready.await;
            }
            let mut requests = Vec::new();
            for body in responses {
                let (mut stream, _) =
                    tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                        .await
                        .expect("test server accept timed out")
                        .expect("request should connect");
                let mut buffer = [0_u8; 4096];
                let read = stream.read(&mut buffer).await.expect("request should read");
                requests.push(String::from_utf8_lossy(&buffer[..read]).to_string());
                let header = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                stream
                    .write_all(header.as_bytes())
                    .await
                    .expect("response header should write");
                stream
                    .write_all(body.as_bytes())
                    .await
                    .expect("response body should write");
            }
            requests
        });

        (ApiClient::for_tests(&base_url), handle)
    }

    #[tokio::test]
    async fn list_commands_cover_human_and_json_output_branches() {
        let responses = vec![
            json!([{"id": 7, "name": "Groceries"}]).to_string(),
            json!({"id": 7, "name": "Groceries", "list_type": "tasks"}).to_string(),
            json!({"id": 8, "name": "Notes", "list_type": "note", "note_content": "Seed"})
                .to_string(),
            json!({"id": 7, "name": "Renamed", "list_type": "note", "note_content": "Body"})
                .to_string(),
            json!({"id": 7, "name": "Renamed", "list_type": "note", "note_content": "Updated"})
                .to_string(),
            json!({"ok": true, "undo_token": "undo-list"}).to_string(),
            json!({"id": 7, "name": "Renamed", "folder_id": 3}).to_string(),
            json!({"id": 7, "name": "Renamed", "folder_id": null}).to_string(),
        ];
        let (api, requests) = api_with_responses(responses).await;
        let base_url = api.base_url_for_tests().to_string();

        crate::test_env::with_env_vars_async(
            &[
                ("KRAMLI_URL", base_url.as_str()),
                ("KRAMLI_API_KEY", "kramli_test"),
            ],
            || async {
                run_lists(ListCmd::List, false)
                    .await
                    .expect("human list output should succeed");
                run_lists(
                    ListCmd::Resolve {
                        reference: "7".to_string(),
                    },
                    true,
                )
                .await
                .expect("json resolve output should succeed");
                run_lists_show(&api, 7, false)
                    .await
                    .expect("human show output should succeed");
                run_lists_create(
                    &api,
                    true,
                    CreateListArgs {
                        name: "Notes".to_string(),
                        icon: None,
                        color: None,
                        folder: None,
                        list_type: Some("note".to_string()),
                        note_content: Some("Seed".to_string()),
                        states: None,
                    },
                )
                .await
                .expect("json create should succeed");
        run_lists_update(
            &api,
            false,
            UpdateListArgs {
                id: 7,
                name: Some("Renamed".to_string()),
                icon: None,
                color: None,
                note_content: None,
                states: None,
            },
        )
        .await
        .expect("human update without note content should succeed");
                run_lists_delete(&api, false, 7)
                    .await
                    .expect("human delete should succeed");
                run_lists_move(&api, false, 7, Some(3))
                    .await
                    .expect("human move into folder should succeed");
                run_lists_move(&api, true, 7, None)
                    .await
                    .expect("json move out of folder should succeed");
            },
        )
        .await;

        let requests = requests.await.expect("test server should finish");
        assert_eq!(requests.len(), 8);
        assert!(requests[0].starts_with("GET /api/lists HTTP/1.1"));
        assert!(requests[4].starts_with("PUT /api/lists/7 HTTP/1.1"));
    }

    #[tokio::test]
    async fn list_update_with_note_content_uses_safe_delta_contract() {
        let current = json!({
            "id": 7,
            "name": "Notes",
            "list_type": "note",
            "note_content": "Old",
            "note_delta": "[{\"insert\":\"Old\\n\"}]",
            "note_version": 4
        });
        let updated = json!({
            "id": 7,
            "name": "Notes",
            "list_type": "note",
            "note_content": "New",
            "note_delta": "[{\"insert\":\"New\\n\"}]",
            "note_version": 5
        });
        let (api, requests) = api_with_responses(vec![current.to_string(), updated.to_string()]).await;

        run_lists_update(
            &api,
            true,
            UpdateListArgs {
                id: 7,
                name: None,
                icon: None,
                color: None,
                note_content: Some("New".to_string()),
                states: None,
            },
        )
        .await
        .expect("note update should fetch current delta before put");

        let requests = requests.await.expect("test server should finish");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /api/lists/7 HTTP/1.1"));
        assert!(requests[1].starts_with("PUT /api/lists/7 HTTP/1.1"));
    }
}
