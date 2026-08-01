use std::{
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use actix_multipart::{Field, Multipart};
use actix_web::{
    dev::Payload as ActixPayload,
    error::PayloadError,
    http::StatusCode,
    web::{self, Bytes},
};
use futures_util::{Stream, TryStreamExt};
use std::task::{Context, Poll};
use tokio::{fs, io::AsyncWriteExt};

use crate::{
    error::AppError,
    ingest::limits::{MAX_MULTIPART_TEXT_FIELD_SIZE, MAX_UPLOAD_FILES},
};

use super::filename::{format_bytes, sanitize_filename, unique_storage_name};

pub const MAX_MULTIPART_FIELDS: usize = 64;
const MAX_MULTIPART_OVERHEAD_BYTES: u64 = 256 * 1024;

pub struct UploadedFile {
    pub original_name: String,
    pub display_name: String,
    pub storage_name: String,
    pub temp_path: PathBuf,
    pub size_bytes: u64,
    pub content_type: Option<String>,
}

pub struct MultipartUpload {
    pub files: Vec<UploadedFile>,
    pub total_bytes: u64,
    pub receive_reservation: ReceiveReservation,
}

pub struct ReceiveReservation {
    budget: TempBudget,
}

#[derive(Clone)]
pub struct TempBudget {
    used: Arc<AtomicU64>,
    reserved: Arc<AtomicU64>,
    max: u64,
}

impl ReceiveReservation {
    fn new(used: Arc<AtomicU64>, max: u64) -> Self {
        Self {
            budget: TempBudget {
                used,
                reserved: Arc::new(AtomicU64::new(0)),
                max,
            },
        }
    }

    fn reserve(&self, bytes: u64) -> Result<(), AppError> {
        self.budget.reserve(bytes)
    }

    pub fn temp_budget(&self) -> TempBudget {
        self.budget.clone()
    }
}

impl TempBudget {
    pub fn reserve(&self, bytes: u64) -> Result<(), AppError> {
        loop {
            let current = self.used.load(Ordering::Acquire);
            let next = current.checked_add(bytes).ok_or_else(tmp_budget_error)?;
            if next > self.max {
                return Err(tmp_budget_error());
            }
            if self
                .used
                .compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.reserved.fetch_add(bytes, Ordering::AcqRel);
                return Ok(());
            }
        }
    }
}

impl Drop for ReceiveReservation {
    fn drop(&mut self) {
        let reserved = self.budget.reserved.swap(0, Ordering::AcqRel);
        self.budget.used.fetch_sub(reserved, Ordering::AcqRel);
    }
}

fn tmp_budget_error() -> AppError {
    AppError::api(
        StatusCode::TOO_MANY_REQUESTS,
        "UPLOAD_TMP_BUDGET_EXCEEDED",
        "上传临时存储配额已用尽，请稍后重试",
    )
}

pub async fn collect_multipart_upload(
    mut payload: Multipart,
    temp_dir: &Path,
    max_total_bytes: u64,
    tmp_bytes: Arc<AtomicU64>,
    max_tmp_bytes: u64,
) -> Result<MultipartUpload, AppError> {
    let mut files: Vec<UploadedFile> = Vec::new();
    let mut total_file_bytes: u64 = 0;
    let mut total_request_bytes: u64 = 0;
    let mut file_fields = 0;
    let mut field_count = 0;
    let mut issue_code_fields = 0;
    let receive_reservation = ReceiveReservation::new(tmp_bytes, max_tmp_bytes);

    while let Some(mut field) = payload
        .try_next()
        .await
        .map_err(|err| AppError::BadRequest(format!("multipart error: {err}")))?
    {
        let content_disposition = field.content_disposition().clone();
        let field_name = content_disposition.get_name().unwrap_or("").to_string();
        field_count += 1;
        if field_count > MAX_MULTIPART_FIELDS {
            return Err(AppError::BadRequest(format!(
                "too many multipart fields; max {} fields per upload",
                MAX_MULTIPART_FIELDS
            )));
        }

        match field_name.as_str() {
            "issue_code" => {
                issue_code_fields += 1;
                if issue_code_fields > 1 {
                    return Err(AppError::BadRequest(
                        "issue_code field may only appear once".into(),
                    ));
                }
                collect_text_field(
                    &mut field,
                    MAX_MULTIPART_TEXT_FIELD_SIZE,
                    &mut total_request_bytes,
                    max_total_bytes,
                )
                .await?;
            }
            "files" => {
                if file_fields >= MAX_UPLOAD_FILES {
                    return Err(AppError::BadRequest(format!(
                        "too many files; max {} files per upload",
                        MAX_UPLOAD_FILES
                    )));
                }
                file_fields += 1;
                let filename = content_disposition
                    .get_filename()
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| "upload.log".into());

                let content_type = field.content_type().map(|mime| mime.to_string());
                let display_name = sanitize_filename(&filename);
                let storage_name = unique_storage_name(&filename);
                let temp_name = format!("{}-{storage_name}", file_fields - 1);
                let temp_path = temp_dir.join(temp_name);
                let remaining_bytes = max_total_bytes
                    .checked_sub(total_request_bytes)
                    .ok_or_else(|| {
                        AppError::BadRequest(format!(
                            "upload exceeds the maximum size of {}",
                            format_bytes(max_total_bytes)
                        ))
                    })?;
                let size_bytes = collect_file_field(
                    &mut field,
                    &temp_path,
                    remaining_bytes,
                    &filename,
                    &receive_reservation,
                    &mut total_request_bytes,
                    max_total_bytes,
                )
                .await?;

                if size_bytes > 0 {
                    total_file_bytes =
                        total_file_bytes.checked_add(size_bytes).ok_or_else(|| {
                            AppError::BadRequest("upload size exceeds the supported limit".into())
                        })?;
                    files.push(UploadedFile {
                        original_name: filename,
                        display_name,
                        storage_name,
                        temp_path,
                        size_bytes,
                        content_type,
                    });
                } else {
                    let _ = fs::remove_file(&temp_path).await;
                }
            }
            _ => {
                return Err(AppError::BadRequest(format!(
                    "unknown multipart field: {field_name}"
                )));
            }
        }
    }

    if files.is_empty() {
        return Err(AppError::BadRequest("no files provided".into()));
    }

    Ok(MultipartUpload {
        files,
        total_bytes: total_file_bytes,
        receive_reservation,
    })
}

pub fn raw_payload_limit(max_total_bytes: u64) -> u64 {
    max_total_bytes.saturating_add(MAX_MULTIPART_OVERHEAD_BYTES)
}

pub fn limited_multipart(
    req: &actix_web::HttpRequest,
    payload: web::Payload,
    limit: u64,
) -> Multipart {
    Multipart::new(
        req.headers(),
        LimitedPayload {
            inner: payload.into_inner(),
            seen: 0,
            limit,
        },
    )
}

struct LimitedPayload {
    inner: ActixPayload,
    seen: u64,
    limit: u64,
}

impl Stream for LimitedPayload {
    type Item = Result<Bytes, PayloadError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                let Some(next) = self.seen.checked_add(chunk.len() as u64) else {
                    return Poll::Ready(Some(Err(PayloadError::Overflow)));
                };
                if next > self.limit {
                    return Poll::Ready(Some(Err(PayloadError::Overflow)));
                }
                self.seen = next;
                Poll::Ready(Some(Ok(chunk)))
            }
            other => other,
        }
    }
}

async fn collect_text_field(
    field: &mut Field,
    limit: u64,
    total_request_bytes: &mut u64,
    max_total_bytes: u64,
) -> Result<String, AppError> {
    let bytes = collect_binary_field(
        field,
        limit,
        "text field",
        total_request_bytes,
        max_total_bytes,
    )
    .await?;
    let value = String::from_utf8(bytes)
        .map_err(|_| AppError::BadRequest("field is not valid UTF-8".into()))?;
    Ok(value.trim().to_string())
}

async fn collect_binary_field(
    field: &mut Field,
    limit: u64,
    label: &str,
    total_request_bytes: &mut u64,
    max_total_bytes: u64,
) -> Result<Vec<u8>, AppError> {
    let mut data = Vec::new();
    while let Some(chunk) = field
        .try_next()
        .await
        .map_err(|err| AppError::BadRequest(format!("failed to read field: {err}")))?
    {
        let next_size = (data.len() as u64)
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| {
                AppError::BadRequest(format!(
                    "{label} is too large; max size is {}",
                    format_bytes(limit)
                ))
            })?;
        if next_size > limit {
            return Err(AppError::BadRequest(format!(
                "{label} is too large; max size is {}",
                format_bytes(limit)
            )));
        }
        reserve_request_bytes(total_request_bytes, chunk.len() as u64, max_total_bytes)?;
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

async fn collect_file_field(
    field: &mut Field,
    path: &Path,
    limit: u64,
    label: &str,
    reservation: &ReceiveReservation,
    total_request_bytes: &mut u64,
    max_total_bytes: u64,
) -> Result<u64, AppError> {
    let mut file = fs::File::create(path).await.map_err(AppError::Io)?;
    let mut written = 0u64;
    while let Some(chunk) = field
        .try_next()
        .await
        .map_err(|err| AppError::BadRequest(format!("failed to read field: {err}")))?
    {
        let next_written = written.checked_add(chunk.len() as u64).ok_or_else(|| {
            AppError::BadRequest(format!(
                "{label} is too large; max size is {}",
                format_bytes(limit)
            ))
        })?;
        if next_written > limit {
            return Err(AppError::BadRequest(format!(
                "{label} is too large; max size is {}",
                format_bytes(limit)
            )));
        }
        reserve_request_bytes(total_request_bytes, chunk.len() as u64, max_total_bytes)?;
        reservation.reserve(chunk.len() as u64)?;
        file.write_all(&chunk).await.map_err(AppError::Io)?;
        written = next_written;
    }
    file.flush().await.map_err(AppError::Io)?;
    Ok(written)
}

fn reserve_request_bytes(
    total_request_bytes: &mut u64,
    bytes: u64,
    max_total_bytes: u64,
) -> Result<(), AppError> {
    *total_request_bytes = total_request_bytes.checked_add(bytes).ok_or_else(|| {
        AppError::BadRequest("upload request size exceeds the supported limit".into())
    })?;
    if *total_request_bytes > max_total_bytes {
        return Err(AppError::BadRequest(format!(
            "upload request exceeds the maximum size of {}",
            format_bytes(max_total_bytes)
        )));
    }
    Ok(())
}
