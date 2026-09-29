use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, Instant},
};

use chrono::Utc;
use tokio::{
    sync::{Notify, oneshot},
    task::AbortHandle,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum JobType {
    Upload,
    Index,
    Search,
    Materialize,
    Cleanup,
}

impl JobType {
    pub const ALL: [Self; 5] = [
        Self::Upload,
        Self::Index,
        Self::Search,
        Self::Materialize,
        Self::Cleanup,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Index => "index",
            Self::Search => "search",
            Self::Materialize => "materialize",
            Self::Cleanup => "cleanup",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobStatus {
    Pending,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopReason {
    Cancelled,
    TimedOut,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmitError {
    Closed,
}

#[derive(Clone, Debug, Default)]
pub struct JobRuntimeSnapshot {
    metrics: HashMap<JobType, JobMetricsOwned>,
}

#[derive(Clone, Debug, Default)]
struct JobMetricsOwned {
    active: u64,
    queued: u64,
    completed_total: u64,
    failed_total: u64,
    cancelled_total: u64,
    timed_out_total: u64,
    duration_count: u64,
    duration_sum_ms: u128,
    last_duration_ms: Option<u128>,
    last_success_at: Option<String>,
}

impl JobRuntimeSnapshot {
    pub fn for_type(&self, kind: JobType) -> JobMetricsView {
        self.metrics.get(&kind).cloned().unwrap_or_default().into()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JobMetricsView {
    pub active: u64,
    pub queued: u64,
    pub completed_total: u64,
    pub failed_total: u64,
    pub cancelled_total: u64,
    pub timed_out_total: u64,
    pub duration_count: u64,
    pub duration_sum_ms: u128,
    pub last_duration_ms: Option<u128>,
    pub last_success_at: Option<String>,
}

impl From<JobMetricsOwned> for JobMetricsView {
    fn from(metrics: JobMetricsOwned) -> Self {
        Self {
            active: metrics.active,
            queued: metrics.queued,
            completed_total: metrics.completed_total,
            failed_total: metrics.failed_total,
            cancelled_total: metrics.cancelled_total,
            timed_out_total: metrics.timed_out_total,
            duration_count: metrics.duration_count,
            duration_sum_ms: metrics.duration_sum_ms,
            last_duration_ms: metrics.last_duration_ms,
            last_success_at: metrics.last_success_at,
        }
    }
}

#[derive(Clone)]
pub struct JobContext {
    id: Uuid,
    kind: JobType,
    token: CancellationToken,
    deadline: Option<Instant>,
}

impl JobContext {
    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn kind(&self) -> JobType {
        self.kind
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    pub async fn cancelled(&self) {
        self.token.cancelled().await;
    }

    pub fn checkpoint(&self) -> Result<(), StopReason> {
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(StopReason::TimedOut);
        }
        if self.token.is_cancelled() {
            return Err(StopReason::Cancelled);
        }
        Ok(())
    }

    pub fn remaining(&self) -> Option<Duration> {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }
}

pub struct JobHandle<T, E> {
    id: Uuid,
    receiver: oneshot::Receiver<Result<T, E>>,
}

impl<T, E> JobHandle<T, E> {
    pub fn id(&self) -> Uuid {
        self.id
    }
}

impl<T, E> Future for JobHandle<T, E> {
    type Output = Result<Result<T, E>, oneshot::error::RecvError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        Pin::new(&mut this.receiver).poll(cx)
    }
}

struct JobEntry {
    kind: JobType,
    scheduler: bool,
    status: JobStatus,
    token: CancellationToken,
    stop_reason: Option<StopReason>,
    started_at: Option<Instant>,
    abort: Option<AbortHandle>,
    terminal_at: Option<Instant>,
}

struct CompletionGuard {
    runtime: JobRuntime,
    id: Uuid,
    armed: bool,
}

impl CompletionGuard {
    fn new(runtime: JobRuntime, id: Uuid) -> Self {
        Self {
            runtime,
            id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        if self.armed {
            self.runtime.finish_aborted(self.id);
        }
    }
}

struct RuntimeState {
    accepting: bool,
    jobs: HashMap<Uuid, JobEntry>,
    metrics: HashMap<JobType, JobMetricsOwned>,
}

const TERMINAL_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_TERMINAL_JOBS: usize = 1024;

struct RuntimeInner {
    state: Mutex<RuntimeState>,
    changed: Notify,
    executor: Option<tokio::runtime::Handle>,
}

#[derive(Clone)]
pub struct JobRuntime {
    inner: Arc<RuntimeInner>,
}

impl Default for JobRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl JobRuntime {
    pub fn new() -> Self {
        let metrics = JobType::ALL
            .into_iter()
            .map(|kind| (kind, JobMetricsOwned::default()))
            .collect();
        Self {
            inner: Arc::new(RuntimeInner {
                state: Mutex::new(RuntimeState {
                    accepting: true,
                    jobs: HashMap::new(),
                    metrics,
                }),
                changed: Notify::new(),
                executor: tokio::runtime::Handle::try_current().ok(),
            }),
        }
    }

    pub fn is_accepting(&self) -> bool {
        self.inner
            .state
            .lock()
            .map(|state| state.accepting)
            .unwrap_or(false)
    }

    pub fn spawn<T, E, F, Fut>(
        &self,
        kind: JobType,
        factory: F,
    ) -> Result<JobHandle<T, E>, SubmitError>
    where
        T: Send + 'static,
        E: Send + 'static,
        F: FnOnce(JobContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.spawn_with_options(kind, None, false, factory)
    }

    pub fn spawn_with_timeout<T, E, F, Fut>(
        &self,
        kind: JobType,
        timeout: Option<Duration>,
        factory: F,
    ) -> Result<JobHandle<T, E>, SubmitError>
    where
        T: Send + 'static,
        E: Send + 'static,
        F: FnOnce(JobContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.spawn_with_options(kind, timeout, false, factory)
    }

    fn spawn_with_options<T, E, F, Fut>(
        &self,
        kind: JobType,
        timeout: Option<Duration>,
        scheduler: bool,
        factory: F,
    ) -> Result<JobHandle<T, E>, SubmitError>
    where
        T: Send + 'static,
        E: Send + 'static,
        F: FnOnce(JobContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        let id = Uuid::new_v4();
        let token = CancellationToken::new();
        let context = JobContext {
            id,
            kind,
            token: token.clone(),
            deadline: timeout.map(|duration| Instant::now() + duration),
        };
        let (sender, receiver) = oneshot::channel();
        {
            let mut state = self.inner.state.lock().map_err(|_| SubmitError::Closed)?;
            Self::prune_locked(&mut state);
            if !state.accepting {
                return Err(SubmitError::Closed);
            }
            state.jobs.insert(
                id,
                JobEntry {
                    kind,
                    scheduler,
                    status: JobStatus::Pending,
                    token,
                    stop_reason: None,
                    started_at: None,
                    abort: None,
                    terminal_at: None,
                },
            );
            if !scheduler {
                state.metrics.entry(kind).or_default().queued += 1;
            }
        }

        let runtime = self.clone();
        let task = self.spawn_future(async move {
            runtime.mark_running(id);
            let mut completion_guard = CompletionGuard::new(runtime.clone(), id);
            let future = factory(context.clone());
            tokio::pin!(future);
            let outcome = if let Some(timeout) = timeout {
                tokio::select! {
                    result = &mut future => result,
                    _ = tokio::time::sleep(timeout) => {
                        runtime.request_stop(id, StopReason::TimedOut);
                        future.await
                    }
                }
            } else {
                future.await
            };
            let status = runtime.finish(id, outcome.is_ok());
            tracing::info!(
                job_id = %id,
                job_type = kind.as_str(),
                status = ?status,
                "background job finished"
            );
            completion_guard.disarm();
            let _ = sender.send(outcome);
        });
        if let Ok(mut state) = self.inner.state.lock()
            && let Some(entry) = state.jobs.get_mut(&id)
        {
            entry.abort = Some(task.abort_handle());
        }
        Ok(JobHandle { id, receiver })
    }

    pub fn spawn_periodic_job<F, Fut>(
        &self,
        kind: JobType,
        name: &'static str,
        initial_delay: Duration,
        interval_duration: Duration,
        job: F,
    ) -> Result<tokio::task::JoinHandle<()>, SubmitError>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let runtime_for_ticks = self.clone();
        let handle = self.spawn_with_options(kind, None, true, move |context| async move {
            tokio::select! {
                _ = context.cancelled() => return Ok::<(), String>(()),
                _ = tokio::time::sleep(initial_delay) => {}
            }
            let mut interval = tokio::time::interval(interval_duration);
            loop {
                tokio::select! {
                    _ = context.cancelled() => break,
                    _ = interval.tick() => {
                        let started = Instant::now();
                        let result = job().await;
                        runtime_for_ticks.record_periodic_result(kind, started.elapsed(), result.is_ok());
                        match result {
                            Ok(()) => tracing::debug!(job = name, elapsed_ms = started.elapsed().as_millis(), "periodic job completed"),
                            Err(error) => tracing::warn!(job = name, elapsed_ms = started.elapsed().as_millis(), %error, "periodic job failed; will retry"),
                        }
                    }
                }
            }
            Ok::<(), String>(())
        })?;
        Ok(self.spawn_future(async move {
            let _ = handle.await;
        }))
    }

    fn spawn_future<F>(&self, future: F) -> tokio::task::JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        match self.inner.executor.clone() {
            Some(executor) => executor.spawn(future),
            None => tokio::spawn(future),
        }
    }

    fn record_periodic_result(&self, kind: JobType, duration: Duration, succeeded: bool) {
        if let Ok(mut state) = self.inner.state.lock()
            && let Some(metrics) = state.metrics.get_mut(&kind)
        {
            if succeeded {
                metrics.completed_total += 1;
                metrics.last_success_at = Some(Utc::now().to_rfc3339());
            } else {
                metrics.failed_total += 1;
            }
            let millis = duration.as_millis();
            metrics.duration_count += 1;
            metrics.duration_sum_ms += millis;
            metrics.last_duration_ms = Some(millis);
        }
    }

    pub fn cancel(&self, id: Uuid) -> bool {
        self.request_stop(id, StopReason::Cancelled)
    }

    pub fn stop(&self, id: Uuid, reason: StopReason) -> bool {
        self.request_stop(id, reason)
    }

    pub fn status(&self, id: Uuid) -> Option<JobStatus> {
        let mut state = self.inner.state.lock().ok()?;
        Self::prune_locked(&mut state);
        state.jobs.get(&id).map(|entry| entry.status)
    }

    pub fn snapshot(&self) -> JobRuntimeSnapshot {
        let metrics = self
            .inner
            .state
            .lock()
            .map(|mut state| {
                Self::prune_locked(&mut state);
                state.metrics.clone()
            })
            .unwrap_or_default();
        JobRuntimeSnapshot { metrics }
    }

    pub async fn shutdown(&self, grace: Duration) -> ShutdownReport {
        let deadline = Instant::now() + grace;
        let (tokens, aborts) = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.accepting = false;
            let mut tokens = Vec::new();
            let mut aborts = Vec::new();
            let mut pending_by_kind = HashMap::<JobType, u64>::new();
            for entry in state.jobs.values_mut() {
                if matches!(entry.status, JobStatus::Pending | JobStatus::Running) {
                    let was_pending = entry.status == JobStatus::Pending;
                    let kind = entry.kind;
                    entry.status = JobStatus::Cancelling;
                    entry.stop_reason = Some(StopReason::Shutdown);
                    if was_pending {
                        *pending_by_kind.entry(kind).or_default() += 1;
                    }
                    tokens.push(entry.token.clone());
                }
                if let Some(abort) = entry.abort.clone() {
                    aborts.push(abort);
                }
            }
            for (kind, count) in pending_by_kind {
                if let Some(metrics) = state.metrics.get_mut(&kind) {
                    metrics.queued = metrics.queued.saturating_sub(count);
                }
            }
            (tokens, aborts)
        };
        for token in tokens {
            token.cancel();
        }
        self.inner.changed.notify_waiters();

        while Instant::now() < deadline {
            if self.active_count() == 0 {
                return ShutdownReport {
                    graceful: true,
                    outstanding: 0,
                };
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let _ = tokio::time::timeout(
                remaining.min(Duration::from_millis(20)),
                self.inner.changed.notified(),
            )
            .await;
        }
        let forced = self.active_count() > 0;
        for abort in aborts {
            abort.abort();
        }
        tokio::task::yield_now().await;
        let outstanding = self.active_count();
        self.force_finish_outstanding();
        ShutdownReport {
            graceful: !forced && outstanding == 0,
            outstanding,
        }
    }

    fn request_stop(&self, id: Uuid, reason: StopReason) -> bool {
        let (token, was_pending, kind) = {
            let Ok(mut state) = self.inner.state.lock() else {
                return false;
            };
            let Some(entry) = state.jobs.get_mut(&id) else {
                return false;
            };
            if matches!(
                entry.status,
                JobStatus::Completed
                    | JobStatus::Failed
                    | JobStatus::Cancelled
                    | JobStatus::TimedOut
            ) {
                return false;
            }
            let was_pending = entry.status == JobStatus::Pending;
            let kind = entry.kind;
            entry.stop_reason = Some(reason);
            entry.status = JobStatus::Cancelling;
            (entry.token.clone(), was_pending, kind)
        };
        if was_pending
            && let Ok(mut state) = self.inner.state.lock()
            && let Some(metrics) = state.metrics.get_mut(&kind)
        {
            metrics.queued = metrics.queued.saturating_sub(1);
        }
        token.cancel();
        self.inner.changed.notify_waiters();
        true
    }

    fn mark_running(&self, id: Uuid) {
        if let Ok(mut state) = self.inner.state.lock() {
            let Some((kind, scheduler)) = state
                .jobs
                .get(&id)
                .map(|entry| (entry.kind, entry.scheduler))
            else {
                return;
            };
            if state.jobs.get(&id).is_some_and(|entry| {
                matches!(
                    entry.status,
                    JobStatus::Completed
                        | JobStatus::Failed
                        | JobStatus::Cancelled
                        | JobStatus::TimedOut
                )
            }) {
                return;
            }
            if let Some(entry) = state.jobs.get_mut(&id) {
                if entry.status == JobStatus::Pending {
                    entry.status = JobStatus::Running;
                }
                entry.started_at = Some(Instant::now());
            }
            if !scheduler && let Some(metrics) = state.metrics.get_mut(&kind) {
                metrics.queued = metrics.queued.saturating_sub(1);
                metrics.active += 1;
            }
        }
        self.inner.changed.notify_waiters();
    }

    fn finish(&self, id: Uuid, succeeded: bool) -> JobStatus {
        let (status, kind, duration, scheduler) = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(entry) = state.jobs.get_mut(&id) else {
                return JobStatus::Failed;
            };
            if matches!(
                entry.status,
                JobStatus::Completed
                    | JobStatus::Failed
                    | JobStatus::Cancelled
                    | JobStatus::TimedOut
            ) {
                return entry.status;
            }
            let status = match entry.stop_reason {
                Some(StopReason::TimedOut) => JobStatus::TimedOut,
                Some(StopReason::Cancelled | StopReason::Shutdown) => JobStatus::Cancelled,
                None if succeeded => JobStatus::Completed,
                None => JobStatus::Failed,
            };
            let duration = entry.started_at.map(|started| started.elapsed());
            entry.status = status;
            entry.terminal_at = Some(Instant::now());
            (status, entry.kind, duration, entry.scheduler)
        };
        if !scheduler
            && let Ok(mut state) = self.inner.state.lock()
            && let Some(metrics) = state.metrics.get_mut(&kind)
        {
            metrics.active = metrics.active.saturating_sub(1);
            match status {
                JobStatus::Completed => {
                    metrics.completed_total += 1;
                    metrics.last_success_at = Some(Utc::now().to_rfc3339());
                }
                JobStatus::Failed => metrics.failed_total += 1,
                JobStatus::Cancelled => metrics.cancelled_total += 1,
                JobStatus::TimedOut => metrics.timed_out_total += 1,
                JobStatus::Pending | JobStatus::Running | JobStatus::Cancelling => {}
            }
            if let Some(duration) = duration {
                let millis = duration.as_millis();
                metrics.duration_count += 1;
                metrics.duration_sum_ms += millis;
                metrics.last_duration_ms = Some(millis);
            }
        }
        self.inner.changed.notify_waiters();
        status
    }

    fn finish_aborted(&self, id: Uuid) {
        let (kind, duration, was_pending, scheduler) = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(entry) = state.jobs.get_mut(&id) else {
                return;
            };
            if matches!(
                entry.status,
                JobStatus::Completed
                    | JobStatus::Failed
                    | JobStatus::Cancelled
                    | JobStatus::TimedOut
            ) {
                return;
            }
            let was_pending = entry.started_at.is_none();
            let duration = entry.started_at.map(|started| started.elapsed());
            entry.status = JobStatus::Failed;
            entry.terminal_at = Some(Instant::now());
            (entry.kind, duration, was_pending, entry.scheduler)
        };
        if !scheduler
            && let Ok(mut state) = self.inner.state.lock()
            && let Some(metrics) = state.metrics.get_mut(&kind)
        {
            if was_pending {
                metrics.queued = metrics.queued.saturating_sub(1);
            } else {
                metrics.active = metrics.active.saturating_sub(1);
            }
            metrics.failed_total += 1;
            if let Some(duration) = duration {
                let millis = duration.as_millis();
                metrics.duration_count += 1;
                metrics.duration_sum_ms += millis;
                metrics.last_duration_ms = Some(millis);
            }
        }
        tracing::error!(job_id = %id, "background job was aborted before it reached a terminal result");
        self.inner.changed.notify_waiters();
    }

    fn force_finish_outstanding(&self) {
        let ids = self
            .inner
            .state
            .lock()
            .map(|state| {
                state
                    .jobs
                    .iter()
                    .filter_map(|(id, entry)| {
                        (!matches!(
                            entry.status,
                            JobStatus::Completed
                                | JobStatus::Failed
                                | JobStatus::Cancelled
                                | JobStatus::TimedOut
                        ))
                        .then_some(*id)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for id in ids {
            self.finish_aborted(id);
        }
    }

    fn active_count(&self) -> usize {
        self.inner
            .state
            .lock()
            .map(|state| {
                state
                    .jobs
                    .values()
                    .filter(|entry| {
                        matches!(
                            entry.status,
                            JobStatus::Pending | JobStatus::Running | JobStatus::Cancelling
                        )
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    fn prune_locked(state: &mut RuntimeState) {
        let now = Instant::now();
        state.jobs.retain(|_, entry| {
            entry
                .terminal_at
                .is_none_or(|terminal_at| now.duration_since(terminal_at) < TERMINAL_TTL)
        });
        let mut terminals = state
            .jobs
            .iter()
            .filter_map(|(id, entry)| entry.terminal_at.map(|at| (*id, at)))
            .collect::<Vec<_>>();
        terminals.sort_by_key(|(_, at)| *at);
        while terminals.len() > MAX_TERMINAL_JOBS {
            if let Some((id, _)) = terminals.first().copied() {
                state.jobs.remove(&id);
                terminals.remove(0);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShutdownReport {
    pub graceful: bool,
    pub outstanding: usize,
}
