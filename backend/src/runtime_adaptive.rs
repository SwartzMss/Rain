//! Deterministic startup-time resource tuning for Rain runtimes.
//!
//! The resolver is deliberately independent from Tokio, SQL, and HTTP.  It
//! turns a resource snapshot plus persisted settings intent into concrete
//! values that can be used while constructing the process runtimes.

use serde::{Deserialize, Serialize};

use crate::{
    config::AppLimits,
    settings::{ResourceMode, ResourceModes, SettingsValues},
};

const MIB: u64 = 1024 * 1024;
#[cfg(test)]
const GIB: u64 = 1024 * MIB;

const MIN_WRITERS: usize = 1;
const MAX_WRITERS: usize = 4;
const MIN_PROCESSING: usize = 1;
const MAX_PROCESSING: usize = 8;
const MIN_QUERIES: usize = 1;
const MAX_QUERIES: usize = 16;
const MIN_HEAP: u64 = 16 * MIB;
const MAX_HEAP: u64 = 256 * MIB;
const RESERVED_PER_TASK: u64 = 32 * MIB;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceSource {
    Os,
    Cgroup,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub cpu_cores: usize,
    pub memory_limit_bytes: Option<u64>,
    pub cpu_source: ResourceSource,
    pub memory_source: ResourceSource,
    pub warnings: Vec<String>,
}

impl ResourceSnapshot {
    pub fn conservative() -> Self {
        Self {
            cpu_cores: 1,
            memory_limit_bytes: Some(512 * MIB),
            cpu_source: ResourceSource::Fallback,
            memory_source: ResourceSource::Fallback,
            warnings: vec!["resource_probe_fallback".into()],
        }
    }

    pub fn probe() -> Self {
        let mut snapshot = Self::conservative();
        snapshot.warnings.clear();
        if let Ok(parallelism) = std::thread::available_parallelism() {
            snapshot.cpu_cores = parallelism.get().max(1);
            snapshot.cpu_source = ResourceSource::Os;
        }

        #[cfg(target_os = "linux")]
        {
            if let Some(quota_cores) = read_linux_cpu_quota() {
                snapshot.cpu_cores = snapshot.cpu_cores.min(quota_cores).max(1);
                snapshot.cpu_source = ResourceSource::Cgroup;
            }
            if let Some(memory) = read_linux_memory_limit() {
                snapshot.memory_limit_bytes = Some(memory);
                snapshot.memory_source = ResourceSource::Cgroup;
            } else if let Some(memory) = read_linux_meminfo() {
                snapshot.memory_limit_bytes = Some(memory);
                snapshot.memory_source = ResourceSource::Os;
            }
        }
        if snapshot.cpu_source == ResourceSource::Fallback {
            snapshot.warnings.push("cpu_probe_fallback".into());
        }
        if snapshot.memory_source == ResourceSource::Fallback {
            snapshot.warnings.push("memory_probe_fallback".into());
        }
        snapshot
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDecision {
    pub value: u64,
    pub mode: ResourceMode,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePlan {
    pub resources: ResourceSnapshot,
    pub upload_processing_tasks: usize,
    pub tantivy_max_writers: usize,
    pub tantivy_writer_heap_size: u64,
    pub tantivy_max_concurrent_queries: usize,
    pub estimated_bytes: u64,
    /// Heuristic planning target, not an RSS or allocation hard limit.
    pub adaptive_memory_target_bytes: Option<u64>,
    pub warnings: Vec<String>,
    pub decisions: Vec<(String, RuntimeDecision)>,
}

impl RuntimePlan {
    pub fn apply_to_limits(&self, limits: &mut AppLimits) {
        limits.upload.concurrent_processing_tasks = self.upload_processing_tasks;
        limits.search.tantivy_max_writers = self.tantivy_max_writers;
        limits.search.tantivy_writer_heap_size = self.tantivy_writer_heap_size;
    }
}

pub fn resolve(
    resources: &ResourceSnapshot,
    configured: &SettingsValues,
    modes: &ResourceModes,
    tantivy_enabled: bool,
) -> RuntimePlan {
    let cores = resources.cpu_cores.max(1);
    let memory = resources.memory_limit_bytes;
    let auto_processing = (cores.saturating_add(1) / 2).clamp(MIN_PROCESSING, MAX_PROCESSING);
    let auto_writers = (cores / 4).clamp(MIN_WRITERS, MAX_WRITERS);
    let auto_heap = memory
        .map(|bytes| ((bytes / 64) / (16 * MIB) * (16 * MIB)).clamp(MIN_HEAP, MAX_HEAP))
        .unwrap_or(MIN_HEAP);
    let auto_queries = cores.clamp(MIN_QUERIES, MAX_QUERIES);

    let processing_mode = mode_for(modes, "upload_concurrent_processing_tasks");
    let writers_mode = mode_for(modes, "search_tantivy_max_writers");
    let heap_mode = mode_for(modes, "search_tantivy_writer_heap_size");
    let mut processing = choose(
        processing_mode,
        configured.upload_concurrent_processing_tasks,
        auto_processing,
    );
    let mut writers = choose(
        writers_mode,
        configured.search_tantivy_max_writers,
        auto_writers,
    );
    let mut heap = choose_bytes(
        heap_mode,
        configured.search_tantivy_writer_heap_size,
        auto_heap,
    );
    let mut queries = if tantivy_enabled { auto_queries } else { 1 };
    let mut warnings = resources.warnings.clone();
    warnings.push("adaptive_memory_target_is_heuristic".into());

    // This target reserves a heuristic quarter of the detected memory for the
    // adaptive runtime. It is not a hard RSS or allocation limit.
    let adaptive_memory_target_bytes = memory.map(|bytes| bytes / 4);
    let estimated = |p: usize, w: usize, h: u64, q: usize| {
        (p as u64)
            .saturating_mul(RESERVED_PER_TASK)
            .saturating_add((w as u64).saturating_mul(h.saturating_add(RESERVED_PER_TASK)))
            .saturating_add((q as u64).saturating_mul(RESERVED_PER_TASK))
    };
    let estimate_for_runtime = |p: usize, w: usize, h: u64, q: usize| {
        if tantivy_enabled {
            estimated(p, w, h, q)
        } else {
            estimated(p, 0, 0, 0)
        }
    };

    if let Some(target_bytes) = adaptive_memory_target_bytes {
        while estimate_for_runtime(processing, writers, heap, queries) > target_bytes {
            if heap_mode == ResourceMode::Auto && heap > MIN_HEAP {
                heap = heap.saturating_sub(16 * MIB).max(MIN_HEAP);
            } else if writers_mode == ResourceMode::Auto && writers > 1 {
                writers -= 1;
            } else if processing_mode == ResourceMode::Auto && processing > 1 {
                processing -= 1;
            } else if tantivy_enabled && queries > 1 {
                queries -= 1;
            } else {
                warnings.push("estimated_budget_exceeded".into());
                break;
            }
        }
    }
    if !tantivy_enabled {
        writers = configured.search_tantivy_max_writers;
        heap = configured.search_tantivy_writer_heap_size;
    }

    let mut decisions = Vec::new();
    decisions.push((
        "upload_concurrent_processing_tasks".into(),
        decision(processing_mode, processing as u64, "cpu_capacity"),
    ));
    decisions.push((
        "search_tantivy_max_writers".into(),
        decision(
            writers_mode,
            writers as u64,
            "cpu_capacity_and_memory_budget",
        ),
    ));
    decisions.push((
        "search_tantivy_writer_heap_size".into(),
        decision(heap_mode, heap, "memory_budget"),
    ));
    RuntimePlan {
        resources: resources.clone(),
        upload_processing_tasks: processing,
        tantivy_max_writers: writers,
        tantivy_writer_heap_size: heap,
        tantivy_max_concurrent_queries: queries,
        estimated_bytes: estimate_for_runtime(processing, writers, heap, queries),
        adaptive_memory_target_bytes,
        warnings,
        decisions,
    }
}

fn mode_for(modes: &ResourceModes, key: &str) -> ResourceMode {
    modes.get(key).copied().unwrap_or(ResourceMode::Manual)
}

fn choose(mode: ResourceMode, configured: usize, auto: usize) -> usize {
    if mode == ResourceMode::Auto {
        auto
    } else {
        configured.max(1)
    }
}

fn choose_bytes(mode: ResourceMode, configured: u64, auto: u64) -> u64 {
    if mode == ResourceMode::Auto {
        auto
    } else {
        configured.max(1)
    }
}

fn decision(mode: ResourceMode, value: u64, reason: &str) -> RuntimeDecision {
    RuntimeDecision {
        value,
        mode,
        reason: if mode == ResourceMode::Auto {
            reason.into()
        } else {
            "configured_value".into()
        },
    }
}

#[cfg(target_os = "linux")]
fn read_linux_meminfo() -> Option<u64> {
    let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
    contents.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()? != "MemTotal:" {
            return None;
        }
        parts.next()?.parse::<u64>().ok()?.checked_mul(1024)
    })
}

#[cfg(target_os = "linux")]
fn read_linux_memory_limit() -> Option<u64> {
    for path in [
        "/sys/fs/cgroup/memory.max",
        "/sys/fs/cgroup/memory/memory.limit_in_bytes",
    ] {
        let Ok(value) = std::fs::read_to_string(path) else {
            continue;
        };
        let value = value.trim().to_owned();
        if value == "max" {
            continue;
        }
        let Ok(bytes) = value.parse::<u64>() else {
            continue;
        };
        if bytes > 0 && bytes < 1 << 60 {
            return Some(bytes);
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn read_linux_cpu_quota() -> Option<usize> {
    for path in [
        "/sys/fs/cgroup/cpu.max",
        "/sys/fs/cgroup/cpu/cpu.cfs_quota_us",
    ] {
        let Ok(value) = std::fs::read_to_string(path) else {
            continue;
        };
        if path.ends_with("cpu.max") {
            let mut parts = value.split_whitespace();
            let Some(quota) = parts.next() else { continue };
            let Some(period) = parts.next().and_then(|value| value.parse::<u64>().ok()) else {
                continue;
            };
            if quota == "max" {
                continue;
            }
            let Ok(quota) = quota.parse::<u64>() else {
                continue;
            };
            return Some((quota / period.max(1)).max(1) as usize);
        }
        let Ok(quota) = value.trim().parse::<i64>() else {
            continue;
        };
        if quota > 0 {
            let Ok(period) = std::fs::read_to_string("/sys/fs/cgroup/cpu/cpu.cfs_period_us")
                .and_then(|value| {
                    value.trim().parse::<u64>().map_err(|error| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, error)
                    })
                })
            else {
                continue;
            };
            return Some((quota as u64 / period.max(1)).max(1) as usize);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::AuthConfig, settings::SettingsValues};

    fn values() -> SettingsValues {
        SettingsValues::from_config(&AppLimits::default(), &AuthConfig::default())
    }

    fn modes(mode: ResourceMode) -> ResourceModes {
        [
            "upload_concurrent_processing_tasks",
            "search_tantivy_max_writers",
            "search_tantivy_writer_heap_size",
        ]
        .into_iter()
        .map(|key| (key.to_owned(), mode))
        .collect()
    }

    #[test]
    fn auto_plan_scales_with_cpu_and_memory() {
        let resources = ResourceSnapshot {
            cpu_cores: 16,
            memory_limit_bytes: Some(64 * GIB),
            cpu_source: ResourceSource::Cgroup,
            memory_source: ResourceSource::Cgroup,
            warnings: Vec::new(),
        };
        let plan = resolve(&resources, &values(), &modes(ResourceMode::Auto), true);
        assert_eq!(plan.upload_processing_tasks, 8);
        assert_eq!(plan.tantivy_max_writers, 4);
        assert_eq!(plan.tantivy_writer_heap_size, 256 * MIB);
        assert_eq!(plan.tantivy_max_concurrent_queries, 16);
        assert!(
            !plan
                .decisions
                .iter()
                .any(|(key, _)| key == "search_tantivy_max_concurrent_queries")
        );
        assert!(
            plan.warnings
                .iter()
                .any(|warning| warning == "adaptive_memory_target_is_heuristic")
        );
    }

    #[test]
    fn manual_values_are_not_changed_by_budget_resolution() {
        let resources = ResourceSnapshot {
            cpu_cores: 16,
            memory_limit_bytes: Some(512 * MIB),
            cpu_source: ResourceSource::Cgroup,
            memory_source: ResourceSource::Cgroup,
            warnings: Vec::new(),
        };
        let mut configured = values();
        configured.search_tantivy_max_writers = 3;
        configured.search_tantivy_writer_heap_size = 64 * MIB;
        let modes = modes(ResourceMode::Manual);
        let plan = resolve(&resources, &configured, &modes, true);
        assert_eq!(plan.tantivy_max_writers, 3);
        assert_eq!(plan.tantivy_writer_heap_size, 64 * MIB);
    }

    #[test]
    fn low_memory_auto_plan_degrades_before_reporting_over_budget() {
        let resources = ResourceSnapshot {
            cpu_cores: 16,
            memory_limit_bytes: Some(1 * GIB),
            cpu_source: ResourceSource::Cgroup,
            memory_source: ResourceSource::Cgroup,
            warnings: Vec::new(),
        };
        let plan = resolve(&resources, &values(), &modes(ResourceMode::Auto), true);
        assert!(plan.estimated_bytes <= plan.adaptive_memory_target_bytes.unwrap());
        assert!(plan.tantivy_max_writers >= 1);
    }
}
