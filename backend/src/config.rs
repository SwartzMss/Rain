use std::{env, path::PathBuf};

use crate::error::AppError;
use crate::ingest::limits::{
    MAX_ARCHIVE_COMPRESSION_RATIO, MAX_ARCHIVE_ENTRIES, MAX_ARCHIVE_OUTPUT_PATH_CHARS,
    MAX_ARCHIVE_PATH_DEPTH, MAX_ARCHIVE_RECURSION_DEPTH,
};

const KIB: u64 = 1024;
const MIB: u64 = KIB * 1024;
const GIB: u64 = MIB * 1024;

#[derive(Clone)]
pub struct BootstrapAdminConfig {
    pub username: String,
    password: String,
}

impl std::fmt::Debug for BootstrapAdminConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BootstrapAdminConfig")
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

impl BootstrapAdminConfig {
    pub fn password(&self) -> &str {
        &self.password
    }
}

#[derive(Debug, Clone)]
pub struct UploadConfig {
    pub concurrent_processing_tasks: usize,
}

impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            concurrent_processing_tasks: 4,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArchiveConfig {
    pub max_extracted_size: u64,
    pub max_entry_size: u64,
    pub max_entries: usize,
    pub max_path_depth: usize,
    pub max_recursion_depth: usize,
    pub max_output_path_chars: usize,
    pub max_compression_ratio: u64,
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self::for_content_limit(4 * GIB)
    }
}

impl ArchiveConfig {
    pub fn for_content_limit(content_limit: u64) -> Self {
        Self {
            max_extracted_size: content_limit,
            max_entry_size: content_limit,
            max_entries: MAX_ARCHIVE_ENTRIES,
            max_path_depth: MAX_ARCHIVE_PATH_DEPTH,
            max_recursion_depth: MAX_ARCHIVE_RECURSION_DEPTH,
            max_output_path_chars: MAX_ARCHIVE_OUTPUT_PATH_CHARS,
            max_compression_ratio: MAX_ARCHIVE_COMPRESSION_RATIO,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IndexingConfig {
    pub max_indexed_line_size: u64,
}

impl Default for IndexingConfig {
    fn default() -> Self {
        Self {
            max_indexed_line_size: 256 * KIB,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub file_preview_size: u64,
    pub max_preview_line_size: u64,
    pub default_line_page_size: i64,
    pub max_line_page_size: i64,
    pub default_search_results: i64,
    pub max_search_results: i64,
}

#[derive(Debug, Clone)]
pub struct TempResultConfig {
    pub max_result_size: u64,
    pub max_total_size: u64,
    pub max_records: i64,
    pub concurrent_materializations: usize,
}

impl Default for TempResultConfig {
    fn default() -> Self {
        Self {
            max_result_size: 64 * MIB,
            max_total_size: GIB,
            max_records: 1_000,
            concurrent_materializations: 2,
        }
    }
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            file_preview_size: 64 * KIB,
            max_preview_line_size: 8 * MIB,
            default_line_page_size: 5_000,
            max_line_page_size: 10_000,
            default_search_results: 50,
            max_search_results: 100,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppLimits {
    pub issue_max_content_size: u64,
    pub upload: UploadConfig,
    pub indexing: IndexingConfig,
    pub api: ApiConfig,
    pub temp_results: TempResultConfig,
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub allow_registration: bool,
    pub session_ttl_seconds: u64,
    pub argon2_concurrency: usize,
    pub login_ip_limit_per_minute: usize,
    pub login_username_failure_limit_per_5_minutes: usize,
    pub register_ip_limit_per_hour: usize,
}

const MAX_SESSION_TTL_SECONDS: u64 = 90 * 24 * 60 * 60;

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            allow_registration: true,
            session_ttl_seconds: 604_800,
            argon2_concurrency: 5,
            login_ip_limit_per_minute: 20,
            login_username_failure_limit_per_5_minutes: 10,
            register_ip_limit_per_hour: 10,
        }
    }
}

impl AuthConfig {
    fn from_env() -> Result<Self, AppError> {
        let defaults = Self::default();
        let config = Self {
            allow_registration: env_value("RAIN_ALLOW_REGISTRATION", defaults.allow_registration)?,
            session_ttl_seconds: env_value(
                "RAIN_SESSION_TTL_SECONDS",
                defaults.session_ttl_seconds,
            )?,
            argon2_concurrency: env_value(
                "RAIN_AUTH_ARGON2_CONCURRENCY",
                defaults.argon2_concurrency,
            )?,
            login_ip_limit_per_minute: env_value(
                "RAIN_AUTH_LOGIN_IP_LIMIT_PER_MINUTE",
                defaults.login_ip_limit_per_minute,
            )?,
            login_username_failure_limit_per_5_minutes: env_value(
                "RAIN_AUTH_LOGIN_USERNAME_FAILURE_LIMIT_PER_5_MINUTES",
                defaults.login_username_failure_limit_per_5_minutes,
            )?,
            register_ip_limit_per_hour: env_value(
                "RAIN_AUTH_REGISTER_IP_LIMIT_PER_HOUR",
                defaults.register_ip_limit_per_hour,
            )?,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), AppError> {
        if self.session_ttl_seconds == 0 {
            return Err(AppError::Config(
                "RAIN_SESSION_TTL_SECONDS must be positive".into(),
            ));
        }
        if self.session_ttl_seconds > MAX_SESSION_TTL_SECONDS {
            return Err(AppError::Config(format!(
                "RAIN_SESSION_TTL_SECONDS must not exceed {MAX_SESSION_TTL_SECONDS}"
            )));
        }
        if self.argon2_concurrency == 0 {
            return Err(AppError::Config(
                "RAIN_AUTH_ARGON2_CONCURRENCY must be positive".into(),
            ));
        }
        if self.login_ip_limit_per_minute == 0 {
            return Err(AppError::Config(
                "RAIN_AUTH_LOGIN_IP_LIMIT_PER_MINUTE must be positive".into(),
            ));
        }
        if self.login_username_failure_limit_per_5_minutes == 0 {
            return Err(AppError::Config(
                "RAIN_AUTH_LOGIN_USERNAME_FAILURE_LIMIT_PER_5_MINUTES must be positive".into(),
            ));
        }
        if self.register_ip_limit_per_hour == 0 {
            return Err(AppError::Config(
                "RAIN_AUTH_REGISTER_IP_LIMIT_PER_HOUR must be positive".into(),
            ));
        }
        Ok(())
    }
}

impl Default for AppLimits {
    fn default() -> Self {
        Self {
            issue_max_content_size: 4 * GIB,
            upload: UploadConfig::default(),
            indexing: IndexingConfig::default(),
            api: ApiConfig::default(),
            temp_results: TempResultConfig::default(),
        }
    }
}

pub fn parse_byte_size(value: &str) -> Result<u64, String> {
    let value = value.trim();
    let digits_end = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());
    let number = value[..digits_end]
        .parse::<u64>()
        .map_err(|_| format!("invalid byte size '{value}'"))?;
    if number == 0 {
        return Err("byte size must be positive".into());
    }
    let unit = value[digits_end..].trim().to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "" | "b" => 1,
        "kib" => KIB,
        "mib" => MIB,
        "gib" => GIB,
        "tib" => GIB * 1024,
        _ => {
            return Err(format!(
                "unsupported byte size unit '{unit}'; use a binary unit such as KiB, MiB, or GiB"
            ));
        }
    };
    number
        .checked_mul(multiplier)
        .ok_or_else(|| format!("byte size '{value}' exceeds u64"))
}

fn env_value<T>(name: &str, default: T) -> Result<T, AppError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|error| AppError::Config(format!("invalid {name} value '{value}': {error}"))),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(AppError::Config(format!("invalid {name}: {error}"))),
    }
}

fn env_size(name: &str, default: u64) -> Result<u64, AppError> {
    match env::var(name) {
        Ok(value) => parse_byte_size(&value)
            .map_err(|error| AppError::Config(format!("invalid {name} value '{value}': {error}"))),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(AppError::Config(format!("invalid {name}: {error}"))),
    }
}

impl AppLimits {
    fn from_env() -> Result<Self, AppError> {
        let defaults = Self::default();
        let limits = Self {
            issue_max_content_size: env_size(
                "RAIN_ISSUE_MAX_CONTENT_SIZE",
                defaults.issue_max_content_size,
            )?,
            upload: UploadConfig {
                concurrent_processing_tasks: env_value(
                    "RAIN_UPLOAD_CONCURRENT_PROCESSING_TASKS",
                    defaults.upload.concurrent_processing_tasks,
                )?,
            },
            indexing: IndexingConfig {
                max_indexed_line_size: env_size(
                    "RAIN_INDEXING_MAX_INDEXED_LINE_SIZE",
                    defaults.indexing.max_indexed_line_size,
                )?,
            },
            api: ApiConfig {
                file_preview_size: env_size(
                    "RAIN_API_FILE_PREVIEW_SIZE",
                    defaults.api.file_preview_size,
                )?,
                max_preview_line_size: env_size(
                    "RAIN_API_MAX_PREVIEW_LINE_SIZE",
                    defaults.api.max_preview_line_size,
                )?,
                default_line_page_size: env_value(
                    "RAIN_API_DEFAULT_LINE_PAGE_SIZE",
                    defaults.api.default_line_page_size,
                )?,
                max_line_page_size: env_value(
                    "RAIN_API_MAX_LINE_PAGE_SIZE",
                    defaults.api.max_line_page_size,
                )?,
                default_search_results: env_value(
                    "RAIN_API_DEFAULT_SEARCH_RESULTS",
                    defaults.api.default_search_results,
                )?,
                max_search_results: env_value(
                    "RAIN_API_MAX_SEARCH_RESULTS",
                    defaults.api.max_search_results,
                )?,
            },
            temp_results: TempResultConfig {
                max_result_size: env_size(
                    "RAIN_TEMP_RESULT_MAX_SIZE",
                    defaults.temp_results.max_result_size,
                )?,
                max_total_size: env_size(
                    "RAIN_TEMP_RESULT_MAX_TOTAL_SIZE",
                    defaults.temp_results.max_total_size,
                )?,
                max_records: env_value(
                    "RAIN_TEMP_RESULT_MAX_RECORDS",
                    defaults.temp_results.max_records,
                )?,
                concurrent_materializations: env_value(
                    "RAIN_TEMP_RESULT_CONCURRENT_MATERIALIZATIONS",
                    defaults.temp_results.concurrent_materializations,
                )?,
            },
        };
        limits.validate()?;
        Ok(limits)
    }

    pub fn validate(&self) -> Result<(), AppError> {
        macro_rules! positive {
            ($value:expr, $name:literal) => {
                if $value == 0 {
                    return Err(AppError::Config(format!(concat!(
                        $name,
                        " must be positive"
                    ))));
                }
            };
        }
        positive!(self.issue_max_content_size, "RAIN_ISSUE_MAX_CONTENT_SIZE");
        positive!(
            self.upload.concurrent_processing_tasks,
            "RAIN_UPLOAD_CONCURRENT_PROCESSING_TASKS"
        );
        positive!(
            self.indexing.max_indexed_line_size,
            "RAIN_INDEXING_MAX_INDEXED_LINE_SIZE"
        );
        positive!(self.api.file_preview_size, "RAIN_API_FILE_PREVIEW_SIZE");
        positive!(
            self.api.max_preview_line_size,
            "RAIN_API_MAX_PREVIEW_LINE_SIZE"
        );
        positive!(
            self.api.default_line_page_size,
            "RAIN_API_DEFAULT_LINE_PAGE_SIZE"
        );
        positive!(self.api.max_line_page_size, "RAIN_API_MAX_LINE_PAGE_SIZE");
        positive!(
            self.api.default_search_results,
            "RAIN_API_DEFAULT_SEARCH_RESULTS"
        );
        positive!(self.api.max_search_results, "RAIN_API_MAX_SEARCH_RESULTS");
        positive!(
            self.temp_results.max_result_size,
            "RAIN_TEMP_RESULT_MAX_SIZE"
        );
        positive!(
            self.temp_results.max_total_size,
            "RAIN_TEMP_RESULT_MAX_TOTAL_SIZE"
        );
        positive!(
            self.temp_results.max_records,
            "RAIN_TEMP_RESULT_MAX_RECORDS"
        );
        positive!(
            self.temp_results.concurrent_materializations,
            "RAIN_TEMP_RESULT_CONCURRENT_MATERIALIZATIONS"
        );
        if self.temp_results.max_result_size > self.temp_results.max_total_size {
            return Err(AppError::Config(
                "RAIN_TEMP_RESULT_MAX_SIZE must not exceed RAIN_TEMP_RESULT_MAX_TOTAL_SIZE".into(),
            ));
        }
        if self.api.default_line_page_size > self.api.max_line_page_size {
            return Err(AppError::Config(
                "RAIN_API_DEFAULT_LINE_PAGE_SIZE must not exceed RAIN_API_MAX_LINE_PAGE_SIZE"
                    .into(),
            ));
        }
        if self.api.default_search_results > self.api.max_search_results {
            return Err(AppError::Config(
                "RAIN_API_DEFAULT_SEARCH_RESULTS must not exceed RAIN_API_MAX_SEARCH_RESULTS"
                    .into(),
            ));
        }
        usize::try_from(self.indexing.max_indexed_line_size).map_err(|_| {
            AppError::Config(
                "RAIN_INDEXING_MAX_INDEXED_LINE_SIZE cannot be represented on this platform".into(),
            )
        })?;
        usize::try_from(self.api.max_preview_line_size).map_err(|_| {
            AppError::Config(
                "RAIN_API_MAX_PREVIEW_LINE_SIZE cannot be represented on this platform".into(),
            )
        })?;
        Ok(())
    }
}

fn dotenv_path_for_executable(executable: &std::path::Path) -> Option<PathBuf> {
    executable.parent().map(|directory| directory.join(".env"))
}

fn load_dotenv() -> Result<Option<PathBuf>, AppError> {
    if let Ok(executable) = env::current_exe()
        && let Some(path) = dotenv_path_for_executable(&executable)
        && path.is_file()
    {
        dotenvy::from_path(&path).map_err(|error| {
            AppError::Config(format!(
                "failed to load .env file '{}': {}; please save the file as UTF-8",
                path.display(),
                error
            ))
        })?;
        return Ok(Some(path));
    }

    match dotenvy::dotenv() {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.not_found() => Ok(None),
        Err(error) => Err(AppError::Config(format!(
            "failed to load .env: {}; please save the file as UTF-8",
            error
        ))),
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub data_root: PathBuf,
    pub log_dir: PathBuf,
    pub reset_db: bool,
    pub retention_days: Option<u64>,
    pub limits: AppLimits,
    pub auth: AuthConfig,
    pub bootstrap_admin: BootstrapAdminConfig,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, AppError> {
        let dotenv_path = load_dotenv()?;

        if let Some(path) = &dotenv_path {
            eprintln!("loaded environment file: {}", path.display());
        } else {
            eprintln!("environment file not found, using process environment and defaults");
        }
        let host = env::var("SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".into());
        let port: u16 = env::var("SERVER_PORT")
            .unwrap_or_else(|_| "8078".into())
            .parse()
            .map_err(|err| AppError::Config(format!("invalid SERVER_PORT: {err}")))?;

        let database_url =
            env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://./data/rain.db".into());

        let data_root =
            PathBuf::from(env::var("RAIN_DATA_ROOT").unwrap_or_else(|_| "./data/uploads".into()));

        let log_dir = PathBuf::from(env::var("RAIN_LOG_DIR").unwrap_or_else(|_| "./log".into()));

        let reset_db = env::var("RESET_DB")
            .unwrap_or_else(|_| "false".into())
            .parse::<bool>()
            .map_err(|err| AppError::Config(format!("invalid RESET_DB: {err}")))?;

        let retention_days = match env::var("RAIN_RETENTION_DAYS") {
            Ok(value) if !value.trim().is_empty() => {
                let days = value.parse::<u64>().map_err(|err| {
                    AppError::Config(format!("invalid RAIN_RETENTION_DAYS: {err}"))
                })?;
                if days == 0 { None } else { Some(days) }
            }
            _ => None,
        };

        let limits = AppLimits::from_env()?;
        let auth = AuthConfig::from_env()?;
        let bootstrap_admin = BootstrapAdminConfig {
            username: env::var("RAIN_BOOTSTRAP_ADMIN_USERNAME").unwrap_or_else(|_| "admin".into()),
            password: env::var("RAIN_BOOTSTRAP_ADMIN_PASSWORD").unwrap_or_default(),
        };

        eprintln!(
            "bootstrap administrator configuration: username={}, password_configured={}",
            bootstrap_admin.username,
            !bootstrap_admin.password.is_empty()
        );
        Ok(Self {
            host,
            port,
            database_url,
            data_root,
            log_dir,
            reset_db,
            retention_days,
            limits,
            auth,
            bootstrap_admin,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Mutex};

    use super::{
        AppLimits, ArchiveConfig, AuthConfig, dotenv_path_for_executable, parse_byte_size,
    };

    #[test]
    fn resolves_dotenv_next_to_executable() {
        let executable = Path::new("/opt/rain/Rain.exe");

        assert_eq!(
            dotenv_path_for_executable(executable),
            Some(Path::new("/opt/rain/.env").to_path_buf())
        );
    }

    #[test]
    fn parses_human_readable_binary_sizes() {
        assert_eq!(parse_byte_size("64 KiB").unwrap(), 64 * 1024);
        assert_eq!(parse_byte_size("4 gib").unwrap(), 4 * 1024_u64.pow(3));
        assert_eq!(parse_byte_size("20GiB").unwrap(), 20 * 1024_u64.pow(3));
        assert_eq!(parse_byte_size(" 4096 ").unwrap(), 4096);
    }

    #[test]
    fn rejects_invalid_or_overflowing_binary_sizes() {
        assert!(parse_byte_size("1 MB").unwrap_err().contains("binary unit"));
        assert!(parse_byte_size("18446744073709551615 GiB").is_err());
        assert!(parse_byte_size("0 KiB").unwrap_err().contains("positive"));
    }

    #[test]
    fn defaults_expose_only_meaningful_workflow_limits() {
        let limits = AppLimits::default();
        assert_eq!(limits.issue_max_content_size, 4 * 1024_u64.pow(3));
        assert_eq!(limits.upload.concurrent_processing_tasks, 4);
        assert_eq!(limits.indexing.max_indexed_line_size, 256 * 1024);
        assert_eq!(limits.api.file_preview_size, 64 * 1024);
        assert_eq!(limits.api.max_preview_line_size, 8 * 1024_u64.pow(2));
        assert_eq!(limits.api.default_line_page_size, 5_000);
        assert_eq!(limits.api.max_line_page_size, 10_000);
    }

    #[test]
    fn auth_defaults_and_validation_are_safe() {
        let auth = AuthConfig::default();
        assert_eq!(auth.session_ttl_seconds, 604_800);
        assert_eq!(auth.argon2_concurrency, 5);
        assert_eq!(auth.login_ip_limit_per_minute, 20);
        assert_eq!(auth.login_username_failure_limit_per_5_minutes, 10);
        assert_eq!(auth.register_ip_limit_per_hour, 10);
        assert!(auth.validate().is_ok());

        let invalid = AuthConfig {
            session_ttl_seconds: 0,
            ..AuthConfig::default()
        };
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .to_string()
                .contains("RAIN_SESSION_TTL_SECONDS")
        );

        for (invalid, expected_name) in [
            (
                AuthConfig {
                    login_ip_limit_per_minute: 0,
                    ..AuthConfig::default()
                },
                "RAIN_AUTH_LOGIN_IP_LIMIT_PER_MINUTE",
            ),
            (
                AuthConfig {
                    login_username_failure_limit_per_5_minutes: 0,
                    ..AuthConfig::default()
                },
                "RAIN_AUTH_LOGIN_USERNAME_FAILURE_LIMIT_PER_5_MINUTES",
            ),
            (
                AuthConfig {
                    register_ip_limit_per_hour: 0,
                    ..AuthConfig::default()
                },
                "RAIN_AUTH_REGISTER_IP_LIMIT_PER_HOUR",
            ),
        ] {
            assert!(
                invalid
                    .validate()
                    .unwrap_err()
                    .to_string()
                    .contains(expected_name)
            );
        }
    }

    #[test]
    fn rejects_excessive_session_ttl() {
        let auth = AuthConfig {
            session_ttl_seconds: u64::MAX,
            ..AuthConfig::default()
        };
        assert!(
            auth.validate()
                .unwrap_err()
                .to_string()
                .contains("RAIN_SESSION_TTL_SECONDS")
        );
    }

    #[test]
    fn archive_working_budget_uses_issue_content_limit() {
        let limit = 4 * 1024_u64.pow(3);
        let archive = ArchiveConfig::for_content_limit(limit);

        assert_eq!(archive.max_extracted_size, limit);
        assert_eq!(archive.max_entry_size, limit);
    }

    #[test]
    fn validates_cross_field_limit_relationships() {
        let mut limits = AppLimits::default();
        limits.api.default_line_page_size = limits.api.max_line_page_size + 1;
        assert!(
            limits
                .validate()
                .unwrap_err()
                .to_string()
                .contains("RAIN_API_DEFAULT_LINE_PAGE_SIZE")
        );
    }

    #[test]
    fn environment_values_override_limit_defaults() {
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        let name = "RAIN_API_FILE_PREVIEW_SIZE";
        let previous = std::env::var_os(name);
        // SAFETY: This test serializes mutation of this Rain-specific variable and restores it.
        unsafe { std::env::set_var(name, "4 GiB") };

        let limits = AppLimits::from_env().unwrap();

        match previous {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
        assert_eq!(limits.api.file_preview_size, 4 * 1024_u64.pow(3));
    }

    #[test]
    fn environment_values_override_indexed_and_preview_line_limits() {
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        let indexed_name = "RAIN_INDEXING_MAX_INDEXED_LINE_SIZE";
        let preview_name = "RAIN_API_MAX_PREVIEW_LINE_SIZE";
        let previous_indexed = std::env::var_os(indexed_name);
        let previous_preview = std::env::var_os(preview_name);
        unsafe {
            std::env::set_var(indexed_name, "512 KiB");
            std::env::set_var(preview_name, "4 MiB");
        }

        let limits = AppLimits::from_env().unwrap();

        match previous_indexed {
            Some(value) => unsafe { std::env::set_var(indexed_name, value) },
            None => unsafe { std::env::remove_var(indexed_name) },
        }
        match previous_preview {
            Some(value) => unsafe { std::env::set_var(preview_name, value) },
            None => unsafe { std::env::remove_var(preview_name) },
        }
        assert_eq!(limits.indexing.max_indexed_line_size, 512 * 1024);
        assert_eq!(limits.api.max_preview_line_size, 4 * 1024_u64.pow(2));
    }

    #[test]
    fn rejects_zero_indexed_and_preview_line_limits() {
        let mut limits = AppLimits::default();
        limits.indexing.max_indexed_line_size = 0;
        assert!(
            limits
                .validate()
                .unwrap_err()
                .to_string()
                .contains("RAIN_INDEXING_MAX_INDEXED_LINE_SIZE")
        );

        let mut limits = AppLimits::default();
        limits.api.max_preview_line_size = 0;
        assert!(
            limits
                .validate()
                .unwrap_err()
                .to_string()
                .contains("RAIN_API_MAX_PREVIEW_LINE_SIZE")
        );
    }

    #[cfg(target_pointer_width = "32")]
    #[test]
    fn platform_size_errors_name_the_split_line_limits() {
        let mut limits = AppLimits::default();
        limits.indexing.max_indexed_line_size = u64::MAX;
        assert!(
            limits
                .validate()
                .unwrap_err()
                .to_string()
                .contains("RAIN_INDEXING_MAX_INDEXED_LINE_SIZE")
        );

        let mut limits = AppLimits::default();
        limits.api.max_preview_line_size = u64::MAX;
        assert!(
            limits
                .validate()
                .unwrap_err()
                .to_string()
                .contains("RAIN_API_MAX_PREVIEW_LINE_SIZE")
        );
    }

    #[test]
    fn issue_content_limit_environment_value_overrides_default() {
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        let name = "RAIN_ISSUE_MAX_CONTENT_SIZE";
        let previous = std::env::var_os(name);
        unsafe { std::env::set_var(name, "6 GiB") };

        let limits = AppLimits::from_env().unwrap();

        match previous {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
        assert_eq!(limits.issue_max_content_size, 6 * 1024_u64.pow(3));
    }

    #[test]
    fn rejects_zero_issue_content_limit() {
        let limits = AppLimits {
            issue_max_content_size: 0,
            ..AppLimits::default()
        };
        assert!(
            limits
                .validate()
                .unwrap_err()
                .to_string()
                .contains("RAIN_ISSUE_MAX_CONTENT_SIZE")
        );
    }
}
