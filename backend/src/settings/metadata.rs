use serde::{Serialize, Serializer, ser::SerializeStruct};

use super::{ApplyMode, SettingKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingCategory {
    Common,
    Advanced,
    Expert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingVisibility {
    Default,
    Collapsed,
    Expert,
}

#[derive(Debug, Clone, Copy)]
pub struct FieldMetadata {
    pub key: SettingKey,
    pub db_column: &'static str,
    pub env_name: &'static str,
    pub apply_mode: ApplyMode,
    pub category: SettingCategory,
    pub visibility: SettingVisibility,
    pub recommended_min: Option<u64>,
    pub recommended_max: Option<u64>,
    pub supports_auto: bool,
    pub auto_value: Option<u64>,
    pub protected: bool,
}

impl FieldMetadata {
    pub const fn new(
        key: SettingKey,
        db_column: &'static str,
        env_name: &'static str,
        apply_mode: ApplyMode,
    ) -> Self {
        let (
            category,
            visibility,
            recommended_min,
            recommended_max,
            supports_auto,
            auto_value,
            protected,
        ) = presentation(key);
        Self {
            key,
            db_column,
            env_name,
            apply_mode,
            category,
            visibility,
            recommended_min,
            recommended_max,
            supports_auto,
            auto_value,
            protected,
        }
    }
}

impl Serialize for FieldMetadata {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let (value_type, unit, default_value, default_rule, min, max, description) =
            details(self.key);
        let mut output = serializer.serialize_struct("FieldMetadata", 20)?;
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
        output.serialize_field("category", &self.category)?;
        output.serialize_field("visibility", &self.visibility)?;
        output.serialize_field("recommended_min", &self.recommended_min)?;
        output.serialize_field("recommended_max", &self.recommended_max)?;
        output.serialize_field("supports_auto", &self.supports_auto)?;
        output.serialize_field("auto_value", &self.auto_value)?;
        output.serialize_field("protected", &self.protected)?;
        output.end()
    }
}

type Presentation = (
    SettingCategory,
    SettingVisibility,
    Option<u64>,
    Option<u64>,
    bool,
    Option<u64>,
    bool,
);

const fn presentation(key: SettingKey) -> Presentation {
    use SettingCategory::{Advanced, Common, Expert};
    use SettingKey::*;
    use SettingVisibility::{Collapsed, Default, Expert as ExpertVisibility};

    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;

    match key {
        AllowRegistration => (Common, Default, None, None, false, None, false),
        SessionTtlSeconds => (
            Common,
            Default,
            Some(86_400),
            Some(2_592_000),
            false,
            None,
            false,
        ),
        RegisterIpLimitPerHour => (Common, Default, Some(1), Some(100), false, None, false),
        LoginIpLimitPerMinute => (Common, Default, Some(5), Some(100), false, None, false),
        LoginUsernameFailureLimitPer5Minutes => {
            (Common, Default, Some(5), Some(50), false, None, false)
        }
        Argon2Concurrency => (
            Expert,
            ExpertVisibility,
            Some(1),
            Some(16),
            false,
            None,
            true,
        ),
        IssueInactiveDays => (Common, Default, Some(7), Some(30), false, None, false),
        CleanupExemptUsernames => (Common, Default, None, None, false, None, false),
        IssueMaxContentSize => (
            Common,
            Default,
            Some(GIB),
            Some(32 * GIB),
            false,
            None,
            false,
        ),
        ArchiveMaxWorkingSize => (
            Advanced,
            Collapsed,
            Some(2 * GIB),
            Some(64 * GIB),
            false,
            None,
            false,
        ),
        UploadConcurrentProcessingTasks => {
            (Advanced, Collapsed, Some(1), Some(8), true, Some(4), false)
        }
        UploadConcurrentReceiveTasks => {
            (Advanced, Collapsed, Some(1), Some(8), true, Some(4), false)
        }
        UploadMaxTmpBytes => (
            Advanced,
            Collapsed,
            Some(8 * GIB),
            Some(128 * GIB),
            false,
            None,
            false,
        ),
        IndexingMaxIndexedLineSize => (
            Expert,
            ExpertVisibility,
            Some(64 * KIB),
            Some(MIB),
            false,
            None,
            false,
        ),
        SearchTantivyMaxWriters => (
            Expert,
            ExpertVisibility,
            Some(1),
            Some(4),
            true,
            Some(1),
            false,
        ),
        SearchTantivyWriterHeapSize => (
            Expert,
            ExpertVisibility,
            Some(16 * MIB),
            Some(256 * MIB),
            true,
            Some(64 * MIB),
            false,
        ),
        ApiFilePreviewSize => (
            Advanced,
            Collapsed,
            Some(16 * KIB),
            Some(MIB),
            false,
            None,
            false,
        ),
        ApiMaxPreviewLineSize => (
            Advanced,
            Collapsed,
            Some(64 * KIB),
            Some(16 * MIB),
            false,
            None,
            false,
        ),
        ApiDefaultLinePageSize => (
            Advanced,
            Collapsed,
            Some(100),
            Some(5_000),
            false,
            None,
            false,
        ),
        ApiMaxLinePageSize => (
            Advanced,
            Collapsed,
            Some(1_000),
            Some(10_000),
            false,
            None,
            false,
        ),
        ApiMaxLinePageBytes => (
            Advanced,
            Collapsed,
            Some(MIB),
            Some(32 * MIB),
            false,
            None,
            false,
        ),
        ApiConcurrentLineReads => (
            Expert,
            ExpertVisibility,
            Some(1),
            Some(16),
            true,
            Some(8),
            false,
        ),
        ApiConcurrentLineReadsPerClient => (
            Expert,
            ExpertVisibility,
            Some(1),
            Some(4),
            false,
            None,
            false,
        ),
        ApiDefaultSearchResults => (Common, Default, Some(10), Some(100), false, None, false),
        ApiMaxSearchResults => (Advanced, Collapsed, Some(50), Some(500), false, None, false),
        ApiMaxSearchWindow => (
            Advanced,
            Collapsed,
            Some(1_000),
            Some(50_000),
            false,
            None,
            false,
        ),
        TempResultsMaxResultSize => (
            Advanced,
            Collapsed,
            Some(16 * MIB),
            Some(256 * MIB),
            false,
            None,
            false,
        ),
        TempResultsMaxTotalSize => (
            Advanced,
            Collapsed,
            Some(256 * MIB),
            Some(8 * GIB),
            false,
            None,
            false,
        ),
        TempResultsMaxRecords => (
            Advanced,
            Collapsed,
            Some(100),
            Some(10_000),
            false,
            None,
            false,
        ),
        TempResultsConcurrentMaterializations => (
            Expert,
            ExpertVisibility,
            Some(1),
            Some(4),
            true,
            Some(2),
            false,
        ),
        TempResultsMaxSources => (
            Advanced,
            Collapsed,
            Some(1_000),
            Some(50_000),
            false,
            None,
            false,
        ),
        TempResultsMaxScanBytes => (
            Advanced,
            Collapsed,
            Some(256 * MIB),
            Some(8 * GIB),
            false,
            None,
            false,
        ),
        TempResultsMaxScanDurationSeconds => {
            (Advanced, Collapsed, Some(10), Some(300), false, None, false)
        }
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
    const FIELDS: &[FieldMetadata] = &[
        FieldMetadata::new(
            AllowRegistration,
            "allow_registration",
            "RAIN_ALLOW_REGISTRATION",
            H,
        ),
        FieldMetadata::new(
            SessionTtlSeconds,
            "session_ttl_seconds",
            "RAIN_SESSION_TTL_SECONDS",
            H,
        ),
        FieldMetadata::new(
            RegisterIpLimitPerHour,
            "register_ip_limit_per_hour",
            "RAIN_AUTH_REGISTER_IP_LIMIT_PER_HOUR",
            H,
        ),
        FieldMetadata::new(
            LoginIpLimitPerMinute,
            "login_ip_limit_per_minute",
            "RAIN_AUTH_LOGIN_IP_LIMIT_PER_MINUTE",
            H,
        ),
        FieldMetadata::new(
            LoginUsernameFailureLimitPer5Minutes,
            "login_username_failure_limit_per_5_minutes",
            "RAIN_AUTH_LOGIN_USERNAME_FAILURE_LIMIT_PER_5_MINUTES",
            H,
        ),
        FieldMetadata::new(
            Argon2Concurrency,
            "argon2_concurrency",
            "RAIN_AUTH_ARGON2_CONCURRENCY",
            R,
        ),
        FieldMetadata::new(
            IssueInactiveDays,
            "issue_inactive_days",
            "RAIN_ISSUE_INACTIVE_DAYS",
            H,
        ),
        FieldMetadata::new(
            CleanupExemptUsernames,
            "cleanup_exempt_usernames_json",
            "RAIN_CLEANUP_EXEMPT_USERS",
            H,
        ),
        FieldMetadata::new(
            IssueMaxContentSize,
            "issue_max_content_size",
            "RAIN_ISSUE_MAX_CONTENT_SIZE",
            H,
        ),
        FieldMetadata::new(
            ArchiveMaxWorkingSize,
            "archive_max_working_size",
            "RAIN_ARCHIVE_MAX_WORKING_SIZE",
            H,
        ),
        FieldMetadata::new(
            UploadConcurrentProcessingTasks,
            "upload_concurrent_processing_tasks",
            "RAIN_UPLOAD_CONCURRENT_PROCESSING_TASKS",
            R,
        ),
        FieldMetadata::new(
            UploadConcurrentReceiveTasks,
            "upload_concurrent_receive_tasks",
            "RAIN_UPLOAD_CONCURRENT_RECEIVE_TASKS",
            R,
        ),
        FieldMetadata::new(
            UploadMaxTmpBytes,
            "upload_max_tmp_bytes",
            "RAIN_UPLOAD_MAX_TMP_BYTES",
            H,
        ),
        FieldMetadata::new(
            IndexingMaxIndexedLineSize,
            "indexing_max_indexed_line_size",
            "RAIN_INDEXING_MAX_INDEXED_LINE_SIZE",
            R,
        ),
        FieldMetadata::new(
            SearchTantivyMaxWriters,
            "search_tantivy_max_writers",
            "RAIN_SEARCH_TANTIVY_MAX_WRITERS",
            R,
        ),
        FieldMetadata::new(
            SearchTantivyWriterHeapSize,
            "search_tantivy_writer_heap_size",
            "RAIN_SEARCH_TANTIVY_WRITER_HEAP",
            R,
        ),
        FieldMetadata::new(
            ApiFilePreviewSize,
            "api_file_preview_size",
            "RAIN_API_FILE_PREVIEW_SIZE",
            H,
        ),
        FieldMetadata::new(
            ApiMaxPreviewLineSize,
            "api_max_preview_line_size",
            "RAIN_API_MAX_PREVIEW_LINE_SIZE",
            H,
        ),
        FieldMetadata::new(
            ApiDefaultLinePageSize,
            "api_default_line_page_size",
            "RAIN_API_DEFAULT_LINE_PAGE_SIZE",
            H,
        ),
        FieldMetadata::new(
            ApiMaxLinePageSize,
            "api_max_line_page_size",
            "RAIN_API_MAX_LINE_PAGE_SIZE",
            H,
        ),
        FieldMetadata::new(
            ApiMaxLinePageBytes,
            "api_max_line_page_bytes",
            "RAIN_API_MAX_LINE_PAGE_BYTES",
            H,
        ),
        FieldMetadata::new(
            ApiConcurrentLineReads,
            "api_concurrent_line_reads",
            "RAIN_API_CONCURRENT_LINE_READS",
            R,
        ),
        FieldMetadata::new(
            ApiConcurrentLineReadsPerClient,
            "api_concurrent_line_reads_per_client",
            "RAIN_API_CONCURRENT_LINE_READS_PER_CLIENT",
            H,
        ),
        FieldMetadata::new(
            ApiDefaultSearchResults,
            "api_default_search_results",
            "RAIN_API_DEFAULT_SEARCH_RESULTS",
            H,
        ),
        FieldMetadata::new(
            ApiMaxSearchResults,
            "api_max_search_results",
            "RAIN_API_MAX_SEARCH_RESULTS",
            H,
        ),
        FieldMetadata::new(
            ApiMaxSearchWindow,
            "api_max_search_window",
            "RAIN_API_MAX_SEARCH_WINDOW",
            H,
        ),
        FieldMetadata::new(
            TempResultsMaxResultSize,
            "temp_results_max_result_size",
            "RAIN_TEMP_RESULT_MAX_SIZE",
            H,
        ),
        FieldMetadata::new(
            TempResultsMaxTotalSize,
            "temp_results_max_total_size",
            "RAIN_TEMP_RESULT_MAX_TOTAL_SIZE",
            H,
        ),
        FieldMetadata::new(
            TempResultsMaxRecords,
            "temp_results_max_records",
            "RAIN_TEMP_RESULT_MAX_RECORDS",
            H,
        ),
        FieldMetadata::new(
            TempResultsConcurrentMaterializations,
            "temp_results_concurrent_materializations",
            "RAIN_TEMP_RESULT_CONCURRENT_MATERIALIZATIONS",
            R,
        ),
        FieldMetadata::new(
            TempResultsMaxSources,
            "temp_results_max_sources",
            "RAIN_TEMP_RESULT_MAX_SOURCES",
            H,
        ),
        FieldMetadata::new(
            TempResultsMaxScanBytes,
            "temp_results_max_scan_bytes",
            "RAIN_TEMP_RESULT_MAX_SCAN_BYTES",
            H,
        ),
        FieldMetadata::new(
            TempResultsMaxScanDurationSeconds,
            "temp_results_max_scan_duration_seconds",
            "RAIN_TEMP_RESULT_MAX_SCAN_DURATION_SECONDS",
            H,
        ),
    ];
    FIELDS
}

pub fn admin() -> Vec<&'static FieldMetadata> {
    all().iter().filter(|field| !field.protected).collect()
}
