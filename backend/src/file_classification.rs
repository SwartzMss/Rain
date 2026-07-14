use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

use crate::error::AppError;

const PROBE_BYTES: u64 = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewKind {
    Directory,
    Text,
    Binary,
    Archive,
}

impl PreviewKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Directory => "directory",
            Self::Text => "text",
            Self::Binary => "binary",
            Self::Archive => "archive",
        }
    }
}

pub async fn classify_file(
    path: &Path,
    name: &str,
    mime_type: Option<&str>,
) -> Result<PreviewKind, AppError> {
    if let Some(kind) = classify_declared(name, mime_type) {
        return Ok(kind);
    }

    let file = tokio::fs::File::open(path).await.map_err(AppError::Io)?;
    let mut sample = Vec::new();
    file.take(PROBE_BYTES)
        .read_to_end(&mut sample)
        .await
        .map_err(AppError::Io)?;
    Ok(if is_probably_text(&sample) {
        PreviewKind::Text
    } else {
        PreviewKind::Binary
    })
}

pub fn preview_kind_from_metadata(
    name: &str,
    mime_type: Option<&str>,
    is_dir: bool,
    line_count: Option<i64>,
    meta: Option<&str>,
) -> PreviewKind {
    if is_dir {
        return PreviewKind::Directory;
    }
    if let Some(kind) = meta
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
        .and_then(|value| value.get("preview_kind").cloned())
        .and_then(|value| serde_json::from_value(value).ok())
    {
        return kind;
    }
    if line_count.is_some() {
        return PreviewKind::Text;
    }
    classify_declared(name, mime_type).unwrap_or(PreviewKind::Binary)
}

pub fn effective_mime_type(name: &str, supplied: Option<&str>) -> Option<String> {
    let supplied = supplied.map(str::trim).filter(|value| !value.is_empty());
    if let Some(value) = supplied
        && !value.eq_ignore_ascii_case("application/octet-stream")
    {
        return Some(value.to_string());
    }
    mime_guess::from_path(name)
        .first_raw()
        .map(str::to_string)
        .or_else(|| supplied.map(str::to_string))
}

pub fn is_supported_archive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".zip")
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
        || (lower.ends_with(".gz") && !lower.ends_with(".tar.gz"))
}

fn classify_declared(name: &str, mime_type: Option<&str>) -> Option<PreviewKind> {
    if is_supported_archive_name(name) {
        return Some(PreviewKind::Archive);
    }

    let extension = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    if extension.as_deref().is_some_and(is_binary_extension) {
        return Some(PreviewKind::Binary);
    }
    if extension.as_deref().is_some_and(is_text_extension) {
        return Some(PreviewKind::Text);
    }

    let mime = mime_type.map(str::to_ascii_lowercase)?;
    if mime.starts_with("text/")
        || matches!(
            mime.as_str(),
            "application/json"
                | "application/ld+json"
                | "application/xml"
                | "application/x-yaml"
                | "application/toml"
                | "application/javascript"
        )
    {
        return Some(PreviewKind::Text);
    }
    if mime == "application/octet-stream" {
        return None;
    }
    if mime.starts_with("image/")
        || mime.starts_with("audio/")
        || mime.starts_with("video/")
        || mime.starts_with("font/")
        || mime.starts_with("application/")
    {
        return Some(PreviewKind::Binary);
    }
    None
}

fn is_text_extension(extension: &str) -> bool {
    matches!(
        extension,
        "log"
            | "txt"
            | "toml"
            | "rs"
            | "json"
            | "yaml"
            | "yml"
            | "md"
            | "cfg"
            | "conf"
            | "ini"
            | "env"
            | "csv"
            | "xml"
            | "html"
            | "htm"
            | "css"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "py"
            | "java"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "sh"
            | "bat"
            | "ps1"
            | "properties"
    )
}

fn is_binary_extension(extension: &str) -> bool {
    matches!(
        extension,
        "exe"
            | "dll"
            | "so"
            | "dylib"
            | "bin"
            | "class"
            | "jar"
            | "doc"
            | "docx"
            | "xls"
            | "xlsx"
            | "ppt"
            | "pptx"
            | "pdf"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "bmp"
            | "ico"
            | "mp3"
            | "wav"
            | "mp4"
            | "mov"
            | "avi"
            | "woff"
            | "woff2"
            | "ttf"
            | "otf"
            | "rar"
            | "7z"
    )
}

fn is_probably_text(sample: &[u8]) -> bool {
    if sample.is_empty() {
        return true;
    }
    if sample.contains(&0) {
        return false;
    }
    let text = match std::str::from_utf8(sample) {
        Ok(text) => text,
        Err(error) if error.error_len().is_none() && error.valid_up_to() > 0 => {
            std::str::from_utf8(&sample[..error.valid_up_to()])
                .expect("UTF-8 valid prefix reported by parser")
        }
        Err(_) => return false,
    };
    let mut total = 0usize;
    let mut controls = 0usize;
    for ch in text.chars() {
        total += 1;
        if ch.is_control() && !matches!(ch, '\n' | '\r' | '\t' | '\u{0008}' | '\u{000c}') {
            controls += 1;
        }
    }
    controls * 100 <= total.max(1) * 2
}

#[cfg(test)]
mod tests {
    use super::{PreviewKind, classify_file, preview_kind_from_metadata};

    #[tokio::test]
    async fn probes_unknown_extension_as_text_or_binary() {
        let root = std::env::temp_dir().join(format!("rain-classify-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root)
            .await
            .expect("create test root");
        let text_path = root.join("notes.data");
        let binary_path = root.join("blob.data");
        tokio::fs::write(&text_path, "INFO unknown extension text\n")
            .await
            .expect("write text sample");
        tokio::fs::write(&binary_path, [0, 159, 146, 150])
            .await
            .expect("write binary sample");

        assert_eq!(
            classify_file(&text_path, "notes.data", Some("application/octet-stream"))
                .await
                .expect("classify text"),
            PreviewKind::Text
        );
        assert_eq!(
            classify_file(&binary_path, "blob.data", Some("application/octet-stream"))
                .await
                .expect("classify binary"),
            PreviewKind::Binary
        );
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn office_zip_containers_are_binary_not_archives() {
        for name in ["report.docx", "sheet.xlsx", "slides.pptx"] {
            assert_eq!(
                preview_kind_from_metadata(name, Some("application/zip"), false, None, None),
                PreviewKind::Binary,
                "{name}"
            );
        }
        assert_eq!(
            preview_kind_from_metadata("logs.zip", Some("application/zip"), false, None, None),
            PreviewKind::Archive
        );
    }
}
