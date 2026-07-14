use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

use crate::api::ApiClient;
use crate::i18n::tr_args;
use crate::models::Attachment;

const MCP_UPLOADS_ENV: &str = "KRAMLI_MCP_ALLOW_FILE_UPLOADS";
const MCP_FILE_ROOTS_ENV: &str = "KRAMLI_MCP_FILE_ROOTS";
static MCP_STARTUP_CWD: OnceLock<PathBuf> = OnceLock::new();

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
        let _ = MCP_STARTUP_CWD.set(cwd);
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
    if let Some(startup_cwd) = MCP_STARTUP_CWD.get() {
        roots.push(startup_cwd.clone());
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
    use super::validate_image_path;
    use std::fs;

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
}
