use crate::error::AppError;

pub async fn delete_file_tree(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    root_file_id: i64,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await.map_err(AppError::Database)?;

    let deleted_file = sqlx::query_scalar::<_, i64>(
        "DELETE FROM files WHERE bundle_id = ? AND id = ? RETURNING id",
    )
    .bind(bundle_id)
    .bind(root_file_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(AppError::Database)?;
    if deleted_file.is_none() {
        return Err(AppError::NotFound(format!("file {root_file_id}")));
    }

    sqlx::query(
        "UPDATE bundles SET content_size_bytes = (SELECT COALESCE(SUM(CASE WHEN json_extract(meta, '$.preview_kind') = 'archive' THEN 0 ELSE size_bytes END), 0) FROM files WHERE bundle_id = ? AND is_dir = 0) WHERE id = ?",
    )
    .bind(bundle_id)
    .bind(bundle_id)
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    tx.commit().await.map_err(AppError::Database)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::delete_file_tree;

    #[tokio::test]
    async fn delete_file_tree_uses_cascade_for_a_large_tree() {
        let pool = crate::db::init_pool("sqlite::memory:").expect("init pool");
        crate::db::prepare_schema(&pool, true)
            .await
            .expect("prepare schema");

        sqlx::query("INSERT INTO issues (code, name) VALUES ('DELETE', 'DELETE')")
            .execute(&pool)
            .await
            .expect("insert issue");
        sqlx::query(
            "INSERT INTO bundles (id, issue_code, hash, name, status, content_size_bytes) VALUES ('delete-bundle', 'DELETE', 'delete-hash', 'delete', 'READY', 0), ('other-bundle', 'DELETE', 'other-hash', 'other', 'READY', 0)",
        )
        .execute(&pool)
        .await
        .expect("insert bundles");

        let mut tx = pool.begin().await.expect("begin fixture transaction");
        let root_id: i64 = sqlx::query_scalar(
            "INSERT INTO files (bundle_id, name, path, is_dir) VALUES ('delete-bundle', 'root', '/root', 1) RETURNING id",
        )
        .fetch_one(&mut *tx)
        .await
        .expect("insert root");
        let retained_id: i64 = sqlx::query_scalar(
            "INSERT INTO files (bundle_id, name, path, is_dir, size_bytes) VALUES ('delete-bundle', 'retained.log', '/retained.log', 0, 7) RETURNING id",
        )
        .fetch_one(&mut *tx)
        .await
        .expect("insert retained file");
        sqlx::query(
            "INSERT INTO log_segments (bundle_id, file_id, content) VALUES ('delete-bundle', ?, 'retained content')",
        )
        .bind(retained_id)
        .execute(&mut *tx)
        .await
        .expect("insert retained segment");

        for index in 0..10_000 {
            let file_id: i64 = sqlx::query_scalar(
                "INSERT INTO files (bundle_id, parent_id, name, path, is_dir, size_bytes) VALUES ('delete-bundle', ?, ?, ?, 0, 1) RETURNING id",
            )
            .bind(root_id)
            .bind(format!("file-{index}.log"))
            .bind(format!("/root/file-{index}.log"))
            .fetch_one(&mut *tx)
            .await
            .expect("insert child file");
            sqlx::query(
                "INSERT INTO log_line_offsets (file_id, line_number, byte_offset) VALUES (?, 0, 0)",
            )
            .bind(file_id)
            .execute(&mut *tx)
            .await
            .expect("insert line offset");
            sqlx::query(
                "INSERT INTO log_segments (bundle_id, file_id, content) VALUES ('delete-bundle', ?, ?)",
            )
            .bind(file_id)
            .bind(format!("cascade-child-{index}"))
            .execute(&mut *tx)
            .await
            .expect("insert child segment");
        }
        tx.commit().await.expect("commit fixture transaction");

        let error = delete_file_tree(&pool, "other-bundle", root_id)
            .await
            .expect_err("a file from another bundle must not be deleted");
        assert!(matches!(error, crate::error::AppError::NotFound(_)));

        delete_file_tree(&pool, "delete-bundle", root_id)
            .await
            .expect("delete file tree");

        let counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
            r#"
            SELECT
                (SELECT COUNT(*) FROM files WHERE bundle_id = 'delete-bundle'),
                (SELECT COUNT(*) FROM log_line_offsets),
                (SELECT COUNT(*) FROM log_segments),
                (SELECT COUNT(*) FROM log_segments_fts),
                (SELECT content_size_bytes FROM bundles WHERE id = 'delete-bundle')
            "#,
        )
        .fetch_one(&pool)
        .await
        .expect("count cascaded rows");
        assert_eq!(counts, (1, 0, 1, 1, 7));
    }
}
