use uuid::Uuid;

pub(crate) fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{} GiB", bytes / 1024 / 1024 / 1024)
    } else if bytes >= 1024 * 1024 {
        format!("{} MiB", bytes / 1024 / 1024)
    } else if bytes >= 1024 {
        format!("{} KiB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

pub(crate) fn sanitize_filename(name: &str) -> String {
    use std::path::Path;
    let fallback = "upload.log";
    let file_name = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(fallback);
    let sanitized: String = file_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        fallback.into()
    } else {
        sanitized
    }
}

pub(crate) fn unique_storage_name(original_name: &str) -> String {
    use std::path::Path;

    let suffix = Path::new(original_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(sanitize_extension)
        .filter(|value| !value.is_empty())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    format!("{}{}", Uuid::new_v4().simple(), suffix)
}

fn sanitize_extension(extension: &str) -> String {
    extension
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(16)
        .collect()
}
