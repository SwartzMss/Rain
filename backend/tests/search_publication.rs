#![cfg(feature = "tantivy-search")]

use std::path::PathBuf;

use std::sync::Arc;
use tokio::sync::Semaphore;

use backend::{
    db,
    search::{
        ContentSearchRequest, ContentSearchScope,
        publication::{SearchBackendKind, artifact_relative_path, claim_publication},
        search_tantivy_bundle,
        tantivy::publication::publish_bundle,
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
    sqlx::query("INSERT INTO log_segments(bundle_id,file_id,timeline,content,line_offset,line_end,chunk_index) VALUES('bundle-pub',7,'all','prefix MARKER suffix',0,0,0),('bundle-pub',7,'all','other line',1,1,1)")
        .execute(&pool)
        .await
        .unwrap();

    let generation = claim_publication(&pool, "bundle-pub", SearchBackendKind::Tantivy)
        .await
        .unwrap();
    publish_bundle(
        &pool,
        &root,
        &root.join(".tmp"),
        "bundle-pub",
        generation,
        Arc::new(Semaphore::new(1)),
        16 * 1024 * 1024,
    )
    .await
    .unwrap();

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
