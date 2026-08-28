use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const MAX_ATTACHMENTS: usize = 8;
pub const MAX_ATTACHMENT_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_TOTAL_ATTACHMENT_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentKind {
    Text,
    Image,
}

#[derive(Debug, Clone)]
pub struct TaskAttachment {
    pub id: usize,
    pub display_name: String,
    pub source: PathBuf,
    pub media_type: String,
    pub kind: AttachmentKind,
    pub bytes: Vec<u8>,
    pub content_identity: u64,
}

impl TaskAttachment {
    pub fn snapshot(path: impl AsRef<Path>, id: usize) -> Result<Self, String> {
        let path = path.as_ref();
        let metadata =
            fs::symlink_metadata(path).map_err(|e| format!("cannot attach file: {e}"))?;
        if !metadata.file_type().is_file() {
            return Err(
                "attachments must be regular files (not directories or special files)".into(),
            );
        }
        if metadata.len() > MAX_ATTACHMENT_BYTES {
            return Err(format!(
                "attachment exceeds the {} MiB limit",
                MAX_ATTACHMENT_BYTES / (1024 * 1024)
            ));
        }
        let capacity = usize::try_from(metadata.len())
            .map_err(|_| "attachment is too large for this platform".to_string())?;
        let mut bytes = Vec::with_capacity(capacity);
        File::open(path)
            .map_err(|e| format!("cannot open attachment: {e}"))?
            .take(MAX_ATTACHMENT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("cannot read attachment: {e}"))?;
        if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
            return Err("attachment changed while it was being read".into());
        }
        let (kind, media_type) = detect_type(&bytes, path)?;
        let content_identity = bytes
            .iter()
            .fold(1_469_598_103_934_665_603_u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(1_099_511_628_211)
            });
        Ok(Self {
            id,
            display_name: path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("attachment")
                .to_string(),
            source: path.to_path_buf(),
            media_type,
            kind,
            bytes,
            content_identity,
        })
    }

    pub fn summary(&self) -> String {
        format!(
            "{} · {} · {} KiB",
            self.display_name,
            self.media_type,
            self.bytes.len().div_ceil(1024)
        )
    }
}

fn detect_type(bytes: &[u8], path: &Path) -> Result<(AttachmentKind, String), String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok((AttachmentKind::Image, "image/png".into()));
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok((AttachmentKind::Image, "image/jpeg".into()));
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Ok((AttachmentKind::Image, "image/webp".into()));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Ok((AttachmentKind::Image, "image/gif".into()));
    }
    if bytes.contains(&0) {
        return Err("unsupported binary attachment type".into());
    }
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let media_type = match extension {
        "rs" => "text/x-rust",
        "json" => "application/json",
        "md" => "text/markdown",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",
        _ => "text/plain",
    };
    std::str::from_utf8(bytes).map_err(|_| "attachment is not valid UTF-8 text".to_string())?;
    Ok((AttachmentKind::Text, media_type.into()))
}

pub fn parse_attach_path(raw: &str) -> Result<PathBuf, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("usage: /attach <file>".into());
    }
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value);
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() || value.contains('\0') {
        return Err("invalid attachment path".into());
    }
    Ok(path)
}

pub fn attachment_text(attachment: &TaskAttachment) -> Option<String> {
    (attachment.kind == AttachmentKind::Text)
        .then(|| String::from_utf8_lossy(&attachment.bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::{AttachmentKind, TaskAttachment};
    use std::fs;

    #[test]
    fn snapshots_regular_text_files_without_following_symlinks() {
        let root = std::env::temp_dir().join(format!("claw-attach-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let file = root.join("notes.txt");
        fs::write(&file, "hello").unwrap();
        let attachment = TaskAttachment::snapshot(&file, 1).unwrap();
        assert_eq!(attachment.kind, AttachmentKind::Text);
        assert_eq!(attachment.bytes, b"hello");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_directories_and_binary_blobs() {
        let root = std::env::temp_dir().join(format!("claw-attach-reject-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        assert!(TaskAttachment::snapshot(&root, 1).is_err());
        let blob = root.join("blob.bin");
        fs::write(&blob, [0, 1, 2]).unwrap();
        assert!(TaskAttachment::snapshot(&blob, 1).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
