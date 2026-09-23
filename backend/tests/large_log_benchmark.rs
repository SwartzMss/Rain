//! Opt-in measurement tool. See docs/performance/large-log-baseline.md.
#[path = "support/large_log_fixture.rs"]
mod fixture;
use actix_web::{App, cookie::Cookie, test, web};
use backend::{
    AppState,
    auth::session::{SESSION_COOKIE_NAME, generate_session_token, hash_session_token},
    blob_store::LocalCasBlobStore,
    config::{AppLimits, ArchiveConfig},
    db,
    ingest::{ArchiveBudget, IssueQuota, ProcessFileOptions, process_uploaded_file},
    repositories::{sessions, users},
    routes,
    search::{IngestIndex, publication::SearchBackendKind, resource::SearchResourceBudget},
    upload::{finalizer::finalize_bundle_ready_with_retry, lifecycle::create_processing_bundle},
};
use futures_util::{FutureExt, future::join_all};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tracing::instrument::WithSubscriber;
use tracing_subscriber::prelude::*;

#[derive(Clone, Default)]
struct Metrics(Arc<std::sync::Mutex<std::collections::BTreeMap<String, Value>>>);
#[derive(Default)]
struct Fields(serde_json::Map<String, Value>);
impl tracing::field::Visit for Fields {
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().into(), json!(format!("{value:?}")));
    }
}
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Metrics {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let Some(metric) = fields.0.get("metric").and_then(Value::as_str) else {
            return;
        };
        if ![
            "sqlite_write",
            "log_index_file",
            "operation_phase",
            "tantivy_search",
            "tantivy_index_build",
            "upload_preflight",
        ]
        .contains(&metric)
        {
            return;
        }
        let key = format!(
            "{metric}/{}/{}/{}",
            fields
                .0
                .get("phase")
                .and_then(Value::as_str)
                .unwrap_or("all"),
            fields
                .0
                .get("outcome")
                .and_then(Value::as_str)
                .unwrap_or("unspecified"),
            fields
                .0
                .get("operation")
                .and_then(Value::as_str)
                .unwrap_or("all")
        );
        let mut data = self.0.lock().unwrap();
        let aggregate = data
            .entry(key)
            .or_insert_with(|| json!({"events": 0, "sum": {}}));
        aggregate["events"] = json!(aggregate["events"].as_u64().unwrap() + 1);
        for (key, value) in fields.0 {
            // Ignore identifiers; aggregate measured numeric fields only.
            if (key.ends_with("_us")
                || key.ends_with("_ms")
                || key.ends_with("_bytes")
                || [
                    "active_writers",
                    "writer_active_writers",
                    "committed_batches",
                    "committed_chunks",
                    "queued_writers",
                    "source_lines",
                ]
                .contains(&key.as_str()))
                && let Some(number) = value.as_u64()
            {
                aggregate["sum"][&key] = json!(
                    aggregate["sum"][&key]
                        .as_u64()
                        .unwrap_or(0)
                        .saturating_add(number)
                );
            }
        }
    }
}

fn number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .map(|s| s.parse().expect("benchmark setting must be an integer"))
        .unwrap_or(default)
}
fn command(program: &str, args: &[&str]) -> Option<String> {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}
fn size(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}
fn proc_number(path: &str, key: &str, multiplier: u64) -> Option<u64> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    fs::read_to_string(path).ok()?.lines().find_map(|line| {
        let value = line
            .strip_prefix(key)?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?;
        Some(value * multiplier)
    })
}
fn cpu_ticks() -> Option<(u64, u64)> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let stat = fs::read_to_string("/proc/self/stat").ok()?;
    let fields: Vec<_> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some((fields.get(11)?.parse().ok()?, fields.get(12)?.parse().ok()?))
}
fn resources(db: &Path) -> Value {
    let ticks = cpu_ticks();
    json!({"db_bytes": size(db), "wal_bytes": size(&db.with_extension("db-wal")),
        "cpu_user_ticks": ticks.map(|t| t.0), "cpu_system_ticks": ticks.map(|t| t.1),
        "rss_bytes": proc_number("/proc/self/status", "VmRSS:", 1024),
        "process_read_bytes": proc_number("/proc/self/io", "read_bytes:", 1),
        "process_write_bytes": proc_number("/proc/self/io", "write_bytes:", 1)})
}

#[cfg(feature = "tantivy-search")]
type BenchBuild = Option<Arc<backend::search::tantivy::publication::BundleBuildSession>>;

#[cfg(not(feature = "tantivy-search"))]
type BenchBuild = Option<()>;

#[cfg(feature = "tantivy-search")]
async fn begin_bench_build(
    backend: SearchBackendKind,
    pool: &sqlx::SqlitePool,
    data_root: &Path,
    temp_root: &Path,
    bundle_id: &str,
    resource_budget: SearchResourceBudget,
) -> Result<BenchBuild, backend::error::AppError> {
    if backend != SearchBackendKind::Tantivy {
        return Ok(None);
    }
    let generation = backend::search::publication::claim_publication(
        pool,
        bundle_id,
        SearchBackendKind::Tantivy,
    )
    .await?;
    backend::search::tantivy::publication::BundleBuildSession::start(
        pool,
        data_root,
        temp_root,
        bundle_id,
        generation,
        resource_budget,
    )
    .await
    .map(Some)
}

#[cfg(not(feature = "tantivy-search"))]
async fn begin_bench_build(
    _backend: SearchBackendKind,
    _pool: &sqlx::SqlitePool,
    _data_root: &Path,
    _temp_root: &Path,
    _bundle_id: &str,
    _resource_budget: SearchResourceBudget,
) -> Result<BenchBuild, backend::error::AppError> {
    Ok(None)
}

fn bench_index(build: &BenchBuild) -> Option<Arc<dyn IngestIndex>> {
    #[cfg(feature = "tantivy-search")]
    {
        build
            .as_ref()
            .map(|session| session.clone() as Arc<dyn IngestIndex>)
    }
    #[cfg(not(feature = "tantivy-search"))]
    {
        let _ = build;
        None
    }
}

fn clone_bench_build(build: &BenchBuild) -> BenchBuild {
    #[cfg(feature = "tantivy-search")]
    {
        build.clone()
    }
    #[cfg(not(feature = "tantivy-search"))]
    {
        *build
    }
}

async fn finish_bench_build(build: &BenchBuild) -> Result<(), backend::error::AppError> {
    #[cfg(feature = "tantivy-search")]
    if let Some(session) = build {
        return session.finish().await;
    }
    let _ = build;
    Ok(())
}
struct Sampler {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Vec<Value>>>,
}
impl Sampler {
    fn new(db: std::path::PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let started = Instant::now();
            let mut samples = Vec::new();
            loop {
                let mut sample = resources(&db);
                sample["elapsed_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
                samples.push(sample);
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            samples
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
    fn finish(mut self) -> Vec<Value> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap()
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
fn percentiles(samples: &[f64]) -> Value {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = |p: f64| sorted[((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1)];
    json!({"p50_ms": at(0.50), "p95_ms": at(0.95), "p99_ms": at(0.99)})
}

#[actix_web::test]
#[ignore = "opt-in release benchmark; generates large files and indexes them"]
async fn large_log_baseline() {
    let bytes = number("RAIN_BENCH_BYTES", 100 * 1024 * 1024);
    let concurrency = number("RAIN_BENCH_CONCURRENCY", 1) as usize;
    let variant = std::env::var("RAIN_BENCH_VARIANT").unwrap_or_else(|_| "plain".into());
    assert!(["plain", "zip", "targz"].contains(&variant.as_str()));
    let name = match variant.as_str() {
        "zip" => "fixture.zip",
        "targz" => "fixture.tar.gz",
        _ => "fixture.log",
    };
    let warmup = number("RAIN_BENCH_WARMUP", 0) as usize;
    let queries = number("RAIN_BENCH_QUERIES", 20) as usize;
    assert!(bytes > 0 && queries > 0 && [1, 2, 4].contains(&concurrency));
    let iterations = number("RAIN_BENCH_ITERATIONS", 1);
    assert!(iterations > 0, "RAIN_BENCH_ITERATIONS must be positive");
    for iteration in 0..iterations {
        let dir = fixture::TestDir::new();
        let db_path = dir.0.join("rain.db");
        let pool = db::init_pool(&format!(
            "sqlite://{}?mode=rwc",
            db_path.to_string_lossy().replace('\\', "/")
        ))
        .unwrap();
        let metrics = Metrics::default();
        let subscriber = tracing_subscriber::registry().with(metrics.clone());
        let outcome = std::panic::AssertUnwindSafe(async {
            db::prepare_schema(&pool, true).await.unwrap();
            let data_root = dir.0.join("uploads");
            fs::create_dir_all(&data_root).unwrap();
            let mut limits = AppLimits::default();
            // Whole-line rounding must not accidentally reject the 4 x 1 GiB workload.
            limits.issue_max_content_size = limits.issue_max_content_size.max(bytes.checked_add(256).unwrap().checked_mul(concurrency as u64).unwrap());
            let blob_store = Arc::new(LocalCasBlobStore::new(data_root.clone()));
            let user = match users::create_user(&pool, "benchmark", "unused-password-hash").await.unwrap() {
                users::CreateUserOutcome::Created(user) => user,
                _ => panic!("duplicate benchmark user"),
            };
            sqlx::query("INSERT INTO issues (code, name, owner_user_id) VALUES ('BENCH', 'Benchmark', ?)").bind(&user.id).execute(&pool).await.unwrap();
            let token = generate_session_token();
            sessions::create_session(&pool, &user.id, &hash_session_token(&token), chrono::Utc::now() + chrono::Duration::hours(24), None, None).await.unwrap();
            let cookie = Cookie::new(SESSION_COOKIE_NAME, token);
            let mut inputs = Vec::new();
            for index in 0..concurrency {
                let path = dir.0.join(format!("source-{index}.log"));
                let metadata = fixture::generate(&path, bytes, index);
                let path = fixture::package(&path, &variant);
                let uploaded_bytes = size(&path);
                let uploaded_sha256 = fixture::file_sha256(&path);
                let id = format!("bench-{index}");
                create_processing_bundle(&pool, &id, "BENCH", &id, name, uploaded_bytes, Some(&user.id)).await.unwrap();
                inputs.push((id, path, metadata, uploaded_bytes, uploaded_sha256));
            }
            let selected_backend = SearchBackendKind::parse(
                std::env::var("RAIN_SEARCH_BACKEND").ok().as_deref(),
            )
            .unwrap();
            let search_resource_budget = SearchResourceBudget::new(
                limits.search.tantivy_max_writers,
                limits.search.tantivy_writer_heap_size,
            )
            .unwrap();
            let search_temp_root = dir.0.join(".search-tmp");
            fs::create_dir_all(&search_temp_root).unwrap();
            let search_builds = join_all(inputs.iter().map(|(id, _, _, _, _)| {
                begin_bench_build(
                    selected_backend,
                    &pool,
                    &data_root,
                    &search_temp_root,
                    id,
                    search_resource_budget.clone(),
                )
            }))
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
            metrics.0.lock().unwrap().clear();
            let before = resources(&db_path);
            let sampler = Sampler::new(db_path.clone());
            let started = Instant::now();
            let times = join_all(inputs.iter().enumerate().map(|(input_index, (id, path, _, uploaded_bytes, _))| {
                let pool = &pool; let data_root = &data_root; let limits = &limits; let blob_store = blob_store.clone();
                let search_build = clone_bench_build(&search_builds[input_index]);
                async move {
                    let start = Instant::now();
                    process_uploaded_file(ProcessFileOptions {
                        pool, bundle_id: id, bundle_hash: id, data_root, blob_store,
                        storage_name: name, original_name: name, display_name: name,
                        content_type: None, source_path: path, size_bytes: *uploaded_bytes,
                        archive_budget: ArchiveBudget::new(ArchiveConfig::for_content_limit_with_working_size(limits.issue_max_content_size, limits.archive_max_working_size)),
                        issue_quota: IssueQuota::new(pool.clone(), "BENCH", id, limits.issue_max_content_size), indexing: &limits.indexing, search_index: bench_index(&search_build),
                        preflighted: false,
                    }).await.unwrap();
                    finish_bench_build(&search_build).await.unwrap();
                    finalize_bundle_ready_with_retry(pool, id).await.unwrap();
                    json!({"bundle": id, "ingest_to_ready_ms": start.elapsed().as_secs_f64() * 1000.0})
                }
            })).await;
            let ingest_ms = started.elapsed().as_secs_f64() * 1000.0;
            let samples = sampler.finish();
            let ingest_metrics = metrics.0.lock().unwrap().clone();
            let after_ingest = resources(&db_path);
            let ready: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bundles WHERE status = 'READY'").fetch_one(&pool).await.unwrap();
            assert_eq!(ready, concurrency as i64);
            let app = test::init_service(App::new().app_data(web::Data::new(AppState::new(pool.clone(), data_root, limits.clone()))).configure(routes::register)).await;
            let mut search = Vec::new();
            for (kind, term, encoded) in [
                ("common", "INFO", "INFO"), ("rare", "RARE_SENTINEL", "RARE_SENTINEL"),
                ("uuid", "550e8400-e29b-41d4-a716-446655440000", "550e8400-e29b-41d4-a716-446655440000"),
                ("chinese", "中文连续文本", "%E4%B8%AD%E6%96%87%E8%BF%9E%E7%BB%AD%E6%96%87%E6%9C%AC"),
            ] {
                for (scope, base) in [("bundle", "/api/log/v2/bench-0/search"), ("issue", "/api/issues/BENCH/search")] {
                    let mut latency = Vec::new(); let mut totals = Vec::new();
                    for sample in 0..queries.checked_add(warmup).unwrap() {
                        let start = Instant::now();
                        let response = test::call_service(&app, test::TestRequest::get().uri(&format!("{base}?q={encoded}&size=10")).cookie(cookie.clone()).to_request()).await;
                        assert!(response.status().is_success(), "{kind}/{scope}: {}", response.status());
                        let body: Value = test::read_body_json(response).await;
                        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                        assert!(body["hits"].as_array().is_some_and(|h| !h.is_empty()), "missing fixture matches: {body}");
                        if sample >= warmup { latency.push(elapsed); totals.push(body["total"].clone()); }
                    }
                    search.push(json!({"kind": kind, "term": term, "scope": scope, "samples_ms": latency, "percentiles": percentiles(&latency), "totals": totals}));
                }
            }
            let sampled_peaks = ["rss_bytes", "db_bytes", "wal_bytes"].into_iter().map(|key| (key.to_owned(), json!(samples.iter().filter_map(|s| s[key].as_u64()).max()))).collect::<serde_json::Map<_, _>>();
            json!({"schema_version": 1, "iteration": iteration, "timestamp": chrono::Utc::now().to_rfc3339(),
                "machine": {"host_notes": std::env::var("RAIN_BENCH_HOST_NOTES").ok(), "filesystem": if cfg!(target_os = "linux") { command("df", &["-T", dir.0.to_str().unwrap()]) } else { None }, "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "clock_ticks_per_second": if cfg!(target_os = "linux") { command("getconf", &["CLK_TCK"]).and_then(|s| s.parse::<u64>().ok()) } else { None }, "logical_cpus": std::thread::available_parallelism().ok().map(|v| v.get()), "uname": command("uname", &["-a"]), "cpu": fs::read_to_string("/proc/cpuinfo").ok().and_then(|s| s.lines().find(|l| l.starts_with("model name")).map(str::to_owned)), "memory": fs::read_to_string("/proc/meminfo").ok().and_then(|s| s.lines().next().map(str::to_owned))},
                "build": {"git_commit": command("git", &["rev-parse", "HEAD"]), "git_status": command("git", &["status", "--porcelain"]), "rustc": command("rustc", &["-Vv"]), "debug_assertions": cfg!(debug_assertions), "package_version": env!("CARGO_PKG_VERSION")},
                "config": {"bytes_per_bundle_minimum": bytes, "concurrency": concurrency, "query_samples": queries, "query_warmup": warmup, "tantivy_max_writers": limits.search.tantivy_max_writers, "tantivy_writer_heap_size_bytes": limits.search.tantivy_writer_heap_size, "limits": format!("{limits:?}"), "archive_limits": format!("{:?}", ArchiveConfig::for_content_limit_with_working_size(limits.issue_max_content_size, limits.archive_max_working_size)), "fixture_version": 1, "variant": variant, "sampler_interval_ms": 100},
                "boundaries": {"receive_included": false, "fixture_generation_included": false, "cache_state": "fresh database; generated and hashed inputs may be cached; OS caches not flushed", "queries": "authenticated in-process HTTP handlers, including body decoding; serial, configured warmup excluded, no cache flush", "resource_scope": "entire test process; samples cover ingest only; Linux IO counters cumulative since process start"},
                "inputs": inputs.iter().map(|(id, _, metadata, uploaded_bytes, uploaded_sha256)| json!({"bundle": id, "fixture": metadata, "uploaded_bytes": uploaded_bytes, "uploaded_sha256": uploaded_sha256})).collect::<Vec<_>>(),
                "throughput": {"raw_mib_per_second": inputs.iter().map(|i| i.2.bytes).sum::<u64>() as f64 / 1048576.0 / (ingest_ms / 1000.0), "lines_per_second": inputs.iter().map(|i| i.2.lines).sum::<u64>() as f64 / (ingest_ms / 1000.0)},
                "ingest_sampled_peaks": sampled_peaks,
                "ingest_to_all_ready_ms": ingest_ms, "bundle_timings": times, "resources_before": before, "resources_after": resources(&db_path), "ingest_resource_samples": samples, "search": search, "stage_metrics": ingest_metrics, "resources_after_ingest": after_ingest})
        }.with_subscriber(subscriber)).catch_unwind().await;
        pool.close().await;
        let report = outcome.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        let line = serde_json::to_string(&report).unwrap();
        println!("{line}");
        if let Ok(path) = std::env::var("RAIN_BENCH_REPORT") {
            let mut output = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(output, "{line}").unwrap();
        }
    }
}
