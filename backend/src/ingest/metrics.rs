//! Per-operation summaries, with no process-global counters or per-line timers.
use std::{
    future::Future,
    time::{Duration, Instant},
};

pub(crate) fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

/// A dropped future records cancellation rather than a false success.
pub(crate) struct PhaseTimer {
    phase: &'static str,
    started: Instant,
    outcome: &'static str,
}

impl PhaseTimer {
    pub(crate) fn new(phase: &'static str) -> Self {
        Self {
            phase,
            started: Instant::now(),
            outcome: "cancelled",
        }
    }
}

impl Drop for PhaseTimer {
    fn drop(&mut self) {
        tracing::info!(
            metric = "operation_phase",
            phase = self.phase,
            outcome = self.outcome,
            elapsed_us = micros(self.started.elapsed()),
            "operation phase completed"
        );
    }
}

pub(crate) async fn measure<T, E>(
    phase: &'static str,
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let mut timer = PhaseTimer::new(phase);
    let result = future.await;
    timer.outcome = if result.is_ok() { "success" } else { "error" };
    result
}

pub(crate) struct FileIndexMetrics<'a> {
    bundle_id: &'a str,
    file_id: i64,
    started: Instant,
    checkpoint: Instant,
    writing: bool,
    read_parse: Duration,
    write_wait: Duration,
    pub(crate) source_bytes: u64,
    pub(crate) source_lines: u64,
    pub(crate) indexed_bytes: u64,
    pub(crate) committed_chunks: u64,
    pub(crate) committed_batches: u64,
    pub(crate) outcome: &'static str,
}

impl<'a> FileIndexMetrics<'a> {
    pub(crate) fn new(bundle_id: &'a str, file_id: i64) -> Self {
        let now = Instant::now();
        Self {
            bundle_id,
            file_id,
            started: now,
            checkpoint: now,
            writing: false,
            read_parse: Duration::ZERO,
            write_wait: Duration::ZERO,
            source_bytes: 0,
            source_lines: 0,
            indexed_bytes: 0,
            committed_chunks: 0,
            committed_batches: 0,
            outcome: "cancelled",
        }
    }

    pub(crate) fn set_writing(&mut self, writing: bool) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.checkpoint);
        if self.writing {
            self.write_wait += elapsed;
        } else {
            self.read_parse += elapsed;
        }
        self.checkpoint = now;
        self.writing = writing;
    }
}

impl Drop for FileIndexMetrics<'_> {
    fn drop(&mut self) {
        self.set_writing(self.writing);
        tracing::info!(
            metric = "log_index_file",
            bundle_id = self.bundle_id,
            file_id = self.file_id,
            outcome = self.outcome,
            source_bytes = self.source_bytes,
            source_lines = self.source_lines,
            indexed_bytes = self.indexed_bytes,
            committed_chunks = self.committed_chunks,
            committed_batches = self.committed_batches,
            read_parse_us = micros(self.read_parse),
            write_wait_us = micros(self.write_wait),
            elapsed_us = micros(self.started.elapsed()),
            "log file indexing completed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::{
        Event, Subscriber,
        field::{Field, Visit},
        instrument::WithSubscriber,
    };
    use tracing_subscriber::{Layer, layer::Context, prelude::*};

    #[derive(Clone, Default)]
    struct Outcomes(Arc<Mutex<Vec<String>>>);

    impl Visit for Outcomes {
        fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "outcome" {
                self.0.lock().unwrap().push(value.into());
            }
        }
    }
    impl<S: Subscriber> Layer<S> for Outcomes {
        fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
            event.record(&mut self.clone());
        }
    }

    #[tokio::test]
    async fn cancelled_phase_is_not_a_success() {
        let outcomes = Outcomes::default();
        async {
            let mut future = Box::pin(measure(
                "cancel-test",
                std::future::pending::<Result<(), ()>>(),
            ));
            assert!(futures_util::poll!(future.as_mut()).is_pending());
            drop(future);
        }
        .with_subscriber(tracing_subscriber::registry().with(outcomes.clone()))
        .await;
        assert_eq!(*outcomes.0.lock().unwrap(), ["cancelled"]);
    }

    #[tokio::test]
    async fn phase_preserves_result_and_records_exactly_one_terminal_outcome() {
        let outcomes = Outcomes::default();
        async {
            assert_eq!(
                measure("success-test", async { Ok::<_, &str>(42) }).await,
                Ok(42)
            );
            assert_eq!(
                measure("failure-test", async { Err::<(), _>("failed") }).await,
                Err("failed")
            );
        }
        .with_subscriber(tracing_subscriber::registry().with(outcomes.clone()))
        .await;
        assert_eq!(*outcomes.0.lock().unwrap(), ["success", "error"]);
    }

    async fn file_fixture() -> (std::path::PathBuf, sqlx::SqlitePool, i64) {
        let root = std::env::temp_dir().join(format!("rain-index-metric-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(root.join("rain.db"))
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_millis(5));
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        crate::db::prepare_schema(&pool, false).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name) VALUES('METRIC','Metric')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle','METRIC','hash','test','PROCESSING')").execute(&pool).await.unwrap();
        let file_id = sqlx::query_scalar("INSERT INTO files(bundle_id,name,path,is_dir) VALUES('bundle','app.log','/hash/app.log',0) RETURNING id").fetch_one(&pool).await.unwrap();
        std::fs::write(root.join("app.log"), "first\nsecond\n").unwrap();
        (root, pool, file_id)
    }

    #[tokio::test]
    async fn cancelling_file_waiting_for_writer_emits_cancelled_summary() {
        let (root, pool, file_id) = file_fixture().await;
        let permit = crate::db::write::acquire(&pool).await;
        let outcomes = Outcomes::default();
        async {
            let config = crate::config::IndexingConfig::default();
            let path = root.join("app.log");
            let mut future = Box::pin(super::super::ingest_text_file(
                "bundle",
                file_id,
                "/app.log",
                &path,
                13,
                &config,
                Arc::new(crate::search::sqlite::SqliteFtsSearchIndex::new(
                    pool.clone(),
                )),
            ));
            assert!(
                tokio::time::timeout(Duration::from_millis(50), &mut future)
                    .await
                    .is_err()
            );
            drop(future);
        }
        .with_subscriber(tracing_subscriber::registry().with(outcomes.clone()))
        .await;
        assert_eq!(*outcomes.0.lock().unwrap(), ["cancelled"]);
        drop(permit);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM log_segments")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        pool.close().await;
        crate::db::write::remove_fixture_dir(root).await;
    }

    #[derive(Clone)]
    struct RetrySignal(Arc<tokio::sync::Notify>);
    impl Visit for RetrySignal {
        fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
        fn record_bool(&mut self, field: &Field, value: bool) {
            if field.name() == "retry" && value {
                self.0.notify_one();
            }
        }
    }
    impl<S: Subscriber> Layer<S> for RetrySignal {
        fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
            event.record(&mut self.clone());
        }
    }

    #[tokio::test]
    async fn retried_index_transaction_counts_committed_content_once() {
        let (root, pool, file_id) = file_fixture().await;
        let external = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(pool.connect_options().as_ref().clone())
            .await
            .unwrap();
        let mut lock = external.begin().await.unwrap();
        sqlx::query("UPDATE files SET line_count=0")
            .execute(&mut *lock)
            .await
            .unwrap();
        let signal = Arc::new(tokio::sync::Notify::new());
        let path = root.join("app.log");
        let config = crate::config::IndexingConfig::default();
        let mut metrics = FileIndexMetrics::new("bundle", file_id);
        let indexing = super::super::ingest_text_file_inner(
            "bundle",
            file_id,
            "/app.log",
            &path,
            &config,
            Arc::new(crate::search::sqlite::SqliteFtsSearchIndex::new(
                pool.clone(),
            )),
            &mut metrics,
        )
        .with_subscriber(tracing_subscriber::registry().with(RetrySignal(signal.clone())));
        let release = async {
            // Release only after a real SQLITE_BUSY has entered the retry path.
            tokio::time::timeout(Duration::from_secs(5), signal.notified())
                .await
                .unwrap();
            lock.rollback().await.unwrap();
        };
        let (result, ()) = tokio::join!(indexing, release);
        result.unwrap();
        assert_eq!(metrics.source_bytes, 13);
        assert_eq!(metrics.source_lines, 2);
        assert_eq!(metrics.indexed_bytes, "first\nsecond".len() as u64);
        assert_eq!(metrics.committed_chunks, 1);
        assert_eq!(metrics.committed_batches, 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM log_segments")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        pool.close().await;
        external.close().await;
        crate::db::write::remove_fixture_dir(root).await;
    }
}
