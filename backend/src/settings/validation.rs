use super::{SettingKey, SettingsValues, ValidationError};

fn error(field: SettingKey, message: impl Into<String>) -> ValidationError {
    ValidationError {
        field,
        message: message.into(),
    }
}

pub fn validate(values: &SettingsValues) -> Result<(), Vec<ValidationError>> {
    use SettingKey::*;
    let mut errors = Vec::new();
    let positive = [
        (SessionTtlSeconds, values.session_ttl_seconds),
        (
            RegisterIpLimitPerHour,
            values.register_ip_limit_per_hour as u64,
        ),
        (
            LoginIpLimitPerMinute,
            values.login_ip_limit_per_minute as u64,
        ),
        (
            LoginUsernameFailureLimitPer5Minutes,
            values.login_username_failure_limit_per_5_minutes as u64,
        ),
        (Argon2Concurrency, values.argon2_concurrency as u64),
        (IssueMaxContentSize, values.issue_max_content_size),
        (ArchiveMaxWorkingSize, values.archive_max_working_size),
        (
            UploadConcurrentProcessingTasks,
            values.upload_concurrent_processing_tasks as u64,
        ),
        (
            UploadConcurrentReceiveTasks,
            values.upload_concurrent_receive_tasks as u64,
        ),
        (UploadMaxTmpBytes, values.upload_max_tmp_bytes),
        (
            IndexingMaxIndexedLineSize,
            values.indexing_max_indexed_line_size,
        ),
        (
            SearchTantivyMaxWriters,
            values.search_tantivy_max_writers as u64,
        ),
        (
            SearchTantivyWriterHeapSize,
            values.search_tantivy_writer_heap_size,
        ),
        (ApiFilePreviewSize, values.api_file_preview_size),
        (ApiMaxPreviewLineSize, values.api_max_preview_line_size),
        (ApiMaxLinePageBytes, values.api_max_line_page_bytes),
        (
            ApiConcurrentLineReads,
            values.api_concurrent_line_reads as u64,
        ),
        (
            ApiConcurrentLineReadsPerClient,
            values.api_concurrent_line_reads_per_client as u64,
        ),
        (
            TempResultsMaxResultSize,
            values.temp_results_max_result_size,
        ),
        (TempResultsMaxTotalSize, values.temp_results_max_total_size),
        (
            TempResultsMaxRecords,
            values.temp_results_max_records as u64,
        ),
        (
            TempResultsConcurrentMaterializations,
            values.temp_results_concurrent_materializations as u64,
        ),
        (
            TempResultsMaxSources,
            values.temp_results_max_sources as u64,
        ),
        (TempResultsMaxScanBytes, values.temp_results_max_scan_bytes),
        (
            TempResultsMaxScanDurationSeconds,
            values.temp_results_max_scan_duration_seconds,
        ),
    ];
    for (field, value) in positive {
        if value == 0 {
            errors.push(error(field, "must be positive"));
        }
    }
    if values.session_ttl_seconds > 90 * 24 * 60 * 60 {
        errors.push(error(SessionTtlSeconds, "must not exceed 90 days"));
    }
    if values.login_ip_limit_per_minute > 1000 {
        errors.push(error(LoginIpLimitPerMinute, "must not exceed 1000"));
    }
    if values.login_username_failure_limit_per_5_minutes > 100 {
        errors.push(error(
            LoginUsernameFailureLimitPer5Minutes,
            "must not exceed 100",
        ));
    }
    if values.issue_inactive_days != 0 && !(7..=30).contains(&values.issue_inactive_days) {
        errors.push(error(IssueInactiveDays, "must be 0 or between 7 and 30"));
    }
    if values.api_default_line_page_size <= 0
        || values.api_default_line_page_size > values.api_max_line_page_size
    {
        errors.push(error(
            ApiDefaultLinePageSize,
            "must be positive and no greater than max_line_page_size",
        ));
    }
    if values.api_max_line_page_size <= 0 {
        errors.push(error(ApiMaxLinePageSize, "must be positive"));
    }
    if values.api_default_search_results <= 0
        || values.api_default_search_results > values.api_max_search_results
    {
        errors.push(error(
            ApiDefaultSearchResults,
            "must be positive and no greater than max_search_results",
        ));
    }
    if values.api_max_search_results <= 0
        || values.api_max_search_results > values.api_max_search_window
    {
        errors.push(error(
            ApiMaxSearchResults,
            "must be no greater than max_search_window",
        ));
    }
    if values.api_max_search_window <= 0
        || values.api_max_search_window > crate::search::HARD_MAX_SEARCH_WINDOW as i64
    {
        errors.push(error(
            ApiMaxSearchWindow,
            "exceeds the server search window",
        ));
    }
    if values.temp_results_max_result_size > values.temp_results_max_total_size {
        errors.push(error(
            TempResultsMaxResultSize,
            "must be no greater than max_total_size",
        ));
    }
    if values.search_tantivy_writer_heap_size < 16 * 1024 * 1024
        || values.search_tantivy_writer_heap_size > 1024 * 1024 * 1024
    {
        errors.push(error(
            SearchTantivyWriterHeapSize,
            "must be between 16 MiB and 1 GiB",
        ));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
