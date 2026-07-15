use crate::{
    error::AppError,
    repositories::files::{
        delete_file_row, delete_index_rows_for_file, fetch_extracted_child_ids,
        fetch_storage_paths_for_ids, fetch_subtree_ids,
    },
};

pub async fn delete_file_tree(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    root_file_id: i64,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await.map_err(AppError::Database)?;

    let mut file_ids = fetch_subtree_ids(&mut tx, bundle_id, root_file_id).await?;
    let extracted_root_ids = fetch_extracted_child_ids(&mut tx, bundle_id, root_file_id).await?;
    for extracted_root_id in extracted_root_ids {
        for id in fetch_subtree_ids(&mut tx, bundle_id, extracted_root_id).await? {
            if !file_ids.contains(&id) {
                file_ids.push(id);
            }
        }
    }

    let disk_paths = fetch_storage_paths_for_ids(&mut tx, bundle_id, &file_ids).await?;

    for file_id in &file_ids {
        delete_index_rows_for_file(&mut tx, *file_id).await?;
    }

    for file_id in &file_ids {
        delete_file_row(&mut tx, bundle_id, *file_id).await?;
    }

    tx.commit().await.map_err(AppError::Database)?;

    for disk_path in disk_paths {
        if tokio::fs::metadata(&disk_path).await.is_ok() {
            if disk_path.is_dir() {
                let _ = tokio::fs::remove_dir_all(&disk_path).await;
            } else {
                let _ = tokio::fs::remove_file(&disk_path).await;
            }
        }
    }

    Ok(())
}
