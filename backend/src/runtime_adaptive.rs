//! Deterministic startup-time resource tuning for Rain runtimes.
//!
//! The resolver is deliberately independent from Tokio, SQL, and HTTP.  It
//! turns a resource snapshot plus persisted settings intent into concrete
//! values that can be used while constructing the process runtimes.

use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};

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
const MEMORY_FALLBACK_REASON: &str = "memory_detection_unavailable";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceSource {
    Os,
    Cgroup,
    ProcMeminfo,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub cpu_cores: usize,
    pub memory_limit_bytes: Option<u64>,
    pub cpu_source: ResourceSource,
    pub memory_source: ResourceSource,
    pub memory_fallback_reason: Option<String>,
    pub warnings: Vec<String>,
}

impl ResourceSnapshot {
    pub fn conservative() -> Self {
        Self {
            cpu_cores: 1,
            memory_limit_bytes: Some(512 * MIB),
            cpu_source: ResourceSource::Fallback,
            memory_source: ResourceSource::Fallback,
            memory_fallback_reason: Some(MEMORY_FALLBACK_REASON.into()),
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

        let memory = {
            #[cfg(target_os = "linux")]
            {
                select_memory_source(read_linux_memory_limit(), read_linux_meminfo(), None)
            }
            #[cfg(target_os = "windows")]
            {
                select_memory_source(None, None, read_windows_total_memory())
            }
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            {
                select_memory_source(None, None, None)
            }
        };
        snapshot.memory_limit_bytes = Some(memory.bytes);
        snapshot.memory_source = memory.source;
        snapshot.memory_fallback_reason = memory.fallback_reason.map(str::to_owned);

        #[cfg(target_os = "linux")]
        {
            if let Some(quota_cores) = read_linux_cpu_quota() {
                snapshot.cpu_cores = snapshot.cpu_cores.min(quota_cores).max(1);
                snapshot.cpu_source = ResourceSource::Cgroup;
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct MemoryDetection {
    bytes: u64,
    source: ResourceSource,
    fallback_reason: Option<&'static str>,
}

fn select_memory_source(
    cgroup: Option<u64>,
    proc_meminfo: Option<u64>,
    os: Option<u64>,
) -> MemoryDetection {
    if let Some(bytes) = cgroup.filter(|bytes| *bytes > 0) {
        return MemoryDetection {
            bytes,
            source: ResourceSource::Cgroup,
            fallback_reason: None,
        };
    }
    if let Some(bytes) = proc_meminfo.filter(|bytes| *bytes > 0) {
        return MemoryDetection {
            bytes,
            source: ResourceSource::ProcMeminfo,
            fallback_reason: None,
        };
    }
    if let Some(bytes) = os.filter(|bytes| *bytes > 0) {
        return MemoryDetection {
            bytes,
            source: ResourceSource::Os,
            fallback_reason: None,
        };
    }
    MemoryDetection {
        bytes: 512 * MIB,
        source: ResourceSource::Fallback,
        fallback_reason: Some(MEMORY_FALLBACK_REASON),
    }
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

    let decisions = vec![
        (
            "upload_concurrent_processing_tasks".into(),
            decision(processing_mode, processing as u64, "cpu_capacity"),
        ),
        (
            "search_tantivy_max_writers".into(),
            decision(
                writers_mode,
                writers as u64,
                "cpu_capacity_and_memory_budget",
            ),
        ),
        (
            "search_tantivy_writer_heap_size".into(),
            decision(heap_mode, heap, "memory_budget"),
        ),
    ];
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
    parse_linux_meminfo(&contents)
}

#[cfg(target_os = "linux")]
fn parse_linux_meminfo(contents: &str) -> Option<u64> {
    contents.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()? != "MemTotal:" {
            return None;
        }
        if parts.next_back()? != "kB" {
            return None;
        }
        let kib = parts.next()?.parse::<u64>().ok()?;
        if kib == 0 {
            return None;
        }
        kib.checked_mul(1024)
    })
}

#[cfg(target_os = "linux")]
fn read_linux_memory_limit() -> Option<u64> {
    let proc_cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    cgroup_memory_limit(&proc_cgroup, &mountinfo, |path| {
        std::fs::read_to_string(path).ok()
    })
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CgroupKind {
    V1,
    V2,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct CgroupMembership {
    kind: CgroupKind,
    path: PathBuf,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct CgroupMount {
    kind: CgroupKind,
    root: PathBuf,
    mount_point: PathBuf,
}

#[cfg(target_os = "linux")]
fn parse_cgroup_memberships(contents: &str) -> Vec<CgroupMembership> {
    contents
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, ':');
            let hierarchy = parts.next()?;
            let controllers = parts.next()?;
            let path = parts.next()?;
            let kind = if hierarchy == "0" && controllers.is_empty() {
                CgroupKind::V2
            } else if controllers
                .split(',')
                .any(|controller| controller == "memory")
            {
                CgroupKind::V1
            } else {
                return None;
            };
            Some(CgroupMembership {
                kind,
                path: PathBuf::from(path),
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn parse_cgroup_mounts(contents: &str) -> Vec<CgroupMount> {
    contents
        .lines()
        .filter_map(|line| {
            let (mount_fields, filesystem_fields) = line.split_once(" - ")?;
            let mount_fields = mount_fields.split_whitespace().collect::<Vec<_>>();
            if mount_fields.len() < 6 {
                return None;
            }
            let filesystem_fields = filesystem_fields.split_whitespace().collect::<Vec<_>>();
            if filesystem_fields.len() < 3 {
                return None;
            }
            let kind = match filesystem_fields[0] {
                "cgroup2" => CgroupKind::V2,
                "cgroup"
                    if filesystem_fields[2]
                        .split(',')
                        .any(|option| option == "memory") =>
                {
                    CgroupKind::V1
                }
                _ => return None,
            };
            Some(CgroupMount {
                kind,
                root: PathBuf::from(unescape_mountinfo_path(mount_fields[3])?),
                mount_point: PathBuf::from(unescape_mountinfo_path(mount_fields[4])?),
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn unescape_mountinfo_path(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let digits = &bytes[index + 1..index + 4];
            if digits.iter().all(|digit| (b'0'..=b'7').contains(digit)) {
                let decoded_byte =
                    (digits[0] - b'0') * 64 + (digits[1] - b'0') * 8 + (digits[2] - b'0');
                decoded.push(decoded_byte);
                index += 4;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(decoded).ok()
}

#[cfg(target_os = "linux")]
fn parse_cgroup_memory_value(value: &str) -> Option<u64> {
    let value = value.split_whitespace().next()?;
    if value == "max" {
        return None;
    }
    let bytes = value.parse::<u64>().ok()?;
    (bytes > 0 && bytes < 1 << 60).then_some(bytes)
}

#[cfg(target_os = "linux")]
fn cgroup_memory_limit<F>(proc_cgroup: &str, mountinfo: &str, read_file: F) -> Option<u64>
where
    F: Fn(&Path) -> Option<String>,
{
    let memberships = parse_cgroup_memberships(proc_cgroup);
    let mounts = parse_cgroup_mounts(mountinfo);
    let mut limit: Option<u64> = None;

    for mount in mounts {
        let Some(membership) = memberships
            .iter()
            .find(|membership| membership.kind == mount.kind)
        else {
            continue;
        };
        let Ok(relative_path) = membership.path.strip_prefix(&mount.root) else {
            continue;
        };
        let mut current_path = mount.mount_point.clone();
        current_path.push(relative_path);
        let file_name = match mount.kind {
            CgroupKind::V1 => "memory.limit_in_bytes",
            CgroupKind::V2 => "memory.max",
        };

        loop {
            if let Some(candidate) = read_file(&current_path.join(file_name))
                .and_then(|value| parse_cgroup_memory_value(&value))
            {
                limit = Some(limit.map_or(candidate, |current| current.min(candidate)));
            }
            if current_path == mount.mount_point || !current_path.pop() {
                break;
            }
        }
    }

    limit
}

#[cfg(target_os = "windows")]
fn read_windows_total_memory() -> Option<u64> {
    use std::mem::size_of;
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    let success = unsafe { GlobalMemoryStatusEx(&mut status) };
    (success != 0 && status.ullTotalPhys > 0).then_some(status.ullTotalPhys)
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

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_memtotal_as_bytes() {
        let contents = "MemTotal:       24511652 kB\nMemFree:         4289624 kB\n";

        assert_eq!(parse_linux_meminfo(contents), Some(24511652 * 1024));
        assert_eq!(parse_linux_meminfo("MemFree: 123 kB\n"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_invalid_meminfo_values() {
        for contents in [
            "MemTotal: not-a-number kB\n",
            "MemTotal: 12 MB\n",
            "MemTotal: 0 kB\n",
            "MemTotal: 18446744073709551615 kB\n",
        ] {
            assert_eq!(parse_linux_meminfo(contents), None, "{contents}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_v2_memory_limit_uses_current_and_ancestor_limits() {
        use std::{collections::HashMap, path::Path};

        let proc_cgroup = "0::/user.slice/user-1000.slice/session.scope\n";
        let mountinfo =
            "42 1 0:42 / /sys/fs/cgroup rw,nosuid,nodev,noexec,relatime - cgroup2 cgroup rw\n";
        let files = HashMap::from([
            (
                "/sys/fs/cgroup/user.slice/user-1000.slice/session.scope/memory.max",
                "max",
            ),
            (
                "/sys/fs/cgroup/user.slice/user-1000.slice/memory.max",
                "68719476736",
            ),
        ]);
        let read_file = |path: &Path| {
            files
                .get(path.to_str().expect("synthetic paths are utf-8"))
                .map(|value| (*value).to_owned())
        };

        assert_eq!(
            cgroup_memory_limit(proc_cgroup, mountinfo, read_file),
            Some(64 * GIB)
        );

        let files = HashMap::from([
            (
                "/sys/fs/cgroup/user.slice/user-1000.slice/session.scope/memory.max",
                "2147483648",
            ),
            (
                "/sys/fs/cgroup/user.slice/user-1000.slice/memory.max",
                "4294967296",
            ),
        ]);
        let read_file = |path: &Path| {
            files
                .get(path.to_str().expect("synthetic paths are utf-8"))
                .map(|value| (*value).to_owned())
        };

        assert_eq!(
            cgroup_memory_limit(proc_cgroup, mountinfo, read_file),
            Some(2 * GIB)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_v1_memory_controller_reads_memory_limit() {
        use std::{collections::HashMap, path::Path};

        let proc_cgroup = "5:memory:/docker/abc\n";
        let mountinfo = "43 1 0:43 / /sys/fs/cgroup/memory rw,nosuid,nodev,noexec,relatime - cgroup cgroup rw,memory\n";
        let files = HashMap::from([(
            "/sys/fs/cgroup/memory/docker/abc/memory.limit_in_bytes",
            "2147483648",
        )]);
        let read_file = |path: &Path| {
            files
                .get(path.to_str().expect("synthetic paths are utf-8"))
                .map(|value| (*value).to_owned())
        };

        assert_eq!(
            cgroup_memory_limit(proc_cgroup, mountinfo, read_file),
            Some(2 * GIB)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_memory_limit_decodes_escaped_mount_paths() {
        use std::{collections::HashMap, path::Path};

        let proc_cgroup = "0::/app\n";
        let mountinfo = "42 1 0:42 / /sys/fs/cgroup/my\\040cgroup rw,nosuid,nodev,noexec,relatime - cgroup2 cgroup rw\n";
        let files = HashMap::from([("/sys/fs/cgroup/my cgroup/app/memory.max", "2147483648")]);
        let read_file = |path: &Path| {
            files
                .get(path.to_str().expect("synthetic paths are utf-8"))
                .map(|value| (*value).to_owned())
        };

        assert_eq!(
            cgroup_memory_limit(proc_cgroup, mountinfo, read_file),
            Some(2 * GIB)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_memory_limit_ignores_unlimited_and_invalid_values() {
        use std::{collections::HashMap, path::Path};

        let proc_cgroup = "0::/user.slice/session.scope\n";
        let mountinfo =
            "42 1 0:42 / /sys/fs/cgroup rw,nosuid,nodev,noexec,relatime - cgroup2 cgroup rw\n";
        let files = HashMap::from([
            ("/sys/fs/cgroup/user.slice/session.scope/memory.max", "max"),
            ("/sys/fs/cgroup/user.slice/memory.max", "0"),
            ("/sys/fs/cgroup/memory.max", "not-a-number"),
        ]);
        let read_file = |path: &Path| {
            files
                .get(path.to_str().expect("synthetic paths are utf-8"))
                .map(|value| (*value).to_owned())
        };

        assert_eq!(cgroup_memory_limit(proc_cgroup, mountinfo, read_file), None);

        let proc_cgroup = "5:memory:/docker/abc\n";
        let mountinfo = "43 1 0:43 / /sys/fs/cgroup/memory rw,nosuid,nodev,noexec,relatime - cgroup cgroup rw,memory\n";
        let files = HashMap::from([(
            "/sys/fs/cgroup/memory/docker/abc/memory.limit_in_bytes",
            "9223372036854771712",
        )]);
        let read_file = |path: &Path| {
            files
                .get(path.to_str().expect("synthetic paths are utf-8"))
                .map(|value| (*value).to_owned())
        };

        assert_eq!(cgroup_memory_limit(proc_cgroup, mountinfo, read_file), None);
    }

    #[test]
    fn resource_source_serializes_proc_meminfo() {
        assert_eq!(
            serde_json::to_value(ResourceSource::ProcMeminfo).expect("source serializes"),
            serde_json::json!("proc_meminfo")
        );
    }

    #[test]
    fn memory_probe_prefers_container_host_and_os_sources_in_order() {
        assert_eq!(
            select_memory_source(Some(2 * GIB), Some(64 * GIB), Some(128 * GIB)),
            MemoryDetection {
                bytes: 2 * GIB,
                source: ResourceSource::Cgroup,
                fallback_reason: None,
            }
        );
        assert_eq!(
            select_memory_source(None, Some(64 * GIB), Some(128 * GIB)),
            MemoryDetection {
                bytes: 64 * GIB,
                source: ResourceSource::ProcMeminfo,
                fallback_reason: None,
            }
        );
        assert_eq!(
            select_memory_source(None, None, Some(128 * GIB)),
            MemoryDetection {
                bytes: 128 * GIB,
                source: ResourceSource::Os,
                fallback_reason: None,
            }
        );
    }

    #[test]
    fn memory_probe_fallback_exposes_reason() {
        assert_eq!(
            select_memory_source(None, None, None),
            MemoryDetection {
                bytes: 512 * MIB,
                source: ResourceSource::Fallback,
                fallback_reason: Some("memory_detection_unavailable"),
            }
        );
        assert_eq!(
            ResourceSnapshot::conservative()
                .memory_fallback_reason
                .as_deref(),
            Some("memory_detection_unavailable")
        );
    }

    #[test]
    fn auto_plan_scales_with_cpu_and_memory() {
        let resources = ResourceSnapshot {
            cpu_cores: 16,
            memory_limit_bytes: Some(64 * GIB),
            cpu_source: ResourceSource::Cgroup,
            memory_source: ResourceSource::Cgroup,
            memory_fallback_reason: None,
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
            memory_fallback_reason: None,
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
            memory_limit_bytes: Some(GIB),
            cpu_source: ResourceSource::Cgroup,
            memory_source: ResourceSource::Cgroup,
            memory_fallback_reason: None,
            warnings: Vec::new(),
        };
        let plan = resolve(&resources, &values(), &modes(ResourceMode::Auto), true);
        assert!(plan.estimated_bytes <= plan.adaptive_memory_target_bytes.unwrap());
        assert!(plan.tantivy_max_writers >= 1);
    }
}
