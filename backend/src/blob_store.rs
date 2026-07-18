use std::{
    path::{Path, PathBuf},
    pin::Pin,
};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::{fs, io::AsyncReadExt};
use uuid::Uuid;

use crate::error::AppError;

pub type BlobReader = Pin<Box<dyn tokio::io::AsyncRead + Send>>;

pub struct StoredBlob {
    pub content_hash: String,
    pub size_bytes: u64,
    pub storage_backend: &'static str,
    pub storage_key: String,
}

#[async_trait]
pub trait BlobStore: Send + Sync {
    fn backend_name(&self) -> &'static str;
    async fn put(&self, source: &Path) -> Result<StoredBlob, AppError>;
    async fn open(&self, storage_key: &str) -> Result<BlobReader, AppError>;
    async fn materialize(&self, storage_key: &str) -> Result<PathBuf, AppError>;
    async fn exists(&self, storage_key: &str) -> Result<bool, AppError>;
    async fn delete(&self, storage_key: &str) -> Result<(), AppError>;
}

pub struct LocalCasBlobStore {
    root: PathBuf,
}

impl LocalCasBlobStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    fn path_for_key(&self, key: &str) -> Result<PathBuf, AppError> {
        local_blob_path(&self.root, key)
    }
}

#[async_trait]
impl BlobStore for LocalCasBlobStore {
    fn backend_name(&self) -> &'static str {
        "local"
    }
    async fn put(&self, source: &Path) -> Result<StoredBlob, AppError> {
        let mut file = fs::File::open(source).await.map_err(AppError::Io)?;
        let mut hasher = Sha256::new();
        let mut size_bytes = 0u64;
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let read = file.read(&mut buffer).await.map_err(AppError::Io)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            size_bytes = size_bytes.saturating_add(read as u64);
        }
        let content_hash = format!("{:x}", hasher.finalize());
        let storage_key = format!("blobs/{}/{}", &content_hash[..2], content_hash);
        let destination = self.path_for_key(&storage_key)?;
        if fs::metadata(&destination).await.is_err() {
            let parent = destination.parent().ok_or_else(|| {
                AppError::Config("blob destination has no parent directory".into())
            })?;
            fs::create_dir_all(parent).await.map_err(AppError::Io)?;
            let temporary = parent.join(format!(".{}.tmp", Uuid::new_v4().simple()));
            fs::copy(source, &temporary).await.map_err(AppError::Io)?;
            if let Err(error) = fs::rename(&temporary, &destination).await
                && fs::metadata(&destination).await.is_err()
            {
                let _ = fs::remove_file(&temporary).await;
                return Err(AppError::Io(error));
            }
            let _ = fs::remove_file(&temporary).await;
        }
        Ok(StoredBlob {
            content_hash,
            size_bytes,
            storage_backend: "local",
            storage_key,
        })
    }

    async fn open(&self, storage_key: &str) -> Result<BlobReader, AppError> {
        Ok(Box::pin(
            fs::File::open(self.path_for_key(storage_key)?)
                .await
                .map_err(AppError::Io)?,
        ))
    }

    async fn materialize(&self, storage_key: &str) -> Result<PathBuf, AppError> {
        self.path_for_key(storage_key)
    }

    async fn exists(&self, storage_key: &str) -> Result<bool, AppError> {
        fs::try_exists(self.path_for_key(storage_key)?)
            .await
            .map_err(AppError::Io)
    }

    async fn delete(&self, storage_key: &str) -> Result<(), AppError> {
        match fs::remove_file(self.path_for_key(storage_key)?).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AppError::Io(error)),
        }
    }
}

pub async fn persist_blob(
    pool: &SqlitePool,
    store: &dyn BlobStore,
    source: &Path,
) -> Result<i64, AppError> {
    let stored = store.put(source).await?;
    let size_bytes = i64::try_from(stored.size_bytes)
        .map_err(|_| AppError::BadRequest("blob is too large".into()))?;
    sqlx::query_scalar(
        r#"
        INSERT INTO blobs (content_hash, size_bytes, storage_backend, storage_key, state)
        VALUES (?, ?, ?, ?, 'STAGING')
        ON CONFLICT(content_hash) DO UPDATE SET
            size_bytes = excluded.size_bytes,
            state = 'STAGING',
            unreferenced_at = NULL
        RETURNING id
        "#,
    )
    .bind(stored.content_hash)
    .bind(size_bytes)
    .bind(stored.storage_backend)
    .bind(stored.storage_key)
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)
}

pub async fn mark_blob_ready(pool: &SqlitePool, blob_id: i64) -> Result<(), AppError> {
    sqlx::query("UPDATE blobs SET state = 'READY' WHERE id = ?")
        .bind(blob_id)
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
    Ok(())
}

pub async fn recover_pending_blobs(
    pool: &SqlitePool,
    store: &dyn BlobStore,
) -> Result<u64, AppError> {
    sqlx::query("UPDATE blobs SET state = 'READY' WHERE state = 'STAGING'")
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
    garbage_collect_unreferenced_blobs(pool, store).await
}

pub async fn audit_local_blobs(pool: &SqlitePool, store: &dyn BlobStore) -> Result<u64, AppError> {
    let rows: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT id, size_bytes, storage_key FROM blobs WHERE storage_backend = ? AND state = 'READY'",
    )
    .bind(store.backend_name())
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    let mut unhealthy = 0u64;
    for (id, expected_size, storage_key) in rows {
        let path = store.materialize(&storage_key).await?;
        let state = match fs::metadata(path).await {
            Ok(metadata) if metadata.len() == expected_size.max(0) as u64 => continue,
            Ok(_) => "CORRUPTED",
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => "MISSING",
            Err(error) => return Err(AppError::Io(error)),
        };
        sqlx::query("UPDATE blobs SET state = ? WHERE id = ? AND state = 'READY'")
            .bind(state)
            .bind(id)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
        unhealthy += 1;
    }
    Ok(unhealthy)
}

fn local_blob_path(data_root: &Path, storage_key: &str) -> Result<PathBuf, AppError> {
    let path = Path::new(storage_key);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        })
        || !storage_key.starts_with("blobs/")
    {
        return Err(AppError::BadRequest(
            "invalid local blob storage key".into(),
        ));
    }
    Ok(data_root.join(path))
}

pub async fn garbage_collect_unreferenced_blobs(
    pool: &SqlitePool,
    store: &dyn BlobStore,
) -> Result<u64, AppError> {
    garbage_collect_unreferenced_blobs_with_grace(pool, store, 24).await
}

pub async fn garbage_collect_unreferenced_blobs_with_grace(
    pool: &SqlitePool,
    store: &dyn BlobStore,
    grace_hours: u64,
) -> Result<u64, AppError> {
    let mut tx = pool.begin().await.map_err(AppError::Database)?;
    sqlx::query(
        r#"
        UPDATE blobs AS b SET unreferenced_at = NULL
        WHERE unreferenced_at IS NOT NULL
          AND EXISTS (SELECT 1 FROM files f WHERE f.blob_id = b.id)
        "#,
    )
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;
    sqlx::query(
        r#"
        UPDATE blobs AS b SET unreferenced_at = CURRENT_TIMESTAMP
        WHERE state = 'READY' AND unreferenced_at IS NULL
          AND NOT EXISTS (SELECT 1 FROM files f WHERE f.blob_id = b.id)
        "#,
    )
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;
    let grace = format!("-{grace_hours} hours");
    let rows: Vec<(i64, String)> = sqlx::query_as(
        r#"
        SELECT id, storage_key FROM blobs b
        WHERE storage_backend = ?
          AND state IN ('READY', 'PENDING_DELETE')
          AND NOT EXISTS (SELECT 1 FROM files f WHERE f.blob_id = b.id)
          AND (state = 'PENDING_DELETE' OR datetime(unreferenced_at) <= datetime('now', ?))
        "#,
    )
    .bind(store.backend_name())
    .bind(grace)
    .fetch_all(&mut *tx)
    .await
    .map_err(AppError::Database)?;
    for (id, _) in &rows {
        sqlx::query("UPDATE blobs SET state = 'PENDING_DELETE' WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;
    }
    tx.commit().await.map_err(AppError::Database)?;

    let mut removed = 0u64;
    for (id, storage_key) in &rows {
        let mut delete_tx = pool.begin().await.map_err(AppError::Database)?;
        let claimed = sqlx::query(
            r#"
            UPDATE blobs SET state = 'PENDING_DELETE'
            WHERE id = ? AND state = 'PENDING_DELETE'
              AND NOT EXISTS (SELECT 1 FROM files WHERE files.blob_id = blobs.id)
            "#,
        )
        .bind(id)
        .execute(&mut *delete_tx)
        .await
        .map_err(AppError::Database)?
        .rows_affected();
        if claimed == 0 {
            delete_tx.rollback().await.map_err(AppError::Database)?;
            continue;
        }
        match store.delete(storage_key).await {
            Ok(()) => {}
            Err(error) => {
                delete_tx.rollback().await.map_err(AppError::Database)?;
                tracing::warn!(storage_key, %error, "failed to remove unused blob");
                continue;
            }
        }
        sqlx::query("DELETE FROM blobs WHERE id = ? AND state = 'PENDING_DELETE'")
            .bind(id)
            .execute(&mut *delete_tx)
            .await
            .map_err(AppError::Database)?;
        delete_tx.commit().await.map_err(AppError::Database)?;
        removed += 1;
    }
    Ok(removed)
}

pub fn spawn_blob_gc(pool: SqlitePool, store: std::sync::Arc<dyn BlobStore>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now() + std::time::Duration::from_secs(3600),
            std::time::Duration::from_secs(3600),
        );
        loop {
            interval.tick().await;
            match garbage_collect_unreferenced_blobs(&pool, store.as_ref()).await {
                Ok(removed) if removed > 0 => tracing::info!(removed, "blob GC completed"),
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "blob GC scan failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn identical_content_reuses_one_blob_and_gc_respects_references() {
        let root = std::env::temp_dir().join(format!("rain-blobs-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&root).await.unwrap();
        let first = root.join("first.log");
        let second = root.join("second.log");
        fs::write(&first, b"same content").await.unwrap();
        fs::write(&second, b"same content").await.unwrap();
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        let store = LocalCasBlobStore::new(root.clone());

        let first_id = persist_blob(&pool, &store, &first).await.unwrap();
        let second_id = persist_blob(&pool, &store, &second).await.unwrap();
        assert_eq!(first_id, second_id);
        let (count, storage_key): (i64, String) =
            sqlx::query_as("SELECT COUNT(*), storage_key FROM blobs")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
        let blob_path = local_blob_path(&root, &storage_key).unwrap();
        assert!(blob_path.is_file());
        assert!(store.exists(&storage_key).await.unwrap());
        let mut reader = store.open(&storage_key).await.unwrap();
        let mut opened = Vec::new();
        reader.read_to_end(&mut opened).await.unwrap();
        assert_eq!(opened, b"same content");

        sqlx::query("INSERT INTO issues (code, name) VALUES ('BLOB', 'Blob')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundles (id, issue_code, hash, name) VALUES ('bundle', 'BLOB', 'hash', 'Bundle')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO files (bundle_id, blob_id, name, path, is_dir) VALUES ('bundle', ?, 'file.log', '/file.log', 0)")
            .bind(first_id)
            .execute(&pool)
            .await
            .unwrap();
        mark_blob_ready(&pool, first_id).await.unwrap();
        fs::write(&blob_path, b"wrong size").await.unwrap();
        assert_eq!(audit_local_blobs(&pool, &store).await.unwrap(), 1);
        let state: String = sqlx::query_scalar("SELECT state FROM blobs WHERE id = ?")
            .bind(first_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(state, "CORRUPTED");
        fs::write(&blob_path, b"same content").await.unwrap();
        mark_blob_ready(&pool, first_id).await.unwrap();
        assert_eq!(
            garbage_collect_unreferenced_blobs_with_grace(&pool, &store, 0)
                .await
                .unwrap(),
            0
        );
        sqlx::query("DELETE FROM files")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            garbage_collect_unreferenced_blobs(&pool, &store)
                .await
                .unwrap(),
            0
        );
        assert!(blob_path.exists());
        let unreferenced_at: Option<String> =
            sqlx::query_scalar("SELECT unreferenced_at FROM blobs WHERE id = ?")
                .bind(first_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(unreferenced_at.is_some());
        assert_eq!(
            garbage_collect_unreferenced_blobs_with_grace(&pool, &store, 0)
                .await
                .unwrap(),
            1
        );
        assert!(!local_blob_path(&root, &storage_key).unwrap().exists());
        let _ = fs::remove_dir_all(root).await;
    }
}
