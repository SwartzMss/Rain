use serde::{Serialize, Serializer, ser::SerializeStruct};

use super::{ApplyMode, SettingKey};

#[derive(Debug, Clone, Copy)]
pub struct FieldMetadata {
    pub key: SettingKey,
    pub db_column: &'static str,
    pub env_name: &'static str,
    pub apply_mode: ApplyMode,
}

impl Serialize for FieldMetadata {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let (value_type, unit, default_value, default_rule, min, max, description) =
            details(self.key);
        let mut output = serializer.serialize_struct("FieldMetadata", 13)?;
        output.serialize_field("key", &self.key)?;
        output.serialize_field("db_column", self.db_column)?;
        output.serialize_field("env_name", self.env_name)?;
        output.serialize_field("value_type", value_type)?;
        output.serialize_field("unit", &unit)?;
        output.serialize_field("default_value", &default_value)?;
        output.serialize_field("default_rule", &default_rule)?;
        output.serialize_field("min", &min)?;
        output.serialize_field("max", &max)?;
        output.serialize_field("description", description)?;
        output.serialize_field("apply_mode", &self.apply_mode)?;
        output.serialize_field(
            "pending_restart",
            &matches!(self.apply_mode, ApplyMode::RestartRequired),
        )?;
        output.serialize_field("sensitive", &false)?;
        output.end()
    }
}

type FieldDetails = (
    &'static str,
    Option<&'static str>,
    serde_json::Value,
    Option<&'static str>,
    Option<u64>,
    Option<u64>,
    &'static str,
);

fn details(key: SettingKey) -> FieldDetails {
    use SettingKey::*;
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    let integer = |default: u64, min: u64, max: Option<u64>, description| {
        (
            "integer",
            None,
            serde_json::json!(default),
            None,
            Some(min),
            max,
            description,
        )
    };
    match key {
        AllowRegistration => (
            "boolean",
            None,
            serde_json::json!(true),
            None,
            None,
            None,
            "是否允许新用户注册",
        ),
        SessionTtlSeconds => integer(604_800, 1, Some(90 * 24 * 60 * 60), "新建会话的有效期"),
        RegisterIpLimitPerHour => integer(10, 1, None, "单 IP 每小时注册尝试次数"),
        LoginIpLimitPerMinute => integer(20, 1, Some(1000), "单 IP 每分钟登录失败尝试次数"),
        LoginUsernameFailureLimitPer5Minutes => {
            integer(10, 1, Some(100), "单用户名五分钟登录失败次数")
        }
        Argon2Concurrency => integer(5, 1, None, "Argon2 并发计算数"),
        IssueInactiveDays => (
            "integer",
            Some("days"),
            serde_json::json!(0),
            None,
            Some(0),
            Some(30),
            "Issue 自动清理闲置天数；0 表示关闭",
        ),
        CleanupExemptUsernames => (
            "string_array",
            None,
            serde_json::json!([]),
            None,
            None,
            Some(200),
            "自动清理白名单用户名",
        ),
        IssueMaxContentSize => (
            "integer",
            Some("bytes"),
            serde_json::json!(8 * GIB),
            None,
            Some(1),
            None,
            "单个 Issue 的内容总量上限",
        ),
        ArchiveMaxWorkingSize => (
            "integer",
            Some("bytes"),
            serde_json::json!(16 * GIB),
            Some("首次初始化默认为 issue_max_content_size 的两倍"),
            Some(1),
            None,
            "归档解压工作区上限",
        ),
        UploadConcurrentProcessingTasks => integer(4, 1, None, "上传处理并发数"),
        UploadConcurrentReceiveTasks => integer(4, 1, None, "上传接收并发数"),
        UploadMaxTmpBytes => (
            "integer",
            Some("bytes"),
            serde_json::json!(32 * GIB),
            None,
            Some(1),
            None,
            "上传临时空间上限",
        ),
        IndexingMaxIndexedLineSize => (
            "integer",
            Some("bytes"),
            serde_json::json!(256 * KIB),
            None,
            Some(1),
            None,
            "索引单行最大字节数",
        ),
        SearchTantivyMaxWriters => integer(1, 1, None, "Tantivy writer 并发数"),
        SearchTantivyWriterHeapSize => (
            "integer",
            Some("bytes"),
            serde_json::json!(64 * MIB),
            None,
            Some(16 * MIB),
            Some(1024 * MIB),
            "单个 Tantivy writer 的堆预算",
        ),
        ApiFilePreviewSize => (
            "integer",
            Some("bytes"),
            serde_json::json!(64 * KIB),
            None,
            Some(1),
            None,
            "文件预览返回字节数",
        ),
        ApiMaxPreviewLineSize => (
            "integer",
            Some("bytes"),
            serde_json::json!(8 * MIB),
            None,
            Some(1),
            None,
            "预览单行最大字节数",
        ),
        ApiDefaultLinePageSize => integer(5000, 1, None, "行读取默认页大小"),
        ApiMaxLinePageSize => integer(10000, 1, None, "行读取最大页大小"),
        ApiMaxLinePageBytes => (
            "integer",
            Some("bytes"),
            serde_json::json!(16 * MIB),
            None,
            Some(1),
            None,
            "行读取响应最大字节数",
        ),
        ApiConcurrentLineReads => integer(8, 1, None, "全局行读取并发数"),
        ApiConcurrentLineReadsPerClient => integer(2, 1, None, "单客户端行读取并发数"),
        ApiDefaultSearchResults => integer(50, 1, None, "搜索默认结果数"),
        ApiMaxSearchResults => integer(100, 1, None, "搜索最大结果数"),
        ApiMaxSearchWindow => integer(10000, 1, Some(100000), "搜索窗口最大结果数"),
        TempResultsMaxResultSize => (
            "integer",
            Some("bytes"),
            serde_json::json!(64 * MIB),
            None,
            Some(1),
            None,
            "单个临时结果大小上限",
        ),
        TempResultsMaxTotalSize => (
            "integer",
            Some("bytes"),
            serde_json::json!(GIB),
            None,
            Some(1),
            None,
            "临时结果总计费大小上限",
        ),
        TempResultsMaxRecords => integer(1000, 1, None, "临时结果记录数上限"),
        TempResultsConcurrentMaterializations => integer(2, 1, None, "临时结果物化并发数"),
        TempResultsMaxSources => integer(10000, 1, None, "临时结果来源数上限"),
        TempResultsMaxScanBytes => (
            "integer",
            Some("bytes"),
            serde_json::json!(GIB),
            None,
            Some(1),
            None,
            "临时结果扫描字节上限",
        ),
        TempResultsMaxScanDurationSeconds => {
            integer(30, 1, Some(31_536_000), "临时结果扫描超时时间")
        }
    }
}

pub fn all() -> &'static [FieldMetadata] {
    use ApplyMode::{Hot as H, RestartRequired as R};
    use SettingKey::*;
    &[
        FieldMetadata {
            key: AllowRegistration,
            db_column: "allow_registration",
            env_name: "RAIN_ALLOW_REGISTRATION",
            apply_mode: H,
        },
        FieldMetadata {
            key: SessionTtlSeconds,
            db_column: "session_ttl_seconds",
            env_name: "RAIN_SESSION_TTL_SECONDS",
            apply_mode: H,
        },
        FieldMetadata {
            key: RegisterIpLimitPerHour,
            db_column: "register_ip_limit_per_hour",
            env_name: "RAIN_AUTH_REGISTER_IP_LIMIT_PER_HOUR",
            apply_mode: H,
        },
        FieldMetadata {
            key: LoginIpLimitPerMinute,
            db_column: "login_ip_limit_per_minute",
            env_name: "RAIN_AUTH_LOGIN_IP_LIMIT_PER_MINUTE",
            apply_mode: H,
        },
        FieldMetadata {
            key: LoginUsernameFailureLimitPer5Minutes,
            db_column: "login_username_failure_limit_per_5_minutes",
            env_name: "RAIN_AUTH_LOGIN_USERNAME_FAILURE_LIMIT_PER_5_MINUTES",
            apply_mode: H,
        },
        FieldMetadata {
            key: Argon2Concurrency,
            db_column: "argon2_concurrency",
            env_name: "RAIN_AUTH_ARGON2_CONCURRENCY",
            apply_mode: R,
        },
        FieldMetadata {
            key: IssueInactiveDays,
            db_column: "issue_inactive_days",
            env_name: "RAIN_ISSUE_INACTIVE_DAYS",
            apply_mode: H,
        },
        FieldMetadata {
            key: CleanupExemptUsernames,
            db_column: "cleanup_exempt_usernames_json",
            env_name: "RAIN_CLEANUP_EXEMPT_USERS",
            apply_mode: H,
        },
        FieldMetadata {
            key: IssueMaxContentSize,
            db_column: "issue_max_content_size",
            env_name: "RAIN_ISSUE_MAX_CONTENT_SIZE",
            apply_mode: H,
        },
        FieldMetadata {
            key: ArchiveMaxWorkingSize,
            db_column: "archive_max_working_size",
            env_name: "RAIN_ARCHIVE_MAX_WORKING_SIZE",
            apply_mode: H,
        },
        FieldMetadata {
            key: UploadConcurrentProcessingTasks,
            db_column: "upload_concurrent_processing_tasks",
            env_name: "RAIN_UPLOAD_CONCURRENT_PROCESSING_TASKS",
            apply_mode: R,
        },
        FieldMetadata {
            key: UploadConcurrentReceiveTasks,
            db_column: "upload_concurrent_receive_tasks",
            env_name: "RAIN_UPLOAD_CONCURRENT_RECEIVE_TASKS",
            apply_mode: R,
        },
        FieldMetadata {
            key: UploadMaxTmpBytes,
            db_column: "upload_max_tmp_bytes",
            env_name: "RAIN_UPLOAD_MAX_TMP_BYTES",
            apply_mode: H,
        },
        FieldMetadata {
            key: IndexingMaxIndexedLineSize,
            db_column: "indexing_max_indexed_line_size",
            env_name: "RAIN_INDEXING_MAX_INDEXED_LINE_SIZE",
            apply_mode: R,
        },
        FieldMetadata {
            key: SearchTantivyMaxWriters,
            db_column: "search_tantivy_max_writers",
            env_name: "RAIN_SEARCH_TANTIVY_MAX_WRITERS",
            apply_mode: R,
        },
        FieldMetadata {
            key: SearchTantivyWriterHeapSize,
            db_column: "search_tantivy_writer_heap_size",
            env_name: "RAIN_SEARCH_TANTIVY_WRITER_HEAP",
            apply_mode: R,
        },
        FieldMetadata {
            key: ApiFilePreviewSize,
            db_column: "api_file_preview_size",
            env_name: "RAIN_API_FILE_PREVIEW_SIZE",
            apply_mode: H,
        },
        FieldMetadata {
            key: ApiMaxPreviewLineSize,
            db_column: "api_max_preview_line_size",
            env_name: "RAIN_API_MAX_PREVIEW_LINE_SIZE",
            apply_mode: H,
        },
        FieldMetadata {
            key: ApiDefaultLinePageSize,
            db_column: "api_default_line_page_size",
            env_name: "RAIN_API_DEFAULT_LINE_PAGE_SIZE",
            apply_mode: H,
        },
        FieldMetadata {
            key: ApiMaxLinePageSize,
            db_column: "api_max_line_page_size",
            env_name: "RAIN_API_MAX_LINE_PAGE_SIZE",
            apply_mode: H,
        },
        FieldMetadata {
            key: ApiMaxLinePageBytes,
            db_column: "api_max_line_page_bytes",
            env_name: "RAIN_API_MAX_LINE_PAGE_BYTES",
            apply_mode: H,
        },
        FieldMetadata {
            key: ApiConcurrentLineReads,
            db_column: "api_concurrent_line_reads",
            env_name: "RAIN_API_CONCURRENT_LINE_READS",
            apply_mode: R,
        },
        FieldMetadata {
            key: ApiConcurrentLineReadsPerClient,
            db_column: "api_concurrent_line_reads_per_client",
            env_name: "RAIN_API_CONCURRENT_LINE_READS_PER_CLIENT",
            apply_mode: H,
        },
        FieldMetadata {
            key: ApiDefaultSearchResults,
            db_column: "api_default_search_results",
            env_name: "RAIN_API_DEFAULT_SEARCH_RESULTS",
            apply_mode: H,
        },
        FieldMetadata {
            key: ApiMaxSearchResults,
            db_column: "api_max_search_results",
            env_name: "RAIN_API_MAX_SEARCH_RESULTS",
            apply_mode: H,
        },
        FieldMetadata {
            key: ApiMaxSearchWindow,
            db_column: "api_max_search_window",
            env_name: "RAIN_API_MAX_SEARCH_WINDOW",
            apply_mode: H,
        },
        FieldMetadata {
            key: TempResultsMaxResultSize,
            db_column: "temp_results_max_result_size",
            env_name: "RAIN_TEMP_RESULT_MAX_SIZE",
            apply_mode: H,
        },
        FieldMetadata {
            key: TempResultsMaxTotalSize,
            db_column: "temp_results_max_total_size",
            env_name: "RAIN_TEMP_RESULT_MAX_TOTAL_SIZE",
            apply_mode: H,
        },
        FieldMetadata {
            key: TempResultsMaxRecords,
            db_column: "temp_results_max_records",
            env_name: "RAIN_TEMP_RESULT_MAX_RECORDS",
            apply_mode: H,
        },
        FieldMetadata {
            key: TempResultsConcurrentMaterializations,
            db_column: "temp_results_concurrent_materializations",
            env_name: "RAIN_TEMP_RESULT_CONCURRENT_MATERIALIZATIONS",
            apply_mode: R,
        },
        FieldMetadata {
            key: TempResultsMaxSources,
            db_column: "temp_results_max_sources",
            env_name: "RAIN_TEMP_RESULT_MAX_SOURCES",
            apply_mode: H,
        },
        FieldMetadata {
            key: TempResultsMaxScanBytes,
            db_column: "temp_results_max_scan_bytes",
            env_name: "RAIN_TEMP_RESULT_MAX_SCAN_BYTES",
            apply_mode: H,
        },
        FieldMetadata {
            key: TempResultsMaxScanDurationSeconds,
            db_column: "temp_results_max_scan_duration_seconds",
            env_name: "RAIN_TEMP_RESULT_MAX_SCAN_DURATION_SECONDS",
            apply_mode: H,
        },
    ]
}
