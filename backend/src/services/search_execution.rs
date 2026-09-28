use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use crate::error::AppError;

const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalStatus {
    Cancelled,
    TimedOut,
    Completed,
    Failed,
}

impl TerminalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timeout",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Cancelled,
    TimedOut,
}

impl StopReason {
    pub fn terminal_status(self) -> TerminalStatus {
        match self {
            Self::Cancelled => TerminalStatus::Cancelled,
            Self::TimedOut => TerminalStatus::TimedOut,
        }
    }

    pub fn into_error(self) -> AppError {
        match self {
            Self::Cancelled => AppError::api(
                actix_web::http::StatusCode::CONFLICT,
                "SEARCH_CANCELLED",
                "搜索已取消",
            ),
            Self::TimedOut => AppError::public(
                actix_web::http::StatusCode::REQUEST_TIMEOUT,
                "TEMP_RESULT_SCAN_TIMEOUT",
                "临时结果扫描超时",
            ),
        }
    }
}

#[derive(Clone)]
pub struct CancellationToken {
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    notify: Arc<Notify>,
}

impl CancellationToken {
    fn new() -> Self {
        Self {
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Acquire)
    }

    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        self.notify.notified().await;
    }
}

#[derive(Clone)]
pub struct SearchExecutionContext {
    pub search_id: String,
    token: CancellationToken,
    deadline: Instant,
}

impl SearchExecutionContext {
    pub fn checkpoint(&self) -> Result<(), StopReason> {
        if self.token.is_cancelled() {
            return Err(StopReason::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(StopReason::TimedOut);
        }
        Ok(())
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReservationError {
    InvalidId,
    Conflict,
    OwnerBusy,
    Capacity,
    Unauthorized,
    Unavailable,
}

pub enum CancelResult {
    Unknown,
    Cancelling,
    Finishing,
    Terminal(TerminalStatus),
}

pub struct SearchReservation {
    pub search_id: String,
    pub cancel_token: String,
    pub expires_in_ms: u64,
}

struct Entry {
    capability_digest: [u8; 32],
    owner_user_id: Option<String>,
    peer_key: String,
    state: EntryState,
    expires_at: Instant,
    terminal_at: Option<Instant>,
    token: CancellationToken,
    notify: Arc<Notify>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryState {
    Reserved,
    Running,
    Cancelling,
    Committing,
    Terminal(TerminalStatus),
}

struct RegistryInner {
    entries: HashMap<String, Entry>,
}

#[derive(Clone)]
pub struct SearchExecutionRegistry {
    inner: Arc<Mutex<RegistryInner>>,
    max_active: usize,
    max_terminal: usize,
    terminal_ttl: Duration,
}

impl SearchExecutionRegistry {
    pub fn new(max_active: usize, max_terminal: usize, terminal_ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RegistryInner {
                entries: HashMap::new(),
            })),
            max_active: max_active.max(1),
            max_terminal: max_terminal.max(1),
            terminal_ttl,
        }
    }

    pub fn reserve(
        &self,
        search_id: &str,
        owner_user_id: Option<&str>,
        peer_key: &str,
    ) -> Result<SearchReservation, ReservationError> {
        let now = Instant::now();
        let mut inner = self.inner.lock().map_err(|_| ReservationError::Capacity)?;
        self.prune_locked(&mut inner, now);
        if inner.entries.contains_key(search_id) {
            return Err(ReservationError::Conflict);
        }

        let active = inner
            .entries
            .values()
            .filter(|entry| !matches!(entry.state, EntryState::Terminal(_)))
            .count();
        if active >= self.max_active {
            return Err(ReservationError::Capacity);
        }
        let owner_busy = inner.entries.values().any(|entry| {
            if matches!(entry.state, EntryState::Terminal(_)) {
                return false;
            }
            match owner_user_id {
                Some(user_id) => entry.owner_user_id.as_deref() == Some(user_id),
                None => entry.owner_user_id.is_none() && entry.peer_key == peer_key,
            }
        });
        if owner_busy {
            return Err(ReservationError::OwnerBusy);
        }

        let mut capability = [0_u8; 32];
        rand::thread_rng().fill_bytes(&mut capability);
        let cancel_token = URL_SAFE_NO_PAD.encode(capability);
        let capability_digest = digest_capability(&cancel_token);
        let expires_at = now + Duration::from_secs(60);
        inner.entries.insert(
            search_id.to_owned(),
            Entry {
                capability_digest,
                owner_user_id: owner_user_id.map(str::to_owned),
                peer_key: peer_key.to_owned(),
                state: EntryState::Reserved,
                expires_at,
                terminal_at: None,
                token: CancellationToken::new(),
                notify: Arc::new(Notify::new()),
            },
        );
        Ok(SearchReservation {
            search_id: search_id.to_owned(),
            cancel_token,
            expires_in_ms: expires_at
                .saturating_duration_since(now)
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        })
    }

    pub fn start(
        &self,
        search_id: &str,
        cancel_token: &str,
        owner_user_id: Option<&str>,
    ) -> Result<SearchExecutionContext, ReservationError> {
        self.start_with_timeout(
            search_id,
            cancel_token,
            owner_user_id,
            DEFAULT_EXECUTION_TIMEOUT,
        )
    }

    pub fn start_with_timeout(
        &self,
        search_id: &str,
        cancel_token: &str,
        owner_user_id: Option<&str>,
        timeout: Duration,
    ) -> Result<SearchExecutionContext, ReservationError> {
        let now = Instant::now();
        let mut inner = self.inner.lock().map_err(|_| ReservationError::Capacity)?;
        self.prune_locked(&mut inner, now);
        let entry = inner
            .entries
            .get_mut(search_id)
            .ok_or(ReservationError::Unavailable)?;
        if !matches!(entry.state, EntryState::Reserved)
            || now >= entry.expires_at
            || !authorized(entry, cancel_token, owner_user_id)
        {
            return if !authorized(entry, cancel_token, owner_user_id) {
                Err(ReservationError::Unauthorized)
            } else {
                Err(ReservationError::Unavailable)
            };
        }
        entry.state = EntryState::Running;
        Ok(SearchExecutionContext {
            search_id: search_id.to_owned(),
            token: entry.token.clone(),
            deadline: now + timeout,
        })
    }

    pub fn cancel(
        &self,
        search_id: &str,
        cancel_token: &str,
        owner_user_id: Option<&str>,
    ) -> CancelResult {
        let Ok(mut inner) = self.inner.lock() else {
            return CancelResult::Unknown;
        };
        self.prune_locked(&mut inner, Instant::now());
        let Some(entry) = inner.entries.get_mut(search_id) else {
            return CancelResult::Unknown;
        };
        if !authorized(entry, cancel_token, owner_user_id) {
            return CancelResult::Unknown;
        }
        match entry.state {
            EntryState::Reserved => {
                entry.state = EntryState::Terminal(TerminalStatus::Cancelled);
                entry.terminal_at = Some(Instant::now());
                entry.token.cancel();
                entry.notify.notify_waiters();
                CancelResult::Terminal(TerminalStatus::Cancelled)
            }
            EntryState::Running => {
                entry.state = EntryState::Cancelling;
                entry.token.cancel();
                entry.notify.notify_waiters();
                CancelResult::Cancelling
            }
            EntryState::Cancelling => CancelResult::Cancelling,
            EntryState::Committing => CancelResult::Finishing,
            EntryState::Terminal(status) => CancelResult::Terminal(status),
        }
    }

    pub fn mark_committing(
        &self,
        search_id: &str,
        context: &SearchExecutionContext,
    ) -> Result<(), StopReason> {
        context.checkpoint()?;
        let Ok(mut inner) = self.inner.lock() else {
            return Err(StopReason::Cancelled);
        };
        let Some(entry) = inner.entries.get_mut(search_id) else {
            return Err(StopReason::Cancelled);
        };
        match entry.state {
            EntryState::Running => {
                entry.state = EntryState::Committing;
                Ok(())
            }
            EntryState::Cancelling => Err(StopReason::Cancelled),
            EntryState::Committing => Ok(()),
            EntryState::Reserved | EntryState::Terminal(_) => Err(StopReason::Cancelled),
        }
    }

    pub fn finish(&self, search_id: &str, requested: TerminalStatus) -> Option<TerminalStatus> {
        let Ok(mut inner) = self.inner.lock() else {
            return None;
        };
        let entry = inner.entries.get_mut(search_id)?;
        let status = match entry.state {
            EntryState::Cancelling => TerminalStatus::Cancelled,
            EntryState::Terminal(status) => status,
            _ => requested,
        };
        entry.state = EntryState::Terminal(status);
        entry.terminal_at = Some(Instant::now());
        entry.notify.notify_waiters();
        Some(status)
    }

    pub async fn wait_for_terminal(
        &self,
        search_id: &str,
        wait: Duration,
    ) -> Option<TerminalStatus> {
        let notify = {
            let inner = self.inner.lock().ok()?;
            let entry = inner.entries.get(search_id)?;
            if let EntryState::Terminal(status) = entry.state {
                return Some(status);
            }
            entry.notify.clone()
        };
        let _ = tokio::time::timeout(wait, notify.notified()).await;
        let inner = self.inner.lock().ok()?;
        match inner.entries.get(search_id)?.state {
            EntryState::Terminal(status) => Some(status),
            _ => None,
        }
    }

    pub fn stored_capability_digest(&self, search_id: &str) -> Option<[u8; 32]> {
        self.inner
            .lock()
            .ok()?
            .entries
            .get(search_id)
            .map(|entry| entry.capability_digest)
    }

    pub fn active_count(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| {
                inner
                    .entries
                    .values()
                    .filter(|entry| !matches!(entry.state, EntryState::Terminal(_)))
                    .count()
            })
            .unwrap_or(0)
    }

    pub fn authorized_capability_scope(
        &self,
        search_id: &str,
        cancel_token: &str,
        owner_user_id: Option<&str>,
    ) -> Option<String> {
        let inner = self.inner.lock().ok()?;
        let entry = inner.entries.get(search_id)?;
        if !authorized(entry, cancel_token, owner_user_id) {
            return None;
        }
        Some(URL_SAFE_NO_PAD.encode(entry.capability_digest))
    }

    fn prune_locked(&self, inner: &mut RegistryInner, now: Instant) {
        inner.entries.retain(|_, entry| match entry.state {
            EntryState::Terminal(_) => entry
                .terminal_at
                .is_some_and(|at| now.duration_since(at) < self.terminal_ttl),
            EntryState::Reserved => now < entry.expires_at,
            EntryState::Running | EntryState::Cancelling | EntryState::Committing => true,
        });
        let mut terminals: Vec<_> = inner
            .entries
            .iter()
            .filter_map(|(id, entry)| entry.terminal_at.map(|at| (id.clone(), at)))
            .collect();
        terminals.sort_by_key(|(_, at)| *at);
        while terminals.len() > self.max_terminal {
            if let Some((id, _)) = terminals.first().cloned() {
                inner.entries.remove(&id);
                terminals.remove(0);
            }
        }
    }
}

fn digest_capability(token: &str) -> [u8; 32] {
    let digest = Sha256::digest(token.as_bytes());
    digest.into()
}

fn authorized(entry: &Entry, cancel_token: &str, owner_user_id: Option<&str>) -> bool {
    entry.capability_digest == digest_capability(cancel_token)
        && entry.owner_user_id.as_deref() == owner_user_id
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{CancelResult, ReservationError, SearchExecutionRegistry, TerminalStatus};

    #[tokio::test]
    async fn reservation_uses_capability_and_has_idempotent_cancel() {
        let registry = SearchExecutionRegistry::new(8, 8, Duration::from_secs(60));
        let reservation = registry
            .reserve("search-a", Some("user-a"), "ip-a")
            .expect("reservation");

        assert_ne!(reservation.cancel_token, "search-a");
        assert!(
            registry
                .stored_capability_digest("search-a")
                .expect("digest")
                .iter()
                .any(|byte| *byte != 0)
        );
        assert!(matches!(
            registry.start("search-a", "wrong-token", Some("user-a")),
            Err(ReservationError::Unauthorized)
        ));

        assert!(matches!(
            registry.cancel("search-a", &reservation.cancel_token, Some("user-a")),
            CancelResult::Terminal(TerminalStatus::Cancelled)
        ));
        assert!(matches!(
            registry.cancel("search-a", &reservation.cancel_token, Some("user-a")),
            CancelResult::Terminal(TerminalStatus::Cancelled)
        ));
        assert!(matches!(
            registry.start("search-a", &reservation.cancel_token, Some("user-a")),
            Err(ReservationError::Unavailable)
        ));
    }

    #[test]
    fn duplicate_ids_and_owner_limits_are_bounded_without_overwriting() {
        let registry = SearchExecutionRegistry::new(2, 8, Duration::from_secs(60));
        let first = registry
            .reserve("search-a", Some("user-a"), "ip-a")
            .unwrap();
        assert!(matches!(
            registry.reserve("search-a", Some("user-a"), "ip-a"),
            Err(ReservationError::Conflict)
        ));
        assert!(matches!(
            registry.reserve("search-b", Some("user-a"), "ip-b"),
            Err(ReservationError::OwnerBusy)
        ));
        assert!(matches!(
            registry.reserve("search-c", Some("user-b"), "ip-c"),
            Ok(_)
        ));
        assert!(matches!(
            registry.reserve("search-d", Some("user-c"), "ip-d"),
            Err(ReservationError::Capacity)
        ));
        assert!(
            registry
                .start("search-a", &first.cancel_token, Some("user-a"))
                .is_ok()
        );
    }

    #[tokio::test]
    async fn guest_capability_is_not_authorized_by_same_ip() {
        let registry = SearchExecutionRegistry::new(8, 8, Duration::from_secs(60));
        let reservation = registry.reserve("search-a", None, "shared-ip").unwrap();

        assert!(matches!(
            registry.start(
                "search-a",
                &reservation.cancel_token,
                Some("different-user")
            ),
            Err(ReservationError::Unauthorized)
        ));
        assert!(matches!(
            registry.cancel(
                "search-a",
                &reservation.cancel_token,
                Some("different-user")
            ),
            CancelResult::Unknown
        ));
        assert!(matches!(
            registry.start("search-a", &reservation.cancel_token, None),
            Ok(_)
        ));
    }
}
