use serde::{Deserialize, Serialize};

use crate::config::{AppLimits, AuthConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyMode {
    Hot,
    RestartRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceMode {
    Auto,
    Manual,
}

pub type ResourceModes = std::collections::BTreeMap<String, ResourceMode>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKey {
    AllowRegistration,
    SessionTtlSeconds,
    RegisterIpLimitPerHour,
    LoginIpLimitPerMinute,
    LoginUsernameFailureLimitPer5Minutes,
    Argon2Concurrency,
    IssueInactiveDays,
    CleanupExemptUsernames,
    IssueMaxContentSize,
    ArchiveMaxWorkingSize,
    UploadConcurrentProcessingTasks,
    UploadConcurrentReceiveTasks,
    UploadMaxTmpBytes,
    IndexingMaxIndexedLineSize,
    SearchTantivyMaxWriters,
    SearchTantivyWriterHeapSize,
    ApiFilePreviewSize,
    ApiMaxPreviewLineSize,
    ApiDefaultLinePageSize,
    ApiMaxLinePageSize,
    ApiMaxLinePageBytes,
    ApiConcurrentLineReads,
    ApiConcurrentLineReadsPerClient,
    ApiDefaultSearchResults,
    ApiMaxSearchResults,
    ApiMaxSearchWindow,
    TempResultsMaxResultSize,
    TempResultsMaxTotalSize,
    TempResultsMaxRecords,
    TempResultsConcurrentMaterializations,
    TempResultsMaxSources,
    TempResultsMaxScanBytes,
    TempResultsMaxScanDurationSeconds,
}

impl SettingKey {
    pub const ALL: &'static [Self] = &[
        Self::AllowRegistration,
        Self::SessionTtlSeconds,
        Self::RegisterIpLimitPerHour,
        Self::LoginIpLimitPerMinute,
        Self::LoginUsernameFailureLimitPer5Minutes,
        Self::Argon2Concurrency,
        Self::IssueInactiveDays,
        Self::CleanupExemptUsernames,
        Self::IssueMaxContentSize,
        Self::ArchiveMaxWorkingSize,
        Self::UploadConcurrentProcessingTasks,
        Self::UploadConcurrentReceiveTasks,
        Self::UploadMaxTmpBytes,
        Self::IndexingMaxIndexedLineSize,
        Self::SearchTantivyMaxWriters,
        Self::SearchTantivyWriterHeapSize,
        Self::ApiFilePreviewSize,
        Self::ApiMaxPreviewLineSize,
        Self::ApiDefaultLinePageSize,
        Self::ApiMaxLinePageSize,
        Self::ApiMaxLinePageBytes,
        Self::ApiConcurrentLineReads,
        Self::ApiConcurrentLineReadsPerClient,
        Self::ApiDefaultSearchResults,
        Self::ApiMaxSearchResults,
        Self::ApiMaxSearchWindow,
        Self::TempResultsMaxResultSize,
        Self::TempResultsMaxTotalSize,
        Self::TempResultsMaxRecords,
        Self::TempResultsConcurrentMaterializations,
        Self::TempResultsMaxSources,
        Self::TempResultsMaxScanBytes,
        Self::TempResultsMaxScanDurationSeconds,
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum SettingValue {
    Bool(bool),
    Integer(i64),
    Bytes(u64),
    Strings(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsValues {
    pub allow_registration: bool,
    pub session_ttl_seconds: u64,
    pub register_ip_limit_per_hour: usize,
    pub login_ip_limit_per_minute: usize,
    pub login_username_failure_limit_per_5_minutes: usize,
    pub argon2_concurrency: usize,
    pub issue_inactive_days: usize,
    pub cleanup_exempt_usernames: Vec<String>,
    pub issue_max_content_size: u64,
    pub archive_max_working_size: u64,
    pub upload_concurrent_processing_tasks: usize,
    pub upload_concurrent_receive_tasks: usize,
    pub upload_max_tmp_bytes: u64,
    pub indexing_max_indexed_line_size: u64,
    pub search_tantivy_max_writers: usize,
    pub search_tantivy_writer_heap_size: u64,
    pub api_file_preview_size: u64,
    pub api_max_preview_line_size: u64,
    pub api_default_line_page_size: i64,
    pub api_max_line_page_size: i64,
    pub api_max_line_page_bytes: u64,
    pub api_concurrent_line_reads: usize,
    pub api_concurrent_line_reads_per_client: usize,
    pub api_default_search_results: i64,
    pub api_max_search_results: i64,
    pub api_max_search_window: i64,
    pub temp_results_max_result_size: u64,
    pub temp_results_max_total_size: u64,
    pub temp_results_max_records: i64,
    pub temp_results_concurrent_materializations: usize,
    pub temp_results_max_sources: usize,
    pub temp_results_max_scan_bytes: u64,
    pub temp_results_max_scan_duration_seconds: u64,
}

impl SettingsValues {
    pub fn from_config(limits: &AppLimits, auth: &AuthConfig) -> Self {
        Self {
            allow_registration: auth.allow_registration,
            session_ttl_seconds: auth.session_ttl_seconds,
            register_ip_limit_per_hour: auth.register_ip_limit_per_hour,
            login_ip_limit_per_minute: auth.login_ip_limit_per_minute,
            login_username_failure_limit_per_5_minutes: auth
                .login_username_failure_limit_per_5_minutes,
            argon2_concurrency: auth.argon2_concurrency,
            issue_inactive_days: 0,
            cleanup_exempt_usernames: Vec::new(),
            issue_max_content_size: limits.issue_max_content_size,
            archive_max_working_size: limits.archive_max_working_size,
            upload_concurrent_processing_tasks: limits.upload.concurrent_processing_tasks,
            upload_concurrent_receive_tasks: limits.upload.concurrent_receive_tasks,
            upload_max_tmp_bytes: limits.upload.max_tmp_bytes,
            indexing_max_indexed_line_size: limits.indexing.max_indexed_line_size,
            search_tantivy_max_writers: limits.search.tantivy_max_writers,
            search_tantivy_writer_heap_size: limits.search.tantivy_writer_heap_size,
            api_file_preview_size: limits.api.file_preview_size,
            api_max_preview_line_size: limits.api.max_preview_line_size,
            api_default_line_page_size: limits.api.default_line_page_size,
            api_max_line_page_size: limits.api.max_line_page_size,
            api_max_line_page_bytes: limits.api.max_line_page_bytes,
            api_concurrent_line_reads: limits.api.concurrent_line_reads,
            api_concurrent_line_reads_per_client: limits.api.concurrent_line_reads_per_client,
            api_default_search_results: limits.api.default_search_results,
            api_max_search_results: limits.api.max_search_results,
            api_max_search_window: limits.api.max_search_window,
            temp_results_max_result_size: limits.temp_results.max_result_size,
            temp_results_max_total_size: limits.temp_results.max_total_size,
            temp_results_max_records: limits.temp_results.max_records,
            temp_results_concurrent_materializations: limits
                .temp_results
                .concurrent_materializations,
            temp_results_max_sources: limits.temp_results.max_sources,
            temp_results_max_scan_bytes: limits.temp_results.max_scan_bytes,
            temp_results_max_scan_duration_seconds: limits.temp_results.max_scan_duration_seconds,
        }
    }

    pub fn apply_to_config(&self, limits: &mut AppLimits, auth: &mut AuthConfig) {
        auth.allow_registration = self.allow_registration;
        auth.session_ttl_seconds = self.session_ttl_seconds;
        auth.register_ip_limit_per_hour = self.register_ip_limit_per_hour;
        auth.login_ip_limit_per_minute = self.login_ip_limit_per_minute;
        auth.login_username_failure_limit_per_5_minutes =
            self.login_username_failure_limit_per_5_minutes;
        auth.argon2_concurrency = self.argon2_concurrency;
        limits.issue_max_content_size = self.issue_max_content_size;
        limits.archive_max_working_size = self.archive_max_working_size;
        limits.upload.concurrent_processing_tasks = self.upload_concurrent_processing_tasks;
        limits.upload.concurrent_receive_tasks = self.upload_concurrent_receive_tasks;
        limits.upload.max_tmp_bytes = self.upload_max_tmp_bytes;
        limits.indexing.max_indexed_line_size = self.indexing_max_indexed_line_size;
        limits.search.tantivy_max_writers = self.search_tantivy_max_writers;
        limits.search.tantivy_writer_heap_size = self.search_tantivy_writer_heap_size;
        limits.api.file_preview_size = self.api_file_preview_size;
        limits.api.max_preview_line_size = self.api_max_preview_line_size;
        limits.api.default_line_page_size = self.api_default_line_page_size;
        limits.api.max_line_page_size = self.api_max_line_page_size;
        limits.api.max_line_page_bytes = self.api_max_line_page_bytes;
        limits.api.concurrent_line_reads = self.api_concurrent_line_reads;
        limits.api.concurrent_line_reads_per_client = self.api_concurrent_line_reads_per_client;
        limits.api.default_search_results = self.api_default_search_results;
        limits.api.max_search_results = self.api_max_search_results;
        limits.api.max_search_window = self.api_max_search_window;
        limits.temp_results.max_result_size = self.temp_results_max_result_size;
        limits.temp_results.max_total_size = self.temp_results_max_total_size;
        limits.temp_results.max_records = self.temp_results_max_records;
        limits.temp_results.concurrent_materializations =
            self.temp_results_concurrent_materializations;
        limits.temp_results.max_sources = self.temp_results_max_sources;
        limits.temp_results.max_scan_bytes = self.temp_results_max_scan_bytes;
        limits.temp_results.max_scan_duration_seconds = self.temp_results_max_scan_duration_seconds;
    }

    pub fn validate(&self) -> Result<(), Vec<ValidationError>> {
        crate::settings::validation::validate(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidationError {
    pub field: SettingKey,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsSnapshot {
    pub revision: i64,
    pub configured: SettingsValues,
    pub effective: SettingsValues,
    pub resource_modes: ResourceModes,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SaveResult {
    pub snapshot: SettingsSnapshot,
    pub changed_fields: Vec<String>,
    pub hot_applied_fields: Vec<String>,
    pub pending_restart_fields: Vec<String>,
}
