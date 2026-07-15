use std::sync::{Arc, Mutex};

use crate::{config::ArchiveConfig, error::AppError};

use super::path_policy::format_binary_size;

#[derive(Clone)]
pub struct ArchiveBudget {
    counters: Arc<Mutex<ArchiveCounters>>,
    pub(crate) config: ArchiveConfig,
}

impl Default for ArchiveBudget {
    fn default() -> Self {
        Self::new(ArchiveConfig::default())
    }
}

#[derive(Default)]
struct ArchiveCounters {
    entries: usize,
    extracted_bytes: u64,
}

impl ArchiveBudget {
    pub fn new(config: ArchiveConfig) -> Self {
        Self {
            counters: Arc::new(Mutex::new(ArchiveCounters::default())),
            config,
        }
    }

    pub(crate) fn reserve_entry(&self) -> Result<(), AppError> {
        let mut counters = self
            .counters
            .lock()
            .map_err(|_| AppError::BadRequest("archive budget lock poisoned".into()))?;
        counters.entries = counters
            .entries
            .checked_add(1)
            .ok_or_else(|| AppError::BadRequest("archive entry count overflow".into()))?;
        if counters.entries > self.config.max_entries {
            return Err(AppError::BadRequest(format!(
                "archive bundle has too many entries; max {}",
                self.config.max_entries
            )));
        }
        Ok(())
    }

    pub(crate) fn reserve_bytes(&self, size_bytes: u64) -> Result<(), AppError> {
        let mut counters = self
            .counters
            .lock()
            .map_err(|_| AppError::BadRequest("archive budget lock poisoned".into()))?;
        counters.extracted_bytes = counters
            .extracted_bytes
            .checked_add(size_bytes)
            .ok_or_else(|| AppError::BadRequest("archive extracted size overflow".into()))?;
        if counters.extracted_bytes > self.config.max_extracted_size {
            return Err(AppError::BadRequest(format!(
                "archive bundle exceeds configured extracted size; max bundle size {}",
                format_binary_size(self.config.max_extracted_size)
            )));
        }
        Ok(())
    }

    pub(crate) fn remaining_bytes(&self) -> Result<u64, AppError> {
        let counters = self
            .counters
            .lock()
            .map_err(|_| AppError::BadRequest("archive budget lock poisoned".into()))?;
        Ok(self
            .config
            .max_extracted_size
            .saturating_sub(counters.extracted_bytes))
    }
}
