use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use backend::{
    blob_store::LocalCasBlobStore,
    config::{ArchiveConfig, IndexingConfig},
    db,
    ingest::{ArchiveBudget, IssueQuota, ProcessFileOptions, process_uploaded_file},
};
use serde_json::{Value, json};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    instrument::WithSubscriber,
};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<BTreeMap<String, Value>>>>);

struct Fields(BTreeMap<String, Value>);
impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().into(), json!(format!("{value:?}")));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().into(), json!(value));
    }
}
impl<S: Subscriber> Layer<S> for Events {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut fields = Fields(BTreeMap::new());
        event.record(&mut fields);
        self.0.lock().unwrap().push(fields.0);
    }
}

async fn process(fail: bool) -> Events {
    let events = Events::default();
    let subscriber = tracing_subscriber::registry().with(events.clone());
    async {
        let root = std::env::temp_dir().join(format!("rain-metrics-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let pool = db::init_pool(&format!("sqlite://{}", root.join("rain.db").display())).unwrap();
        db::prepare_schema(&pool, false).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name) VALUES('METRICS','Metrics')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle','METRICS','hash','test','PROCESSING')").execute(&pool).await.unwrap();
        if fail {
            sqlx::query("CREATE TRIGGER fail_segments BEFORE INSERT ON log_segments BEGIN SELECT RAISE(ABORT, 'injected failure'); END").execute(&pool).await.unwrap();
        }
        let source = root.join("source.log");
        let input = b" INFO alpha \r\n\nERROR metrics\n";
        std::fs::write(&source, input).unwrap();
        let result = process_uploaded_file(ProcessFileOptions {
            pool: &pool, bundle_id: "bundle", bundle_hash: "hash", data_root: &root.join("staging"),
            blob_store: Arc::new(LocalCasBlobStore::new(root.join("data"))),
            storage_name: "test.log", original_name: "test.log", display_name: "test.log",
            content_type: Some("text/plain"), source_path: &source, size_bytes: input.len() as u64,
            archive_budget: ArchiveBudget::new(ArchiveConfig::default()),
            issue_quota: IssueQuota::new(pool.clone(), "METRICS", "bundle", 1024 * 1024),
            indexing: &IndexingConfig::default(),
            search_index: None,
        }).await;
        assert_eq!(result.is_err(), fail);
        pool.close().await;
        std::fs::remove_dir_all(root).unwrap();
    }.with_subscriber(subscriber).await;
    events
}

#[tokio::test]
async fn records_original_and_committed_bytes_once_per_file() {
    let events = process(false).await;
    let events = events.0.lock().unwrap();
    let summaries: Vec<_> = events
        .iter()
        .filter(|e| e.get("metric") == Some(&json!("log_index_file")))
        .collect();
    assert_eq!(summaries.len(), 1, "one file summary must be emitted");
    let summary = summaries[0];
    assert_eq!(summary["outcome"], "success");
    assert_eq!(
        summary["source_bytes"],
        b" INFO alpha \r\n\nERROR metrics\n".len()
    );
    assert_eq!(summary["source_lines"], 3);
    assert_eq!(summary["indexed_bytes"], "INFO alpha\nERROR metrics".len());
    assert_eq!(summary["committed_chunks"], 1);
    assert!(summary.contains_key("read_parse_us"));
    assert!(summary.contains_key("write_wait_us"));
    let writes: Vec<_> = events
        .iter()
        .filter(|e| e.get("operation") == Some(&json!("index-batch")))
        .collect();
    assert!(!writes.is_empty());
    for write in writes {
        assert!(write.contains_key("begin_us"));
        assert!(write.contains_key("execute_us"));
        assert!(write.contains_key("finish_us"));
    }
}

#[tokio::test]
async fn failed_indexing_emits_failure_without_counting_uncommitted_bytes() {
    let events = process(true).await;
    let events = events.0.lock().unwrap();
    let summary = events
        .iter()
        .find(|e| e.get("metric") == Some(&json!("log_index_file")))
        .expect("failure must emit a file summary");
    assert_eq!(summary["outcome"], "error");
    assert_eq!(summary["source_lines"], 3);
    assert_eq!(summary["indexed_bytes"], 0);
    assert_eq!(summary["committed_chunks"], 0);
}
