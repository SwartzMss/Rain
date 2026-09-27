use std::{future::Future, sync::Arc};

use futures_util::{StreamExt, stream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::AppError;

/// Keep Issue fan-out below the process-wide query budget while allowing
/// large Issues to make progress on more than two generations at a time.
pub(crate) const MAX_BUNDLES_PER_ISSUE_SEARCH: usize = 4;

/// Select the per-request fan-out from the number of Tantivy Bundles.
///
/// The thresholds are deliberately conservative: a small Issue should not
/// consume the whole process budget, while a large Issue should not wait for
/// every Bundle to pass through a two-wide queue. The global semaphore remains
/// the final admission guard for concurrent requests and searches.
pub(crate) fn issue_search_parallelism(bundle_count: usize) -> usize {
    match bundle_count {
        0 | 1 => 1,
        2..=8 => 2,
        9..=32 => 3,
        _ => MAX_BUNDLES_PER_ISSUE_SEARCH,
    }
}

pub(crate) async fn run_issue_bundle_searches<I, F, Fut, T>(
    jobs: I,
    global_permits: Arc<Semaphore>,
    operation: F,
) -> Vec<Result<T, AppError>>
where
    I: IntoIterator,
    I::Item: Send + 'static,
    F: Fn(I::Item, OwnedSemaphorePermit) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<T, AppError>> + Send + 'static,
    T: Send + 'static,
{
    let operation = Arc::new(operation);
    let jobs = jobs.into_iter().collect::<Vec<_>>();
    let parallelism = issue_search_parallelism(jobs.len());
    let mut results: Vec<(usize, Result<T, AppError>)> = stream::iter(jobs.into_iter().enumerate())
        .map(move |(index, job)| {
            let operation = operation.clone();
            let global_permits = global_permits.clone();
            async move {
                let result = match global_permits.acquire_owned().await {
                    Ok(permit) => operation(job, permit).await,
                    Err(_) => Err(AppError::Conflict(
                        "Tantivy query admission is shutting down".into(),
                    )),
                };
                (index, result)
            }
        })
        .buffer_unordered(parallelism)
        .collect()
        .await;
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;

    use super::{issue_search_parallelism, run_issue_bundle_searches};

    #[test]
    fn issue_search_parallelism_scales_only_with_bundle_count() {
        assert_eq!(issue_search_parallelism(0), 1);
        assert_eq!(issue_search_parallelism(1), 1);
        assert_eq!(issue_search_parallelism(5), 2);
        assert_eq!(issue_search_parallelism(20), 3);
        assert_eq!(issue_search_parallelism(50), 4);
        assert_eq!(issue_search_parallelism(usize::MAX), 4);
    }

    #[tokio::test]
    async fn issue_search_respects_adaptive_bundle_fan_out() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let permits = Arc::new(tokio::sync::Semaphore::new(4));
        let jobs = (0..50).collect::<Vec<_>>();
        let results = run_issue_bundle_searches(jobs, permits, {
            let active = active.clone();
            let peak = peak.clone();
            move |job, _permit| {
                let active = active.clone();
                let peak = peak.clone();
                async move {
                    let current = active.fetch_add(1, Ordering::AcqRel) + 1;
                    peak.fetch_max(current, Ordering::AcqRel);
                    let _ = job;
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    active.fetch_sub(1, Ordering::AcqRel);
                    Ok::<_, crate::error::AppError>(job)
                }
            }
        })
        .await;

        assert_eq!(results.len(), 50);
        assert_eq!(
            results
                .into_iter()
                .map(|result| result.unwrap())
                .collect::<Vec<_>>(),
            (0..50).collect::<Vec<_>>()
        );
        assert!(peak.load(Ordering::Acquire) <= 4);
    }
}
