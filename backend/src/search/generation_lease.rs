//! Process-local reader and cleanup coordination for immutable search generations.
//!
//! The registry deliberately does not persist reader counts. A process restart
//! drops every guard at once, while durable publication metadata and cleanup
//! claims continue to provide restart recovery.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

static SHARED_REGISTRY: OnceLock<GenerationLeaseRegistry> = OnceLock::new();

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct GenerationLeaseKey {
    pub(crate) bundle_id: String,
    pub(crate) generation: i64,
}

impl GenerationLeaseKey {
    pub(crate) fn new(bundle_id: &str, generation: i64) -> Self {
        Self {
            bundle_id: bundle_id.to_owned(),
            generation,
        }
    }
}

#[derive(Debug, Default)]
struct EntryState {
    readers: usize,
    cleanup_claimed: bool,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct GenerationLeaseRegistry {
    entries: Arc<Mutex<HashMap<GenerationLeaseKey, EntryState>>>,
}

impl GenerationLeaseRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn shared() -> Self {
        SHARED_REGISTRY
            .get_or_init(GenerationLeaseRegistry::new)
            .clone()
    }

    pub(crate) fn try_acquire(&self, bundle_id: &str, generation: i64) -> Option<GenerationLease> {
        let key = GenerationLeaseKey::new(bundle_id, generation);
        let mut entries = self
            .entries
            .lock()
            .expect("generation lease registry poisoned");
        let state = entries.entry(key.clone()).or_default();
        if state.cleanup_claimed {
            return None;
        }
        state.readers = state.readers.saturating_add(1);
        Some(GenerationLease {
            registry: self.clone(),
            key,
            released: false,
        })
    }

    pub(crate) fn try_claim_cleanup(
        &self,
        bundle_id: &str,
        generation: i64,
    ) -> Option<GenerationCleanupClaim> {
        let key = GenerationLeaseKey::new(bundle_id, generation);
        let mut entries = self
            .entries
            .lock()
            .expect("generation lease registry poisoned");
        let state = entries.entry(key.clone()).or_default();
        if state.readers != 0 || state.cleanup_claimed {
            return None;
        }
        state.cleanup_claimed = true;
        Some(GenerationCleanupClaim {
            registry: self.clone(),
            key,
            released: false,
        })
    }

    fn release_reader(&self, key: &GenerationLeaseKey) {
        let mut entries = self
            .entries
            .lock()
            .expect("generation lease registry poisoned");
        let remove = if let Some(state) = entries.get_mut(key) {
            state.readers = state.readers.saturating_sub(1);
            state.readers == 0 && !state.cleanup_claimed
        } else {
            false
        };
        if remove {
            entries.remove(key);
        }
    }

    fn release_cleanup_claim(&self, key: &GenerationLeaseKey) {
        let mut entries = self
            .entries
            .lock()
            .expect("generation lease registry poisoned");
        let remove = if let Some(state) = entries.get_mut(key) {
            state.cleanup_claimed = false;
            state.readers == 0
        } else {
            false
        };
        if remove {
            entries.remove(key);
        }
    }
}

#[derive(Debug)]
pub(crate) struct GenerationLease {
    registry: GenerationLeaseRegistry,
    key: GenerationLeaseKey,
    released: bool,
}

impl Drop for GenerationLease {
    fn drop(&mut self) {
        if !self.released {
            self.released = true;
            self.registry.release_reader(&self.key);
        }
    }
}

#[derive(Debug)]
pub(crate) struct GenerationCleanupClaim {
    registry: GenerationLeaseRegistry,
    key: GenerationLeaseKey,
    released: bool,
}

impl Drop for GenerationCleanupClaim {
    fn drop(&mut self) {
        if !self.released {
            self.released = true;
            self.registry.release_cleanup_claim(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::generation_lease::GenerationLeaseRegistry;

    #[test]
    fn concurrent_readers_block_cleanup_until_the_last_reader_releases() {
        let registry = GenerationLeaseRegistry::new();
        let first = registry.try_acquire("bundle", 7).expect("first reader");
        let second = registry.try_acquire("bundle", 7).expect("second reader");

        assert!(registry.try_claim_cleanup("bundle", 7).is_none());
        drop(first);
        assert!(registry.try_claim_cleanup("bundle", 7).is_none());
        drop(second);

        let cleanup = registry
            .try_claim_cleanup("bundle", 7)
            .expect("cleanup claim after readers release");
        assert!(registry.try_acquire("bundle", 7).is_none());
        drop(cleanup);
        assert!(registry.try_acquire("bundle", 7).is_some());
    }

    #[test]
    fn dropping_reader_releases_state_without_async_work() {
        let registry = GenerationLeaseRegistry::new();
        let reader = registry.try_acquire("bundle", 9).expect("reader");
        drop(reader);

        assert!(registry.try_claim_cleanup("bundle", 9).is_some());
    }

    #[test]
    fn parallel_reader_acquisitions_are_counted_atomically() {
        let registry = GenerationLeaseRegistry::new();
        let readers = std::thread::scope(|scope| {
            (0..8)
                .map(|_| {
                    let registry = registry.clone();
                    scope.spawn(move || registry.try_acquire("bundle", 11).unwrap())
                })
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });

        assert!(registry.try_claim_cleanup("bundle", 11).is_none());
        drop(readers);
        assert!(registry.try_claim_cleanup("bundle", 11).is_some());
    }
}
