use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex, Weak},
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
    async fn verify_size(&self, storage_key: &str, expected_size: u64) -> Result<bool, AppError>;
    async fn verify(
        &self,
        storage_key: &str,
        expected_hash: &str,
        expected_size: u64,
    ) -> Result<bool, AppError>;
    async fn delete(&self, storage_key: &str) -> Result<(), AppError>;
}

pub struct LocalCasBlobStore {
    root: PathBuf,
    publish_locks: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
}

impl LocalCasBlobStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            publish_locks: Mutex::new(HashMap::new()),
        }
    }
    fn path_for_key(&self, key: &str) -> Result<PathBuf, AppError> {
        local_blob_path(&self.root, key)
    }

    fn publish_lock(&self, content_hash: &str) -> Result<Arc<tokio::sync::Mutex<()>>, AppError> {
        let mut locks = self
            .publish_locks
            .lock()
            .map_err(|_| AppError::Config("blob publication lock was poisoned".into()))?;
        if let Some(lock) = locks.get(content_hash).and_then(Weak::upgrade) {
            return Ok(lock);
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(content_hash.to_owned(), Arc::downgrade(&lock));
        locks.retain(|_, weak| weak.strong_count() > 0);
        Ok(lock)
    }
}

#[async_trait]
impl BlobStore for LocalCasBlobStore {
    fn backend_name(&self) -> &'static str {
        "local"
    }
    async fn put(&self, source: &Path) -> Result<StoredBlob, AppError> {
        let (content_hash, size_bytes) = hash_file(source).await?;
        let storage_key = format!("blobs/{}/{}", &content_hash[..2], content_hash);
        let destination = self.path_for_key(&storage_key)?;
        let destination_valid = match fs::metadata(&destination).await {
            Ok(metadata) if metadata.len() == size_bytes => {
                hash_file(&destination).await?.0 == content_hash
            }
            Ok(_) => false,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(AppError::Io(error)),
        };
        if !destination_valid {
            let parent = destination.parent().ok_or_else(|| {
                AppError::Config("blob destination has no parent directory".into())
            })?;
            fs::create_dir_all(parent).await.map_err(AppError::Io)?;
            let temporary = parent.join(format!(".{}.tmp", Uuid::new_v4().simple()));
            fs::copy(source, &temporary).await.map_err(AppError::Io)?;
            let temporary_valid = hash_file(&temporary).await?;
            if temporary_valid != (content_hash.clone(), size_bytes) {
                let _ = fs::remove_file(&temporary).await;
                return Err(AppError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "temporary blob publication verification failed",
                )));
            }
            let publish_lock = self.publish_lock(&content_hash)?;
            let _guard = publish_lock.lock().await;
            if self.verify(&storage_key, &content_hash, size_bytes).await? {
                fs::remove_file(&temporary).await.map_err(AppError::Io)?;
            } else if let Err(error) = atomic_replace(&temporary, &destination).await {
                let _ = fs::remove_file(&temporary).await;
                return Err(AppError::Io(error));
            }
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

    async fn verify_size(&self, storage_key: &str, expected_size: u64) -> Result<bool, AppError> {
        match fs::metadata(self.path_for_key(storage_key)?).await {
            Ok(metadata) => Ok(metadata.len() == expected_size),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(AppError::Io(error)),
        }
    }

    async fn verify(
        &self,
        storage_key: &str,
        expected_hash: &str,
        expected_size: u64,
    ) -> Result<bool, AppError> {
        let path = self.path_for_key(storage_key)?;
        match fs::metadata(&path).await {
            Ok(metadata) if metadata.len() != expected_size => return Ok(false),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(AppError::Io(error)),
        }
        let (actual_hash, actual_size) = hash_file(&path).await?;
        Ok(actual_size == expected_size && actual_hash == expected_hash)
    }

    async fn delete(&self, storage_key: &str) -> Result<(), AppError> {
        match fs::remove_file(self.path_for_key(storage_key)?).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AppError::Io(error)),
        }
    }
}

#[cfg(not(windows))]
async fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination).await
}

#[cfg(windows)]
async fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    tokio::task::spawn_blocking(move || {
        let result = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
    .await
    .map_err(std::io::Error::other)?
}

async fn hash_file(path: &Path) -> Result<(String, u64), AppError> {
    let mut file = fs::File::open(path).await.map_err(AppError::Io)?;
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
    Ok((format!("{:x}", hasher.finalize()), size_bytes))
}

pub async fn persist_blob(
    pool: &SqlitePool,
    store: &dyn BlobStore,
    source: &Path,
) -> Result<i64, AppError> {
    let stored = store.put(source).await?;
    let size_bytes = i64::try_from(stored.size_bytes)
        .map_err(|_| AppError::BadRequest("blob is too large".into()))?;
    let blob_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO blobs (content_hash, size_bytes, storage_backend, storage_key, state)
        VALUES (?, ?, ?, ?, 'STAGING')
        ON CONFLICT(content_hash) DO UPDATE SET
            size_bytes = excluded.size_bytes,
            storage_backend = excluded.storage_backend,
            storage_key = excluded.storage_key,
            state = CASE
                WHEN blobs.state = 'READY' THEN 'READY'
                ELSE 'STAGING'
            END,
            unreferenced_at = NULL
        RETURNING id
        "#,
    )
    .bind(&stored.content_hash)
    .bind(size_bytes)
    .bind(stored.storage_backend)
    .bind(&stored.storage_key)
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;

    // `put` has already verified or published the content. After the database
    // claim, recheck presence and size to close the upload/GC race without
    // hashing the same existing object a second time on the normal path.
    if !store
        .verify_size(&stored.storage_key, stored.size_bytes)
        .await?
    {
        let republished = store.put(source).await?;
        if republished.content_hash != stored.content_hash
            || republished.size_bytes != stored.size_bytes
            || republished.storage_key != stored.storage_key
            || !stored_blob_is_valid(store, &republished).await?
        {
            sqlx::query("UPDATE blobs SET state = 'CORRUPTED' WHERE id = ? AND state = 'STAGING'")
                .bind(blob_id)
                .execute(pool)
                .await
                .map_err(AppError::Database)?;
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "blob publication verification failed",
            )));
        }
    }
    Ok(blob_id)
}

async fn stored_blob_is_valid(
    store: &dyn BlobStore,
    stored: &StoredBlob,
) -> Result<bool, AppError> {
    store
        .verify(&stored.storage_key, &stored.content_hash, stored.size_bytes)
        .await
}

pub async fn mark_blob_ready(
    pool: &SqlitePool,
    store: &dyn BlobStore,
    blob_id: i64,
) -> Result<(), AppError> {
    let (content_hash, size_bytes, storage_backend, storage_key, state): (
        String,
        i64,
        String,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT content_hash, size_bytes, storage_backend, storage_key, state FROM blobs WHERE id = ?",
    )
    .bind(blob_id)
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;
    if state == "READY" {
        return Ok(());
    }
    if state != "STAGING" {
        return Err(AppError::Conflict(format!(
            "blob {blob_id} cannot become READY from {state}"
        )));
    }
    if storage_backend != store.backend_name() {
        return Err(AppError::Config(format!(
            "blob {blob_id} belongs to unsupported backend {storage_backend}"
        )));
    }
    let stored = StoredBlob {
        content_hash,
        size_bytes: size_bytes.max(0) as u64,
        storage_backend: store.backend_name(),
        storage_key,
    };
    if !stored_blob_is_valid(store, &stored).await? {
        let missing = !store.exists(&stored.storage_key).await?;
        sqlx::query("UPDATE blobs SET state = ? WHERE id = ? AND state = 'STAGING'")
            .bind(if missing { "MISSING" } else { "CORRUPTED" })
            .bind(blob_id)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
        return Err(AppError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "blob failed READY publication verification",
        )));
    }
    sqlx::query("UPDATE blobs SET state = 'READY', verified_at = CURRENT_TIMESTAMP WHERE id = ? AND state = 'STAGING'")
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
    let staging: Vec<(i64, String, i64, String)> = sqlx::query_as(
        "SELECT id, content_hash, size_bytes, storage_key FROM blobs WHERE state = 'STAGING' AND storage_backend = ?",
    )
    .bind(store.backend_name())
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    for (id, expected_hash, expected_size, storage_key) in staging {
        let state = if store
            .verify(&storage_key, &expected_hash, expected_size.max(0) as u64)
            .await?
        {
            "READY"
        } else if store.exists(&storage_key).await? {
            "CORRUPTED"
        } else {
            "MISSING"
        };
        sqlx::query(
            "UPDATE blobs SET state = ?, verified_at = CASE WHEN ? = 'READY' THEN CURRENT_TIMESTAMP ELSE verified_at END WHERE id = ? AND state = 'STAGING'",
        )
            .bind(state)
            .bind(state)
            .bind(id)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }
    garbage_collect_unreferenced_blobs(pool, store).await
}

pub async fn audit_local_blobs(pool: &SqlitePool, store: &dyn BlobStore) -> Result<u64, AppError> {
    const AUDIT_BATCH_SIZE: i64 = 100;
    const AUDIT_BYTE_BUDGET: u64 = 5 * 1024 * 1024 * 1024;
    let rows: Vec<(i64, String, i64, String)> = sqlx::query_as(
        r#"
        SELECT id, content_hash, size_bytes, storage_key
        FROM blobs
        WHERE storage_backend = ? AND state = 'READY'
        ORDER BY verified_at IS NOT NULL, datetime(verified_at), id
        LIMIT ?
        "#,
    )
    .bind(store.backend_name())
    .bind(AUDIT_BATCH_SIZE)
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    let mut unhealthy = 0u64;
    let mut audited_bytes = 0u64;
    for (id, expected_hash, expected_size, storage_key) in rows {
        let expected_size = expected_size.max(0) as u64;
        if audited_bytes > 0 && audited_bytes.saturating_add(expected_size) > AUDIT_BYTE_BUDGET {
            break;
        }
        audited_bytes = audited_bytes.saturating_add(expected_size);
        if store
            .verify(&storage_key, &expected_hash, expected_size)
            .await?
        {
            sqlx::query(
                "UPDATE blobs SET verified_at = CURRENT_TIMESTAMP WHERE id = ? AND state = 'READY'",
            )
            .bind(id)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
            continue;
        }
        let state = if store.exists(&storage_key).await? {
            "CORRUPTED"
        } else {
            "MISSING"
        };
        sqlx::query("UPDATE blobs SET state = ?, verified_at = CURRENT_TIMESTAMP WHERE id = ? AND state = 'READY'")
            .bind(state)
            .bind(id)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
        unhealthy += 1;
    }
    Ok(unhealthy)
}

pub fn spawn_blob_audit(pool: SqlitePool, store: Arc<dyn BlobStore>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now() + std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(3600),
        );
        loop {
            interval.tick().await;
            match audit_local_blobs(&pool, store.as_ref()).await {
                Ok(unhealthy) => tracing::info!(unhealthy, "blob integrity audit batch completed"),
                Err(error) => tracing::warn!(%error, "blob integrity audit batch failed"),
            }
        }
    });
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
    let anomalies: Vec<(i64, String, String)> = sqlx::query_as(
        r#"
        SELECT id, storage_key, state
        FROM blobs b
        WHERE storage_backend = ?
          AND state IN ('MISSING', 'CORRUPTED')
          AND NOT EXISTS (SELECT 1 FROM files f WHERE f.blob_id = b.id)
        "#,
    )
    .bind(store.backend_name())
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    let mut removed = 0u64;
    for (id, storage_key, state) in anomalies {
        if state == "MISSING" {
            removed += sqlx::query(
                "DELETE FROM blobs WHERE id = ? AND state = 'MISSING' AND NOT EXISTS (SELECT 1 FROM files WHERE files.blob_id = blobs.id)",
            )
            .bind(id)
            .execute(pool)
            .await
            .map_err(AppError::Database)?
            .rows_affected();
            continue;
        }

        let mut delete_tx = pool.begin().await.map_err(AppError::Database)?;
        let claimed = sqlx::query(
            r#"
            UPDATE blobs SET state = 'PENDING_DELETE'
            WHERE id = ? AND state = 'CORRUPTED'
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
        if let Err(error) = store.delete(&storage_key).await {
            delete_tx.rollback().await.map_err(AppError::Database)?;
            tracing::warn!(storage_key, %error, "failed to remove unreferenced corrupted blob");
            continue;
        }
        sqlx::query("DELETE FROM blobs WHERE id = ? AND state = 'PENDING_DELETE'")
            .bind(id)
            .execute(&mut *delete_tx)
            .await
            .map_err(AppError::Database)?;
        delete_tx.commit().await.map_err(AppError::Database)?;
        removed += 1;
    }

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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn concurrent_publication_of_one_hash_is_serialized_and_atomic() {
        let root = std::env::temp_dir().join(format!(
            "rain-blob-publish-lock-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&root).await.unwrap();
        let first = root.join("first.log");
        let second = root.join("second.log");
        fs::write(&first, b"same content").await.unwrap();
        fs::write(&second, b"same content").await.unwrap();
        let (content_hash, size_bytes) = hash_file(&first).await.unwrap();
        let storage_key = format!("blobs/{}/{}", &content_hash[..2], content_hash);
        let destination = local_blob_path(&root, &storage_key).unwrap();
        fs::create_dir_all(destination.parent().unwrap())
            .await
            .unwrap();
        fs::write(&destination, b"evil content").await.unwrap();
        let store = Arc::new(LocalCasBlobStore::new(root.clone()));

        let (left, right) = tokio::join!(store.put(&first), store.put(&second));
        left.unwrap();
        right.unwrap();
        assert!(
            store
                .verify(&storage_key, &content_hash, size_bytes)
                .await
                .unwrap()
        );
        let _ = fs::remove_dir_all(root).await;
    }

    struct DeleteAfterFirstPutStore {
        inner: LocalCasBlobStore,
        puts: AtomicUsize,
    }

    #[async_trait]
    impl BlobStore for DeleteAfterFirstPutStore {
        fn backend_name(&self) -> &'static str {
            self.inner.backend_name()
        }

        async fn put(&self, source: &Path) -> Result<StoredBlob, AppError> {
            let stored = self.inner.put(source).await?;
            if self.puts.fetch_add(1, Ordering::SeqCst) == 0 {
                self.inner.delete(&stored.storage_key).await?;
            }
            Ok(stored)
        }

        async fn open(&self, storage_key: &str) -> Result<BlobReader, AppError> {
            self.inner.open(storage_key).await
        }

        async fn materialize(&self, storage_key: &str) -> Result<PathBuf, AppError> {
            self.inner.materialize(storage_key).await
        }

        async fn exists(&self, storage_key: &str) -> Result<bool, AppError> {
            self.inner.exists(storage_key).await
        }

        async fn verify_size(
            &self,
            storage_key: &str,
            expected_size: u64,
        ) -> Result<bool, AppError> {
            self.inner.verify_size(storage_key, expected_size).await
        }

        async fn verify(
            &self,
            storage_key: &str,
            expected_hash: &str,
            expected_size: u64,
        ) -> Result<bool, AppError> {
            self.inner
                .verify(storage_key, expected_hash, expected_size)
                .await
        }

        async fn delete(&self, storage_key: &str) -> Result<(), AppError> {
            self.inner.delete(storage_key).await
        }
    }

    #[tokio::test]
    async fn republishes_when_blob_disappears_between_put_and_database_claim() {
        let root = std::env::temp_dir().join(format!("rain-blob-race-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&root).await.unwrap();
        let source = root.join("source.log");
        fs::write(&source, b"race-safe content").await.unwrap();
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        let store = DeleteAfterFirstPutStore {
            inner: LocalCasBlobStore::new(root.clone()),
            puts: AtomicUsize::new(0),
        };

        let blob_id = persist_blob(&pool, &store, &source).await.unwrap();
        assert_eq!(store.puts.load(Ordering::SeqCst), 2);
        let (storage_key, state): (String, String) =
            sqlx::query_as("SELECT storage_key, state FROM blobs WHERE id = ?")
                .bind(blob_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(store.exists(&storage_key).await.unwrap());
        assert_eq!(state, "STAGING");
        mark_blob_ready(&pool, &store, blob_id).await.unwrap();
        let _ = fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn ready_publication_rejects_same_size_hash_mismatch() {
        let root =
            std::env::temp_dir().join(format!("rain-blob-ready-hash-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&root).await.unwrap();
        let source = root.join("source.log");
        fs::write(&source, b"same content").await.unwrap();
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        let store = LocalCasBlobStore::new(root.clone());
        let blob_id = persist_blob(&pool, &store, &source).await.unwrap();
        let storage_key: String = sqlx::query_scalar("SELECT storage_key FROM blobs WHERE id = ?")
            .bind(blob_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        fs::write(
            local_blob_path(&root, &storage_key).unwrap(),
            b"evil content",
        )
        .await
        .unwrap();

        assert!(mark_blob_ready(&pool, &store, blob_id).await.is_err());
        let state: String = sqlx::query_scalar("SELECT state FROM blobs WHERE id = ?")
            .bind(blob_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(state, "CORRUPTED");
        let _ = fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn startup_recovery_rejects_same_size_hash_mismatch() {
        let root = std::env::temp_dir().join(format!(
            "rain-blob-recovery-hash-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&root).await.unwrap();
        let source = root.join("source.log");
        fs::write(&source, b"same content").await.unwrap();
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        let store = LocalCasBlobStore::new(root.clone());
        let blob_id = persist_blob(&pool, &store, &source).await.unwrap();
        let storage_key: String = sqlx::query_scalar("SELECT storage_key FROM blobs WHERE id = ?")
            .bind(blob_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO issues (code, name) VALUES ('RECOVERY', 'Recovery')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundles (id, issue_code, hash, name) VALUES ('recovery-bundle', 'RECOVERY', 'recovery-hash', 'Recovery')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO files (bundle_id, blob_id, name, path, is_dir) VALUES ('recovery-bundle', ?, 'source.log', '/source.log', 0)")
            .bind(blob_id)
            .execute(&pool)
            .await
            .unwrap();
        fs::write(
            local_blob_path(&root, &storage_key).unwrap(),
            b"evil content",
        )
        .await
        .unwrap();

        recover_pending_blobs(&pool, &store).await.unwrap();
        let state: String = sqlx::query_scalar("SELECT state FROM blobs WHERE id = ?")
            .bind(blob_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(state, "CORRUPTED");
        let _ = fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn gc_removes_unreferenced_missing_and_corrupted_blobs() {
        let root =
            std::env::temp_dir().join(format!("rain-blob-anomaly-gc-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&root).await.unwrap();
        let source = root.join("source.log");
        fs::write(&source, b"corrupted orphan").await.unwrap();
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        let store = LocalCasBlobStore::new(root.clone());
        let corrupted = store.put(&source).await.unwrap();
        let corrupted_path = local_blob_path(&root, &corrupted.storage_key).unwrap();
        sqlx::query("INSERT INTO blobs (content_hash, size_bytes, storage_backend, storage_key, state) VALUES (?, ?, 'local', ?, 'CORRUPTED')")
            .bind(&corrupted.content_hash)
            .bind(corrupted.size_bytes as i64)
            .bind(&corrupted.storage_key)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO blobs (content_hash, size_bytes, storage_backend, storage_key, state) VALUES (?, 1, 'local', ?, 'MISSING')")
            .bind("f".repeat(64))
            .bind(format!("blobs/ff/{}", "f".repeat(64)))
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            garbage_collect_unreferenced_blobs(&pool, &store)
                .await
                .unwrap(),
            2
        );
        assert!(!corrupted_path.exists());
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM blobs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0);
        let _ = fs::remove_dir_all(root).await;
    }

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
        mark_blob_ready(&pool, &store, first_id).await.unwrap();
        let duplicate_id = persist_blob(&pool, &store, &second).await.unwrap();
        assert_eq!(duplicate_id, first_id);
        let shared_state: String = sqlx::query_scalar("SELECT state FROM blobs WHERE id = ?")
            .bind(first_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(shared_state, "READY");
        fs::write(&blob_path, b"evil content").await.unwrap();
        assert_eq!(audit_local_blobs(&pool, &store).await.unwrap(), 1);
        let verified_at: Option<String> =
            sqlx::query_scalar("SELECT verified_at FROM blobs WHERE id = ?")
                .bind(first_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(verified_at.is_some());
        let state: String = sqlx::query_scalar("SELECT state FROM blobs WHERE id = ?")
            .bind(first_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(state, "CORRUPTED");
        fs::write(&blob_path, b"same content").await.unwrap();
        assert!(mark_blob_ready(&pool, &store, first_id).await.is_err());
        let state: String = sqlx::query_scalar("SELECT state FROM blobs WHERE id = ?")
            .bind(first_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(state, "CORRUPTED");
        assert_eq!(
            persist_blob(&pool, &store, &second).await.unwrap(),
            first_id
        );
        mark_blob_ready(&pool, &store, first_id).await.unwrap();
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
