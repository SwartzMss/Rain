use std::{future::Future, sync::Arc};

use futures_util::{StreamExt, stream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::AppError;

pub(crate) const MAX_BUNDLES_PER_ISSUE_SEARCH: usize = 2;

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
        .buffer_unordered(MAX_BUNDLES_PER_ISSUE_SEARCH)
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

    use super::run_issue_bundle_searches;

    #[tokio::test]
    async fn issue_search_keeps_at_most_two_bundle_queries_in_flight() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let permits = Arc::new(tokio::sync::Semaphore::new(4));
        let jobs = (0..8).collect::<Vec<_>>();
        let results = run_issue_bundle_searches(jobs, permits, {
            let active = active.clone();
            let peak = peak.clone();
            move |job, _permit| {
                let active = active.clone();
                let peak = peak.clone();
                async move {
                    let current = active.fetch_add(1, Ordering::AcqRel) + 1;
                    peak.fetch_max(current, Ordering::AcqRel);
                    tokio::time::sleep(Duration::from_millis((8 - job) as u64)).await;
                    active.fetch_sub(1, Ordering::AcqRel);
                    Ok::<_, crate::error::AppError>(job)
                }
            }
        })
        .await;

        assert_eq!(results.len(), 8);
        assert_eq!(
            results
                .into_iter()
                .map(|result| result.unwrap())
                .collect::<Vec<_>>(),
            (0..8).collect::<Vec<_>>()
        );
        assert!(peak.load(Ordering::Acquire) <= 2);
    }
}
