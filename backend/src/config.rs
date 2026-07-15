use std::{env, path::PathBuf};

use crate::error::AppError;

const KIB: u64 = 1024;
const MIB: u64 = KIB * 1024;
const GIB: u64 = MIB * 1024;

#[derive(Debug, Clone)]
pub struct UploadConfig {
    pub max_files: usize,
    pub max_file_size: u64,
    pub max_total_size: u64,
    pub max_text_field_size: u64,
    pub concurrent_processing_tasks: usize,
}

impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            max_files: 100,
            max_file_size: 512 * MIB,
            max_total_size: 2 * GIB,
            max_text_field_size: 64 * KIB,
            concurrent_processing_tasks: 2,
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
        Self {
            max_extracted_size: 500 * MIB,
            max_entry_size: 100 * MIB,
            max_entries: 10_000,
            max_path_depth: 16,
            max_recursion_depth: 16,
            max_output_path_chars: 1024,
            max_compression_ratio: 100,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IndexingConfig {
    pub max_line_size: u64,
    pub chunk_lines: usize,
    pub commit_lines: i64,
    pub line_offset_interval: i64,
}

impl Default for IndexingConfig {
    fn default() -> Self {
        Self {
            max_line_size: MIB,
            chunk_lines: 200,
            commit_lines: 5000,
            line_offset_interval: 1000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub file_preview_size: u64,
    pub default_line_page_size: i64,
    pub max_line_page_size: i64,
    pub default_search_results: i64,
    pub max_search_results: i64,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            file_preview_size: 64 * KIB,
            default_line_page_size: 1000,
            max_line_page_size: 3000,
            default_search_results: 50,
            max_search_results: 100,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AppLimits {
    pub upload: UploadConfig,
    pub archive: ArchiveConfig,
    pub indexing: IndexingConfig,
    pub api: ApiConfig,
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
            upload: UploadConfig {
                max_files: env_value("RAIN_UPLOAD_MAX_FILES", defaults.upload.max_files)?,
                max_file_size: env_size(
                    "RAIN_UPLOAD_MAX_FILE_SIZE",
                    defaults.upload.max_file_size,
                )?,
                max_total_size: env_size(
                    "RAIN_UPLOAD_MAX_TOTAL_SIZE",
                    defaults.upload.max_total_size,
                )?,
                max_text_field_size: env_size(
                    "RAIN_UPLOAD_MAX_TEXT_FIELD_SIZE",
                    defaults.upload.max_text_field_size,
                )?,
                concurrent_processing_tasks: env_value(
                    "RAIN_UPLOAD_CONCURRENT_PROCESSING_TASKS",
                    defaults.upload.concurrent_processing_tasks,
                )?,
            },
            archive: ArchiveConfig {
                max_extracted_size: env_size(
                    "RAIN_ARCHIVE_MAX_EXTRACTED_SIZE",
                    defaults.archive.max_extracted_size,
                )?,
                max_entry_size: env_size(
                    "RAIN_ARCHIVE_MAX_ENTRY_SIZE",
                    defaults.archive.max_entry_size,
                )?,
                max_entries: env_value("RAIN_ARCHIVE_MAX_ENTRIES", defaults.archive.max_entries)?,
                max_path_depth: env_value(
                    "RAIN_ARCHIVE_MAX_PATH_DEPTH",
                    defaults.archive.max_path_depth,
                )?,
                max_recursion_depth: env_value(
                    "RAIN_ARCHIVE_MAX_RECURSION_DEPTH",
                    defaults.archive.max_recursion_depth,
                )?,
                max_output_path_chars: env_value(
                    "RAIN_ARCHIVE_MAX_OUTPUT_PATH_CHARS",
                    defaults.archive.max_output_path_chars,
                )?,
                max_compression_ratio: env_value(
                    "RAIN_ARCHIVE_MAX_COMPRESSION_RATIO",
                    defaults.archive.max_compression_ratio,
                )?,
            },
            indexing: IndexingConfig {
                max_line_size: env_size(
                    "RAIN_INDEXING_MAX_LINE_SIZE",
                    defaults.indexing.max_line_size,
                )?,
                chunk_lines: env_value("RAIN_INDEXING_CHUNK_LINES", defaults.indexing.chunk_lines)?,
                commit_lines: env_value(
                    "RAIN_INDEXING_COMMIT_LINES",
                    defaults.indexing.commit_lines,
                )?,
                line_offset_interval: env_value(
                    "RAIN_INDEXING_LINE_OFFSET_INTERVAL",
                    defaults.indexing.line_offset_interval,
                )?,
            },
            api: ApiConfig {
                file_preview_size: env_size(
                    "RAIN_API_FILE_PREVIEW_SIZE",
                    defaults.api.file_preview_size,
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
        positive!(self.upload.max_files, "RAIN_UPLOAD_MAX_FILES");
        positive!(self.upload.max_file_size, "RAIN_UPLOAD_MAX_FILE_SIZE");
        positive!(self.upload.max_total_size, "RAIN_UPLOAD_MAX_TOTAL_SIZE");
        positive!(
            self.upload.max_text_field_size,
            "RAIN_UPLOAD_MAX_TEXT_FIELD_SIZE"
        );
        positive!(
            self.upload.concurrent_processing_tasks,
            "RAIN_UPLOAD_CONCURRENT_PROCESSING_TASKS"
        );
        positive!(
            self.archive.max_extracted_size,
            "RAIN_ARCHIVE_MAX_EXTRACTED_SIZE"
        );
        positive!(self.archive.max_entry_size, "RAIN_ARCHIVE_MAX_ENTRY_SIZE");
        positive!(self.archive.max_entries, "RAIN_ARCHIVE_MAX_ENTRIES");
        positive!(self.archive.max_path_depth, "RAIN_ARCHIVE_MAX_PATH_DEPTH");
        positive!(
            self.archive.max_recursion_depth,
            "RAIN_ARCHIVE_MAX_RECURSION_DEPTH"
        );
        positive!(
            self.archive.max_output_path_chars,
            "RAIN_ARCHIVE_MAX_OUTPUT_PATH_CHARS"
        );
        positive!(
            self.archive.max_compression_ratio,
            "RAIN_ARCHIVE_MAX_COMPRESSION_RATIO"
        );
        positive!(self.indexing.max_line_size, "RAIN_INDEXING_MAX_LINE_SIZE");
        positive!(self.indexing.chunk_lines, "RAIN_INDEXING_CHUNK_LINES");
        positive!(self.indexing.commit_lines, "RAIN_INDEXING_COMMIT_LINES");
        positive!(
            self.indexing.line_offset_interval,
            "RAIN_INDEXING_LINE_OFFSET_INTERVAL"
        );
        positive!(self.api.file_preview_size, "RAIN_API_FILE_PREVIEW_SIZE");
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
        if self.archive.max_entry_size > self.archive.max_extracted_size {
            return Err(AppError::Config(
                "RAIN_ARCHIVE_MAX_ENTRY_SIZE must not exceed RAIN_ARCHIVE_MAX_EXTRACTED_SIZE"
                    .into(),
            ));
        }
        if self.upload.max_file_size > self.upload.max_total_size {
            return Err(AppError::Config(
                "RAIN_UPLOAD_MAX_FILE_SIZE must not exceed RAIN_UPLOAD_MAX_TOTAL_SIZE".into(),
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
        usize::try_from(self.indexing.max_line_size).map_err(|_| {
            AppError::Config(
                "RAIN_INDEXING_MAX_LINE_SIZE cannot be represented on this platform".into(),
            )
        })?;
        Ok(())
    }
}

fn dotenv_path_for_executable(executable: &std::path::Path) -> Option<PathBuf> {
    executable.parent().map(|directory| directory.join(".env"))
}

fn load_dotenv() {
    if let Ok(executable) = env::current_exe()
        && let Some(path) = dotenv_path_for_executable(&executable)
        && path.is_file()
    {
        dotenvy::from_path(path).ok();
        return;
    }

    dotenvy::dotenv().ok();
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
}

impl AppConfig {
    pub fn from_env() -> Result<Self, AppError> {
        load_dotenv();

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

        Ok(Self {
            host,
            port,
            database_url,
            data_root,
            log_dir,
            reset_db,
            retention_days,
            limits,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Mutex};

    use super::{AppLimits, dotenv_path_for_executable, parse_byte_size};

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
    fn defaults_preserve_existing_limits() {
        let limits = AppLimits::default();
        assert_eq!(limits.upload.max_file_size, 512 * 1024 * 1024);
        assert_eq!(limits.upload.max_total_size, 2 * 1024_u64.pow(3));
        assert_eq!(limits.archive.max_extracted_size, 500 * 1024 * 1024);
        assert_eq!(limits.archive.max_entry_size, 100 * 1024 * 1024);
        assert_eq!(limits.indexing.max_line_size, 1024 * 1024);
        assert_eq!(limits.api.file_preview_size, 64 * 1024);
    }

    #[test]
    fn validates_cross_field_limit_relationships() {
        let mut limits = AppLimits::default();
        limits.archive.max_entry_size = limits.archive.max_extracted_size + 1;
        assert!(
            limits
                .validate()
                .unwrap_err()
                .to_string()
                .contains("RAIN_ARCHIVE_MAX_ENTRY_SIZE")
        );

        let mut limits = AppLimits::default();
        limits.upload.max_file_size = limits.upload.max_total_size + 1;
        assert!(
            limits
                .validate()
                .unwrap_err()
                .to_string()
                .contains("RAIN_UPLOAD_MAX_FILE_SIZE")
        );

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
}
