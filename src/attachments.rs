use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use serde::Deserialize;

use crate::api::ApiClient;
use crate::i18n::tr_args;
use crate::models::Attachment;

const MCP_UPLOADS_ENV: &str = "KRAMLI_MCP_ALLOW_FILE_UPLOADS";
const MCP_FILE_ROOTS_ENV: &str = "KRAMLI_MCP_FILE_ROOTS";
static MCP_STARTUP_CWD: LazyLock<RwLock<Option<PathBuf>>> =
    LazyLock::new(|| RwLock::new(None));

#[derive(Clone, Debug)]
pub(crate) struct AttachmentUpload {
    pub(crate) path: PathBuf,
    pub(crate) sensitive: bool,
    pub(crate) context: Option<String>,
    pub(crate) alt_text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UploadResponse {
    attachment: Attachment,
}

pub(crate) fn validate_image_path(path: &Path) -> Result<ValidatedImage, String> {
    let metadata = fs::metadata(path).map_err(|_| tr_args("attachment-file-not-found", &[]))?;
    if !metadata.is_file() {
        return Err(tr_args("attachment-file-not-file", &[]));
    }
    if metadata.len() == 0 {
        return Err(tr_args("attachment-file-empty", &[]));
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| tr_args("attachment-file-invalid-name", &[]))?;
    let extension = Path::new(file_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| tr_args("attachment-file-unsupported", &[]))?;
    let mime_type = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        _ => return Err(tr_args("attachment-file-unsupported", &[])),
    };
    Ok(ValidatedImage {
        file_name: file_name.to_string(),
        mime_type,
        bytes: fs::read(path).map_err(|_| tr_args("attachment-file-read-error", &[]))?,
    })
}

pub(crate) struct ValidatedImage {
    pub(crate) file_name: String,
    pub(crate) mime_type: &'static str,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) async fn upload_item_attachment(
    api: &ApiClient,
    item_id: i64,
    upload: &AttachmentUpload,
) -> Result<Attachment, String> {
    let image = validate_image_path(&upload.path)?;
    let mut fields = vec![("sensitive".to_string(), upload.sensitive.to_string())];
    if let Some(context) = upload.context.as_deref().filter(|value| !value.is_empty()) {
        fields.push(("context".to_string(), context.to_string()));
    }
    if let Some(alt_text) = upload.alt_text.as_deref().filter(|value| !value.is_empty()) {
        fields.push(("alt_text".to_string(), alt_text.to_string()));
    }
    let response: UploadResponse = api
        .post_multipart(
            &format!("/items/{item_id}/attachments"),
            &image.file_name,
            image.mime_type,
            image.bytes,
            &fields,
        )
        .await?;
    Ok(response.attachment)
}

pub(crate) fn initialize_mcp_file_policy() {
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(mut guard) = MCP_STARTUP_CWD.write() {
            if guard.is_none() {
                *guard = Some(cwd);
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn reset_mcp_file_policy_for_tests() {
    if let Ok(mut guard) = MCP_STARTUP_CWD.write() {
        *guard = None;
    }
}

pub(crate) fn ensure_mcp_upload_allowed(path: &Path) -> Result<(), String> {
    if !mcp_file_uploads_enabled() {
        return Err(tr_args("mcp-file-uploads-disabled", &[]));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| tr_args("attachment-file-not-found", &[]))?;
    let mut roots = Vec::new();
    if let Ok(guard) = MCP_STARTUP_CWD.read() {
        match guard.as_ref() {
            Some(startup_cwd) => roots.push(startup_cwd.clone()),
            None => {
                if let Ok(cwd) = std::env::current_dir() {
                    roots.push(cwd);
                }
            }
        }
    } else if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    if let Ok(configured) = std::env::var(MCP_FILE_ROOTS_ENV) {
        roots.extend(
            configured
                .split(':')
                .filter(|root| !root.trim().is_empty())
                .map(PathBuf::from),
        );
    }
    let allowed = roots
        .into_iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| canonical.starts_with(root));
    if allowed {
        Ok(())
    } else {
        Err(tr_args("mcp-file-path-not-allowed", &[]))
    }
}

pub(crate) fn mcp_file_uploads_enabled() -> bool {
    env_truthy(MCP_UPLOADS_ENV)
}

fn env_truthy(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref().map(str::trim),
        Some("1" | "true" | "on" | "yes")
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::{
        ensure_mcp_upload_allowed, initialize_mcp_file_policy, mcp_file_uploads_enabled,
        reset_mcp_file_policy_for_tests, upload_item_attachment, validate_image_path,
        AttachmentUpload,
    };
    use crate::api::ApiClient;
    use crate::test_env::{register_mock_server, with_env_vars};

    async fn api_with_upload_response(
        body: serde_json::Value,
    ) -> (ApiClient, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server address");
        let base_url = format!("http://{addr}");
        let ready = register_mock_server(base_url.clone());
        let handle = tokio::spawn(async move {
            let _ = ready.await;
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .expect("accept timed out")
                .expect("connection");
            let mut buf = vec![0_u8; 16_384];
            let n = stream.read(&mut buf).await.expect("read request");
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let payload = body.to_string();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(payload.as_bytes()).await.unwrap();
            request
        });
        (ApiClient::for_tests(&base_url), handle)
    }

    #[test]
    fn validates_supported_non_empty_files_and_rejects_unsafe_inputs() {
        let root = std::env::temp_dir().join(format!("kramli-attachment-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let png = root.join("photo.PNG");
        fs::write(&png, [1, 2, 3]).unwrap();
        let validated = validate_image_path(&png).unwrap();
        assert_eq!(validated.mime_type, "image/png");
        assert_eq!(validated.file_name, "photo.PNG");
        assert!(validate_image_path(&root).is_err());
        let empty = root.join("empty.jpg");
        fs::write(&empty, []).unwrap();
        assert!(validate_image_path(&empty).is_err());
        let text = root.join("note.txt");
        fs::write(&text, [1]).unwrap();
        assert!(validate_image_path(&text).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn upload_item_attachment_posts_multipart_with_optional_fields() {
        let root = std::env::temp_dir().join(format!("kramli-upload-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let png = root.join("photo.png");
        fs::write(&png, [137, 80, 78, 71]).unwrap();

        let (api, request) = api_with_upload_response(json!({
            "attachment": {
                "id": 9,
                "filename": "photo.png",
                "content_type": "image/png"
            }
        }))
        .await;

        let attachment = upload_item_attachment(
            &api,
            7,
            &AttachmentUpload {
                path: png.clone(),
                sensitive: true,
                context: Some("receipt".to_string()),
                alt_text: Some("Receipt".to_string()),
            },
        )
        .await
        .expect("upload should succeed");
        assert_eq!(attachment.id, 9);

        let request = request.await.expect("server should finish");
        assert!(request.starts_with("POST /api/items/7/attachments"));
        assert!(request.contains("multipart/form-data"));
        assert!(request.contains("name=\"sensitive\""));
        assert!(request.contains("name=\"context\""));
        assert!(request.contains("name=\"alt_text\""));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn validate_image_path_accepts_common_extensions() {
        let root = std::env::temp_dir().join(format!("kramli-mime-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let cases = [
            ("photo.jpg", "image/jpeg"),
            ("photo.jpeg", "image/jpeg"),
            ("photo.gif", "image/gif"),
            ("photo.webp", "image/webp"),
            ("photo.heic", "image/heic"),
            ("photo.heif", "image/heif"),
        ];
        for (name, mime) in cases {
            let path = root.join(name);
            fs::write(&path, [1, 2, 3]).unwrap();
            let validated = validate_image_path(&path).unwrap();
            assert_eq!(validated.mime_type, mime);
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn initialize_mcp_file_policy_is_idempotent() {
        reset_mcp_file_policy_for_tests();
        let cwd = std::env::current_dir().expect("cwd");
        initialize_mcp_file_policy();
        initialize_mcp_file_policy();
        let root = cwd.join(format!("kramli-mcp-idempotent-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let png = root.join("local.png");
        fs::write(&png, [1, 2, 3]).unwrap();

        with_env_vars(&[("KRAMLI_MCP_ALLOW_FILE_UPLOADS", "1")], || {
            assert!(ensure_mcp_upload_allowed(&png).is_ok());
        });

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_mcp_upload_allowed_uses_current_dir_when_startup_cwd_unset() {
        reset_mcp_file_policy_for_tests();
        let cwd = std::env::current_dir().expect("cwd");
        let root = cwd.join(format!("kramli-mcp-no-init-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let png = root.join("local.png");
        fs::write(&png, [1, 2, 3]).unwrap();

        with_env_vars(&[("KRAMLI_MCP_ALLOW_FILE_UPLOADS", "1")], || {
            assert!(ensure_mcp_upload_allowed(&png).is_ok());
        });

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn mcp_file_policy_allows_startup_paths_and_rejects_outside_roots() {
        let cwd = std::env::current_dir().expect("cwd");
        let root = cwd.join(format!("kramli-mcp-policy-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let png = root.join("allowed.png");
        fs::write(&png, [1, 2, 3]).unwrap();
        let outside = std::env::temp_dir().join(format!("kramli-outside-{}", std::process::id()));
        fs::create_dir_all(&outside).unwrap();
        let allowed = outside.join("scoped.png");
        fs::write(&allowed, [1, 2, 3]).unwrap();
        let forbidden_root =
            std::env::temp_dir().join(format!("kramli-forbidden-{}", std::process::id()));
        fs::create_dir_all(&forbidden_root).unwrap();
        let forbidden = forbidden_root.join("secret.png");
        fs::write(&forbidden, [1, 2, 3]).unwrap();

        with_env_vars(&[("KRAMLI_MCP_ALLOW_FILE_UPLOADS", "1")], || {
            reset_mcp_file_policy_for_tests();
            initialize_mcp_file_policy();
            assert!(mcp_file_uploads_enabled());
            assert!(ensure_mcp_upload_allowed(&png).is_ok());
        });

        with_env_vars(
            &[
                ("KRAMLI_MCP_ALLOW_FILE_UPLOADS", "1"),
                (
                    "KRAMLI_MCP_FILE_ROOTS",
                    outside.to_str().expect("outside path utf-8"),
                ),
            ],
            || {
                assert!(ensure_mcp_upload_allowed(&allowed).is_ok());
                assert!(ensure_mcp_upload_allowed(&forbidden).is_err());
            },
        );

        with_env_vars(&[("KRAMLI_MCP_ALLOW_FILE_UPLOADS", "0")], || {
            assert!(!mcp_file_uploads_enabled());
            assert!(ensure_mcp_upload_allowed(&png).is_err());
        });

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
        let _ = fs::remove_dir_all(forbidden_root);
    }
}
