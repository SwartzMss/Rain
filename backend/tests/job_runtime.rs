use std::{future::pending, time::Duration};

use backend::job_runtime::{JobRuntime, JobStatus, JobType, SubmitError};

#[tokio::test]
async fn completed_job_updates_status_and_metrics_once() {
    let runtime = JobRuntime::new();
    let handle = runtime
        .spawn(JobType::Search, |_context| async { Ok::<_, String>(()) })
        .expect("job accepted");
    let id = handle.id();

    assert_eq!(handle.await.expect("worker result").expect("success"), ());
    let snapshot = runtime.snapshot();
    assert_eq!(runtime.status(id), Some(JobStatus::Completed));
    assert_eq!(snapshot.for_type(JobType::Search).completed_total, 1);
    assert_eq!(snapshot.for_type(JobType::Search).active, 0);
}

#[tokio::test]
async fn shutdown_rejects_new_jobs_and_cancels_running_jobs() {
    let runtime = JobRuntime::new();
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped_ref = stopped.clone();
    let handle = runtime
        .spawn(JobType::Upload, move |context| async move {
            context.cancelled().await;
            stopped_ref.store(true, std::sync::atomic::Ordering::Release);
            Ok::<_, String>(())
        })
        .expect("job accepted");
    let id = handle.id();

    runtime.shutdown(Duration::from_millis(100)).await;
    handle
        .await
        .expect("worker result")
        .expect("cooperative stop");
    assert!(stopped.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(runtime.status(id), Some(JobStatus::Cancelled));
    assert_eq!(
        runtime.snapshot().for_type(JobType::Upload).cancelled_total,
        1
    );
    assert!(matches!(
        runtime.spawn(JobType::Cleanup, |_context| async { Ok::<_, String>(()) }),
        Err(SubmitError::Closed)
    ));
}

#[tokio::test]
async fn task_error_is_observable_and_terminal() {
    let runtime = JobRuntime::new();
    let handle = runtime
        .spawn(JobType::Cleanup, |_context| async {
            Err::<(), _>("cleanup failed".to_owned())
        })
        .expect("job accepted");
    let id = handle.id();

    let error = handle.await.expect("worker result").expect_err("failure");
    assert_eq!(error, "cleanup failed");
    let metrics = runtime.snapshot().for_type(JobType::Cleanup);
    assert_eq!(metrics.failed_total, 1);
    assert_eq!(runtime.status(id), Some(JobStatus::Failed));
}

#[tokio::test]
async fn timeout_requests_cooperative_stop_and_records_timeout() {
    let runtime = JobRuntime::new();
    let handle = runtime
        .spawn_with_timeout(
            JobType::Search,
            Some(Duration::from_millis(5)),
            |context| async move {
                context.cancelled().await;
                Ok::<_, String>(())
            },
        )
        .expect("job accepted");
    let id = handle.id();

    handle
        .await
        .expect("worker result")
        .expect("cooperative stop");
    assert_eq!(runtime.status(id), Some(JobStatus::TimedOut));
    assert_eq!(
        runtime.snapshot().for_type(JobType::Search).timed_out_total,
        1
    );
}

#[tokio::test]
async fn forced_shutdown_records_aborted_jobs_without_leaking_active_metrics() {
    let runtime = JobRuntime::new();
    let handle = runtime
        .spawn(JobType::Cleanup, |_context| async {
            pending::<Result<(), String>>().await
        })
        .expect("job accepted");
    let id = handle.id();

    let report = runtime.shutdown(Duration::ZERO).await;

    assert!(!report.graceful);
    assert_eq!(runtime.status(id), Some(JobStatus::Failed));
    let metrics = runtime.snapshot().for_type(JobType::Cleanup);
    assert_eq!(metrics.failed_total, 1);
    assert_eq!(metrics.active, 0);
    assert!(handle.await.is_err());
}
