use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use crate::{error::AppError, search::generation_lease::GenerationLeaseKey};

use super::writer::{CommittedBundleIndex, open_committed};

/// Keep the cache bounded because every entry owns Tantivy's mmap directory
/// and reader. The value is intentionally internal: this is a process-local
/// resource optimization, not a deployment setting.
const DEFAULT_CAPACITY: usize = 64;

type LoadResult = Result<Arc<CommittedBundleIndex>, String>;

#[derive(Clone, Hash, PartialEq, Eq)]
struct ReaderCacheKey {
    generation: GenerationLeaseKey,
    artifact_path: PathBuf,
}

#[derive(Default)]
struct CacheState {
    entries: HashMap<ReaderCacheKey, Arc<CommittedBundleIndex>>,
    order: VecDeque<ReaderCacheKey>,
    loading: HashMap<ReaderCacheKey, Arc<OnceLock<LoadResult>>>,
}

/// Process-local cache for immutable Tantivy generations, scoped by the
/// canonical artifact path as well as the publication generation.
///
/// A generation is immutable after publication, so its reader never needs a
/// reload. Callers must hold a generation lease while using the returned Arc;
/// cleanup invalidates all entries for that generation after claiming it and
/// before removing its files. The path namespace prevents benchmark/test
/// instances that reuse bundle IDs and generation numbers from sharing a
/// reader opened from another data root.
pub(crate) struct GenerationReaderCache {
    capacity: usize,
    state: Mutex<CacheState>,
}

impl Default for GenerationReaderCache {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl GenerationReaderCache {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(CacheState::default()),
        }
    }

    pub(crate) fn get_or_open(
        &self,
        key: GenerationLeaseKey,
        path: impl AsRef<Path>,
    ) -> Result<Arc<CommittedBundleIndex>, AppError> {
        if self.capacity == 0 {
            return open_committed(path).map(Arc::new);
        }

        let path = path.as_ref().to_path_buf();
        let key = ReaderCacheKey {
            generation: key,
            artifact_path: std::fs::canonicalize(&path).map_err(AppError::Io)?,
        };
        let loader = {
            let mut state = self.state.lock().expect("Tantivy reader cache poisoned");
            if let Some(entry) = state.entries.get(&key).cloned() {
                touch(&mut state.order, &key);
                return Ok(entry);
            }
            state
                .loading
                .entry(key.clone())
                .or_insert_with(|| Arc::new(OnceLock::new()))
                .clone()
        };

        let result = loader.get_or_init(|| {
            open_committed(&path)
                .map(Arc::new)
                .map_err(|error| error.to_string())
        });
        let result = match result {
            Ok(entry) => Ok(entry.clone()),
            Err(error) => Err(AppError::Config(error.clone())),
        };

        let mut state = self.state.lock().expect("Tantivy reader cache poisoned");
        state.loading.remove(&key);
        if let Ok(entry) = &result {
            if let Some(existing) = state.entries.get(&key).cloned() {
                touch(&mut state.order, &key);
                return Ok(existing);
            }
            state.entries.insert(key.clone(), entry.clone());
            touch(&mut state.order, &key);
            while state.entries.len() > self.capacity {
                let Some(evicted) = state.order.pop_front() else {
                    break;
                };
                state.entries.remove(&evicted);
            }
        }
        result
    }

    pub(crate) fn invalidate(&self, key: &GenerationLeaseKey) {
        let mut state = self.state.lock().expect("Tantivy reader cache poisoned");
        state
            .entries
            .retain(|candidate, _| candidate.generation != *key);
        state.order.retain(|candidate| candidate.generation != *key);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.state
            .lock()
            .expect("Tantivy reader cache poisoned")
            .entries
            .len()
    }
}

fn touch(order: &mut VecDeque<ReaderCacheKey>, key: &ReaderCacheKey) {
    order.retain(|candidate| candidate != key);
    order.push_back(key.clone());
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use super::GenerationReaderCache;
    use crate::search::generation_lease::GenerationLeaseKey;
    use crate::search::tantivy::writer::{BundleIndexWriter, IndexedChunk};

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rain-tantivy-reader-cache-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn reuses_a_reader_for_the_same_generation_and_reopens_after_invalidation() {
        let path = temp_path("reuse");
        let writer = BundleIndexWriter::create(&path, 16 * 1024 * 1024).unwrap();
        drop(writer.commit().unwrap());

        let cache = GenerationReaderCache::new(4);
        let key = GenerationLeaseKey::new("bundle", 1);
        let first = cache.get_or_open(key.clone(), &path).unwrap();
        let second = cache.get_or_open(key.clone(), &path).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.len(), 1);

        cache.invalidate(&key);
        let reopened = cache.get_or_open(key, &path).unwrap();
        assert!(!Arc::ptr_eq(&first, &reopened));

        drop(first);
        drop(second);
        drop(reopened);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn evicts_the_least_recently_used_generation() {
        let path_a = temp_path("a");
        let path_b = temp_path("b");
        let path_c = temp_path("c");
        for path in [&path_a, &path_b, &path_c] {
            let writer = BundleIndexWriter::create(path, 16 * 1024 * 1024).unwrap();
            drop(writer.commit().unwrap());
        }

        let cache = GenerationReaderCache::new(2);
        let key_a = GenerationLeaseKey::new("bundle", 1);
        let key_b = GenerationLeaseKey::new("bundle", 2);
        let key_c = GenerationLeaseKey::new("bundle", 3);
        let _a = cache.get_or_open(key_a, &path_a).unwrap();
        let _b = cache.get_or_open(key_b, &path_b).unwrap();
        let _c = cache.get_or_open(key_c, &path_c).unwrap();
        assert_eq!(cache.len(), 2);

        for path in [&path_a, &path_b, &path_c] {
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    fn does_not_reuse_a_generation_reader_across_artifact_paths() {
        let path_a = temp_path("iteration-a");
        let path_b = temp_path("iteration-b");
        let mut writer_a = BundleIndexWriter::create(&path_a, 16 * 1024 * 1024).unwrap();
        writer_a
            .add_chunk(&IndexedChunk {
                file_id: 1,
                chunk_index: 0,
                line_start: Some(0),
                line_end: Some(0),
                event_time_start_ms: None,
                event_time_end_ms: None,
                timeline: None,
                content: "iteration-a".into(),
                path: "/a.log".into(),
            })
            .unwrap();
        drop(writer_a.commit().unwrap());
        let writer_b = BundleIndexWriter::create(&path_b, 16 * 1024 * 1024).unwrap();
        drop(writer_b.commit().unwrap());

        let cache = GenerationReaderCache::new(4);
        let key = GenerationLeaseKey::new("bench-0", 1);
        let first = cache.get_or_open(key.clone(), &path_a).unwrap();
        let second = cache.get_or_open(key, &path_b).unwrap();

        assert_eq!(first.document_count, 1);
        assert_eq!(second.document_count, 0);
        assert!(!Arc::ptr_eq(&first, &second));

        drop(first);
        drop(second);
        std::fs::remove_dir_all(path_a).unwrap();
        std::fs::remove_dir_all(path_b).unwrap();
    }
}
