#!/usr/bin/env bash
set -euo pipefail

if [[ ${1:-} == "--self-check" ]]; then
  tmp_root=$(mktemp -d)
  trap 'rm -rf "$tmp_root"' EXIT
  mkdir -p "$tmp_root/backend/src"
  cat >"$tmp_root/backend/src/direct.rs" <<'EOF'
async fn direct(pool: &sqlx::SqlitePool) {
    sqlx::query("UPDATE items SET value = 1").execute(pool).await.unwrap();
}
EOF
  if bash "$0" "$tmp_root" >/dev/null 2>&1; then
    echo "runtime SQLite writer audit self-check: direct writer was accepted" >&2
    exit 1
  fi
  rm "$tmp_root/backend/src/direct.rs"
  cat >"$tmp_root/backend/src/direct-alias.rs" <<'EOF'
async fn direct_alias(db_pool: &sqlx::SqlitePool) {
    sqlx::query("UPDATE items SET value = 1").execute(&db_pool).await.unwrap();
}
EOF
  if bash "$0" "$tmp_root" >/dev/null 2>&1; then
    echo "runtime SQLite writer audit self-check: pool alias was accepted" >&2
    exit 1
  fi
  rm "$tmp_root/backend/src/direct-alias.rs"
  cat >"$tmp_root/backend/src/wrapped.rs" <<'EOF'
async fn wrapped(pool: &sqlx::SqlitePool) {
    crate::db::write::run(pool, "test", &(), |conn, _| Box::pin(async move {
        sqlx::query("UPDATE items SET value = 1").execute(conn).await.unwrap();
        Ok::<_, crate::error::AppError>(())
    })).await.unwrap();
}
EOF
  bash "$0" "$tmp_root"
  echo "runtime SQLite writer audit self-check: ok"
  exit 0
fi

repo_root=${1:-.}
source_root="$repo_root/backend/src"

if [[ ! -d "$source_root" ]]; then
  echo "runtime SQLite writer audit: missing $source_root" >&2
  exit 2
fi

matches=""
while IFS= read -r file; do
  [[ "$file" == */db/migrations.rs ]] && continue
  first_test_line=$(rg -n '^\s*#\[cfg\(test\)\]' "$file" | head -1 | cut -d: -f1 || true)
  while IFS=: read -r line text; do
    [[ -z "$line" ]] && continue
    if [[ -n "$first_test_line" && "$line" -ge "$first_test_line" ]]; then
      continue
    fi
    relative=${file#"$repo_root"/}
    case "$relative:$line" in
      # Startup settings are initialized before normal runtime traffic.
      backend/src/db.rs:625) continue ;;
      # Bootstrap administrator creation runs during startup.
      backend/src/repositories/bootstrap_admin.rs:15) continue ;;
    esac
    matches+="$relative:$line:$text"$'\n'
  done < <(rg -n '\.execute\([^)]*\b([A-Za-z_][A-Za-z0-9_.]*[._])?pool\b[^)]*\)|\b([A-Za-z_][A-Za-z0-9_.]*[._])?pool\.(begin|acquire)\(\)' "$file" || true)
done < <(rg --files "$source_root" -g '*.rs')

if [[ -n "$matches" ]]; then
  echo "Unadmitted runtime SQLite writers found:" >&2
  printf '%s' "$matches" >&2
  exit 1
fi

echo "runtime SQLite writer audit: ok"
