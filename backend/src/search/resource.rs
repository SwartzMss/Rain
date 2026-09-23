use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::AppError;

#[derive(Clone)]
pub struct SearchResourceBudget {
    writer_permits: Arc<Semaphore>,
    writer_heap_size_bytes: usize,
    queued_writers: Arc<AtomicUsize>,
    active_writers: Arc<AtomicUsize>,
}

pub struct SearchResourcePermit {
    _permit: OwnedSemaphorePermit,
    active_writers: Arc<AtomicUsize>,
    queue_wait: Duration,
}

struct QueueGuard {
    counter: Arc<AtomicUsize>,
    armed: bool,
}

impl QueueGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self {
            counter,
            armed: true,
        }
    }

    fn release(&mut self) {
        if self.armed {
            self.counter.fetch_sub(1, Ordering::AcqRel);
            self.armed = false;
        }
    }
}

impl Drop for QueueGuard {
    fn drop(&mut self) {
        self.release();
    }
}

impl SearchResourceBudget {
    pub fn new(max_writers: usize, writer_heap_size_bytes: u64) -> Result<Self, AppError> {
        if max_writers == 0 {
            return Err(AppError::Config(
                "RAIN_SEARCH_TANTIVY_MAX_WRITERS must be positive".into(),
            ));
        }
        let writer_heap_size_bytes = usize::try_from(writer_heap_size_bytes).map_err(|_| {
            AppError::Config(
                "RAIN_SEARCH_TANTIVY_WRITER_HEAP is too large for this platform".into(),
            )
        })?;
        if writer_heap_size_bytes == 0 {
            return Err(AppError::Config(
                "RAIN_SEARCH_TANTIVY_WRITER_HEAP must be positive".into(),
            ));
        }
        Ok(Self {
            writer_permits: Arc::new(Semaphore::new(max_writers)),
            writer_heap_size_bytes,
            queued_writers: Arc::new(AtomicUsize::new(0)),
            active_writers: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub async fn acquire(&self) -> Result<SearchResourcePermit, AppError> {
        let started = Instant::now();
        let mut queued = QueueGuard::new(self.queued_writers.clone());
        let permit = self
            .writer_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AppError::Conflict("Tantivy writer admission is shutting down".into()))?;
        queued.release();
        self.active_writers.fetch_add(1, Ordering::AcqRel);
        Ok(SearchResourcePermit {
            _permit: permit,
            active_writers: self.active_writers.clone(),
            queue_wait: started.elapsed(),
        })
    }

    pub fn writer_heap_size_bytes(&self) -> usize {
        self.writer_heap_size_bytes
    }

    pub fn available_writers(&self) -> usize {
        self.writer_permits.available_permits()
    }

    pub fn queued_writers(&self) -> usize {
        self.queued_writers.load(Ordering::Acquire)
    }

    pub fn active_writers(&self) -> usize {
        self.active_writers.load(Ordering::Acquire)
    }
}

impl SearchResourcePermit {
    pub fn queue_wait(&self) -> Duration {
        self.queue_wait
    }
}

impl Drop for SearchResourcePermit {
    fn drop(&mut self) {
        self.active_writers.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::SearchResourceBudget;

    #[tokio::test]
    async fn second_writer_waits_until_the_first_is_dropped() {
        let budget = SearchResourceBudget::new(1, 64).unwrap();
        let first = budget.acquire().await.unwrap();
        assert_eq!(budget.active_writers(), 1);
        let blocked = tokio::time::timeout(Duration::from_millis(25), budget.acquire()).await;
        assert!(blocked.is_err());
        drop(first);
        let second = tokio::time::timeout(Duration::from_secs(1), budget.acquire())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(budget.active_writers(), 1);
        drop(second);
        assert_eq!(budget.active_writers(), 0);
    }

    #[tokio::test]
    async fn cancelled_waiter_does_not_leave_queue_or_active_counts() {
        let budget = SearchResourceBudget::new(1, 64).unwrap();
        let first = budget.acquire().await.unwrap();
        let waiting = budget.clone();
        let task = tokio::spawn(async move { waiting.acquire().await });
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(budget.queued_writers(), 1);
        task.abort();
        let _ = task.await;
        assert_eq!(budget.queued_writers(), 0);
        assert_eq!(budget.active_writers(), 1);
        drop(first);
        assert_eq!(budget.active_writers(), 0);
    }
}
