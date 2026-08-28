use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const MAX_ATTACHMENTS: usize = 8;
pub const MAX_ATTACHMENT_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_TOTAL_ATTACHMENT_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_IMAGE_WIDTH: u32 = 10_000;
pub const MAX_IMAGE_HEIGHT: u32 = 10_000;
pub const MAX_IMAGE_PIXELS: u64 = 40_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentKind {
    Text,
    Image,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageDimensions {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct TaskAttachment {
    pub id: usize,
    pub display_name: String,
    pub source: PathBuf,
    pub media_type: String,
    pub kind: AttachmentKind,
    pub image_dimensions: Option<ImageDimensions>,
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
        let file = open_snapshot_file(path)
            .map_err(|e| format!("cannot open attachment: {e}"))?
            .take(MAX_ATTACHMENT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("cannot read attachment: {e}"))?;
        if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
            return Err("attachment changed while it was being read".into());
        }
        let (kind, media_type, image_dimensions) = detect_type(&bytes, path)?;
        if let Some(dimensions) = image_dimensions {
            if dimensions.width > MAX_IMAGE_WIDTH
                || dimensions.height > MAX_IMAGE_HEIGHT
                || u64::from(dimensions.width) * u64::from(dimensions.height) > MAX_IMAGE_PIXELS
            {
                return Err("image dimensions exceed the safe attachment limit".into());
            }
        }
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
            image_dimensions,
            bytes,
            content_identity,
        })
    }

    pub fn summary(&self) -> String {
        let dimensions = self.image_dimensions.map_or(String::new(), |value| {
            format!(" · {}x{}", value.width, value.height)
        });
        format!(
            "{} · {}{} · {} KiB",
            self.display_name,
            self.media_type,
            dimensions,
            self.bytes.len().div_ceil(1024)
        )
    }
}

fn open_snapshot_file(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
    }
    #[cfg(not(unix))]
    {
        OpenOptions::new().read(true).open(path)
    }
}

fn detect_type(
    bytes: &[u8],
    path: &Path,
) -> Result<(AttachmentKind, String, Option<ImageDimensions>), String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
            return Err("malformed PNG attachment".into());
        }
        let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        if width == 0 || height == 0 {
            return Err("PNG has invalid dimensions".into());
        }
        return Ok((
            AttachmentKind::Image,
            "image/png".into(),
            Some(ImageDimensions { width, height }),
        ));
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        let dimensions = jpeg_dimensions(bytes).ok_or("malformed JPEG attachment")?;
        return Ok((AttachmentKind::Image, "image/jpeg".into(), Some(dimensions)));
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        if bytes.len() < 30 || &bytes[12..16] != b"VP8X" {
            return Err("malformed or unsupported WebP attachment".into());
        }
        let width =
            (1 + u32::from(bytes[24])) | (u32::from(bytes[25]) << 8) | (u32::from(bytes[26]) << 16);
        let height =
            (1 + u32::from(bytes[27])) | (u32::from(bytes[28]) << 8) | (u32::from(bytes[29]) << 16);
        return Ok((
            AttachmentKind::Image,
            "image/webp".into(),
            Some(ImageDimensions { width, height }),
        ));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Err("GIF attachments are not supported yet".into());
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
    Ok((AttachmentKind::Text, media_type.into(), None))
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<ImageDimensions> {
    let mut index = 2;
    while index + 9 < bytes.len() {
        if bytes[index] != 0xff {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        index += 2;
        if marker == 0xd8 || marker == 0xd9 || marker == 0x01 {
            continue;
        }
        let length = usize::from(u16::from_be_bytes([
            *bytes.get(index)?,
            *bytes.get(index + 1)?,
        ]));
        if length < 2 || index + length > bytes.len() {
            return None;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let height = u32::from(u16::from_be_bytes([bytes[index + 3], bytes[index + 4]]));
            let width = u32::from(u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]));
            return (width > 0 && height > 0).then_some(ImageDimensions { width, height });
        }
        index += length;
    }
    None
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

    #[test]
    fn validates_supported_image_dimensions() {
        let root = std::env::temp_dir().join(format!("claw-attach-image-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let image = root.join("valid.png");
        let mut bytes = vec![
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0,
            0, 0, 2, 0, 0, 0, 3, 8, 2, 0, 0, 0,
        ];
        bytes.extend_from_slice(&[0; 4]);
        fs::write(&image, bytes).unwrap();
        let attachment = TaskAttachment::snapshot(&image, 1).unwrap();
        assert_eq!(attachment.kind, AttachmentKind::Image);
        assert_eq!(attachment.image_dimensions.unwrap().width, 2);
        assert_eq!(attachment.image_dimensions.unwrap().height, 3);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_malformed_and_oversized_images() {
        let root =
            std::env::temp_dir().join(format!("claw-attach-image-reject-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let malformed = root.join("malformed.png");
        fs::write(&malformed, b"\x89PNG\r\n\x1a\n").unwrap();
        assert!(TaskAttachment::snapshot(&malformed, 1).is_err());
        let oversized = root.join("oversized.png");
        let mut bytes = vec![
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
            0xff, 0xff, 0xff, 0xff, 0, 0, 0, 1, 8, 2, 0, 0, 0,
        ];
        bytes.extend_from_slice(&[0; 4]);
        fs::write(&oversized, bytes).unwrap();
        assert!(TaskAttachment::snapshot(&oversized, 1).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn enforces_size_limit_and_distinguishes_snapshots() {
        let root = std::env::temp_dir().join(format!("claw-attach-bounds-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let file = root.join("snapshot.txt");
        fs::write(&file, "VERSION_A").unwrap();
        let first = TaskAttachment::snapshot(&file, 1).unwrap();
        fs::write(&file, "VERSION_B").unwrap();
        let second = TaskAttachment::snapshot(&file, 2).unwrap();
        assert_ne!(first.content_identity, second.content_identity);
        assert_eq!(first.bytes, b"VERSION_A");
        let oversized = root.join("oversized.txt");
        let oversized_len = usize::try_from(super::MAX_ATTACHMENT_BYTES).unwrap() + 1;
        fs::write(&oversized, vec![b'x'; oversized_len]).unwrap();
        assert!(TaskAttachment::snapshot(&oversized, 3).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_even_when_target_is_regular() {
        let root = std::env::temp_dir().join(format!("claw-attach-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let target = root.join("target.txt");
        let link = root.join("link.txt");
        fs::write(&target, "target").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(TaskAttachment::snapshot(&link, 1).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
