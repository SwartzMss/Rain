#![cfg(feature = "tantivy-search")]

use std::path::PathBuf;

use backend::{
    db,
    search::{
        ContentSearchRequest, ContentSearchScope, IndexBatch, IndexChunk, IngestIndex,
        publication::{SearchBackendKind, artifact_relative_path, claim_publication},
        rebuild::run_once,
        resource::SearchResourceBudget,
        search_tantivy_bundle, search_tantivy_bundle_visible,
        tantivy::publication::BundleBuildSession,
        visibility::snapshot_file_ids,
    },
};

fn fixture_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "rain-search-publication-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ))
}

#[tokio::test]
async fn publishes_reopens_and_searches_a_bundle_index() {
    let root = fixture_root();
    std::fs::create_dir_all(&root).unwrap();
    let pool = db::init_pool(&format!("sqlite://{}", root.join("rain.db").display())).unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('PUB','Publication')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-pub','PUB','hash-pub','fixture','PROCESSING')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO files(id,bundle_id,name,path,is_dir,status) VALUES(7,'bundle-pub','app.log','/app.log',0,'READY')")
        .execute(&pool)
        .await
        .unwrap();
    let generation = claim_publication(&pool, "bundle-pub", SearchBackendKind::Tantivy)
        .await
        .unwrap();
    let budget = SearchResourceBudget::new(1, 16 * 1024 * 1024).unwrap();
    let builder = BundleBuildSession::start(
        &pool,
        &root,
        &root.join(".tmp"),
        "bundle-pub",
        generation,
        budget.clone(),
    )
    .await
    .unwrap();
    assert_eq!(budget.active_writers(), 0);
    builder
        .commit_ingest_batch(IndexBatch {
            bundle_id: "bundle-pub".into(),
            file_id: 7,
            path: "/app.log".into(),
            chunks: vec![
                IndexChunk {
                    chunk_index: 0,
                    line_start: Some(0),
                    line_end: Some(0),
                    event_time_start_ms: None,
                    event_time_end_ms: None,
                    content: "prefix MARKER suffix".into(),
                },
                IndexChunk {
                    chunk_index: 1,
                    line_start: Some(1),
                    line_end: Some(1),
                    event_time_start_ms: None,
                    event_time_end_ms: None,
                    content: "other line".into(),
                },
            ],
            offsets: vec![(0, 0), (1, 20)],
            final_line_count: Some(2),
        })
        .await
        .unwrap();
    assert_eq!(budget.active_writers(), 1);
    builder.finish().await.unwrap();
    assert_eq!(budget.active_writers(), 0);

    let state: (String, String) = sqlx::query_as(
        "SELECT backend,state FROM bundle_search_indexes WHERE bundle_id='bundle-pub'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, ("tantivy".into(), "READY".into()));
    assert!(
        root.join(artifact_relative_path("bundle-pub", generation).unwrap())
            .is_dir()
    );

    let result = search_tantivy_bundle(
        root.join(artifact_relative_path("bundle-pub", generation).unwrap()),
        ContentSearchRequest {
            scope: ContentSearchScope::Bundle {
                bundle_id: "bundle-pub".into(),
                timeline: Some("all".into()),
                file_id: None,
            },
            query: "marker".into(),
            path_like: None,
            from: 0,
            size: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.rows[0].path, "/app.log");
    assert_eq!(result.rows[0].offset, Some(0));

    pool.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn streaming_build_keeps_chunk_body_out_of_sqlite() {
    let root = fixture_root();
    std::fs::create_dir_all(&root).unwrap();
    let pool = db::init_pool(&format!("sqlite://{}", root.join("rain.db").display())).unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('STREAM','Streaming')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-stream','STREAM','hash-stream','fixture','PROCESSING')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO files(id,bundle_id,name,path,is_dir,status) VALUES(8,'bundle-stream','app.log','/app.log',0,'READY')")
        .execute(&pool)
        .await
        .unwrap();

    let generation = claim_publication(&pool, "bundle-stream", SearchBackendKind::Tantivy)
        .await
        .unwrap();
    let builder = BundleBuildSession::start(
        &pool,
        &root,
        &root.join(".tmp"),
        "bundle-stream",
        generation,
        SearchResourceBudget::new(1, 16 * 1024 * 1024).unwrap(),
    )
    .await
    .unwrap();
    builder
        .commit_ingest_batch(IndexBatch {
            bundle_id: "bundle-stream".into(),
            file_id: 8,
            path: "/app.log".into(),
            chunks: vec![IndexChunk {
                chunk_index: 0,
                line_start: Some(0),
                line_end: Some(0),
                event_time_start_ms: None,
                event_time_end_ms: None,
                content: "streamed MARKER body".into(),
            }],
            offsets: vec![(0, 0)],
            final_line_count: Some(1),
        })
        .await
        .unwrap();
    builder.finish().await.unwrap();

    let stored: String =
        sqlx::query_scalar("SELECT content FROM log_segments WHERE bundle_id='bundle-stream'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(stored.is_empty());
    let result = search_tantivy_bundle(
        root.join(artifact_relative_path("bundle-stream", generation).unwrap()),
        ContentSearchRequest {
            scope: ContentSearchScope::Bundle {
                bundle_id: "bundle-stream".into(),
                timeline: Some("all".into()),
                file_id: None,
            },
            query: "marker".into(),
            path_like: None,
            from: 0,
            size: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.rows[0].content, "streamed MARKER body");

    pool.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn aborted_build_releases_resource_budget_for_the_next_bundle() {
    let root = fixture_root();
    std::fs::create_dir_all(&root).unwrap();
    let pool = db::init_pool(&format!("sqlite://{}", root.join("rain.db").display())).unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('ABORT','Abort')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-abort-a','ABORT','hash-abort-a','fixture','PROCESSING'),('bundle-abort-b','ABORT','hash-abort-b','fixture','PROCESSING')")
        .execute(&pool)
        .await
        .unwrap();
    let budget = SearchResourceBudget::new(1, 16 * 1024 * 1024).unwrap();
    let first_generation = claim_publication(&pool, "bundle-abort-a", SearchBackendKind::Tantivy)
        .await
        .unwrap();
    let first = BundleBuildSession::start(
        &pool,
        &root,
        &root.join(".tmp"),
        "bundle-abort-a",
        first_generation,
        budget.clone(),
    )
    .await
    .unwrap();
    first.abort().await;
    drop(first);

    let second_generation = claim_publication(&pool, "bundle-abort-b", SearchBackendKind::Tantivy)
        .await
        .unwrap();
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        BundleBuildSession::start(
            &pool,
            &root,
            &root.join(".tmp"),
            "bundle-abort-b",
            second_generation,
            budget,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    second.abort().await;
    drop(second);

    pool.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn deletion_rebuild_publishes_only_visible_documents() {
    let root = fixture_root();
    std::fs::create_dir_all(&root).unwrap();
    let pool = db::init_pool(&format!("sqlite://{}", root.join("rain.db").display())).unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name,status) VALUES('REBUILD','Rebuild','ACTIVE')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-rebuild','REBUILD','hash-rebuild','fixture','PROCESSING')")
        .execute(&pool)
        .await
        .unwrap();
    for (id, name) in [(7_i64, "deleted.log"), (8_i64, "visible.log")] {
        sqlx::query(
            "INSERT INTO files(id,bundle_id,name,path,is_dir,status) VALUES(?,?,?, ?,0,'READY')",
        )
        .bind(id)
        .bind("bundle-rebuild")
        .bind(name)
        .bind(format!("/{name}"))
        .execute(&pool)
        .await
        .unwrap();
    }
    let generation = claim_publication(&pool, "bundle-rebuild", SearchBackendKind::Tantivy)
        .await
        .unwrap();
    let builder = BundleBuildSession::start(
        &pool,
        &root,
        &root.join(".tmp"),
        "bundle-rebuild",
        generation,
        SearchResourceBudget::new(1, 16 * 1024 * 1024).unwrap(),
    )
    .await
    .unwrap();
    for (file_id, path) in [(7_i64, "/deleted.log"), (8_i64, "/visible.log")] {
        builder
            .commit_ingest_batch(IndexBatch {
                bundle_id: "bundle-rebuild".into(),
                file_id,
                path: path.into(),
                chunks: vec![IndexChunk {
                    chunk_index: 0,
                    line_start: Some(0),
                    line_end: Some(0),
                    event_time_start_ms: None,
                    event_time_end_ms: None,
                    content: "UNIQUE_REBUILD_MARKER".into(),
                }],
                offsets: vec![(0, 0)],
                final_line_count: Some(1),
            })
            .await
            .unwrap();
    }
    builder.finish().await.unwrap();
    sqlx::query("UPDATE bundles SET status='READY' WHERE id='bundle-rebuild'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO file_deletion_jobs(id,bundle_id,root_file_id,state,phase) VALUES('delete-rebuild','bundle-rebuild',7,'QUEUED','WALK')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE bundle_search_indexes SET state='NEEDS_REBUILD', visibility_revision=1 WHERE bundle_id='bundle-rebuild'")
        .execute(&pool)
        .await
        .unwrap();

    run_once(
        &pool,
        &root,
        SearchResourceBudget::new(1, 16 * 1024 * 1024).unwrap(),
    )
    .await
    .unwrap();

    let publication: (i64, String, i64, i64) = sqlx::query_as(
        "SELECT generation,state,visibility_revision,compacted_revision FROM bundle_search_indexes WHERE bundle_id='bundle-rebuild'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(publication, (2, "READY".into(), 1, 1));
    let visible = snapshot_file_ids(&pool, "bundle-rebuild").await.unwrap();
    assert_eq!(visible, std::collections::HashSet::from([8_i64]));
    let result = search_tantivy_bundle_visible(
        root.join(artifact_relative_path("bundle-rebuild", 2).unwrap()),
        ContentSearchRequest {
            scope: ContentSearchScope::Bundle {
                bundle_id: "bundle-rebuild".into(),
                timeline: Some("all".into()),
                file_id: None,
            },
            query: "UNIQUE_REBUILD_MARKER".into(),
            path_like: None,
            from: 0,
            size: 10,
        },
        visible,
    )
    .await
    .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.rows[0].file_id, 8);

    pool.close().await;
    std::fs::remove_dir_all(root).unwrap();
}
