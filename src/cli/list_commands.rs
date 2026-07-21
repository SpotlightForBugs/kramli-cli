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
