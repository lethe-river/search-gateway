use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::CircuitConfig;
use crate::model::BreakerSnapshot;

#[derive(Debug)]
struct BreakerInner {
    failures: u32,
    open_until: Option<Instant>,
    probe_in_flight: bool,
}

/// Per-provider circuit breaker.
///
/// Key-level failures (auth, quota, rate limit) are handled by [`crate::keyring`]
/// and must not be recorded here.
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    inner: Arc<Mutex<BreakerInner>>,
    cfg: CircuitConfig,
}

impl CircuitBreaker {
    pub fn new(cfg: CircuitConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BreakerInner {
                failures: 0,
                open_until: None,
                probe_in_flight: false,
            })),
            cfg,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BreakerInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Returns true when a request may be sent. In the half-open state only
    /// one probe is admitted.
    pub fn allow(&self) -> bool {
        let mut inner = self.lock();
        let now = Instant::now();
        match inner.open_until {
            None => true,
            Some(until) if now < until => false,
            Some(_) => {
                if inner.probe_in_flight {
                    false
                } else {
                    inner.probe_in_flight = true;
                    true
                }
            }
        }
    }

    pub fn record_success(&self) {
        let mut inner = self.lock();
        inner.failures = 0;
        inner.open_until = None;
        inner.probe_in_flight = false;
    }

    /// Releases a half-open probe without recording a provider failure.
    /// Used when the request never reached the provider (for example, all keys
    /// are cooling down).
    pub fn release_probe(&self) {
        let mut inner = self.lock();
        inner.probe_in_flight = false;
    }

    pub fn record_transient_failure(&self) {
        let mut inner = self.lock();
        if inner.probe_in_flight || inner.open_until.is_some() {
            inner.open_until = Some(Instant::now() + Duration::from_secs(self.cfg.cooldown_secs));
            inner.probe_in_flight = false;
            inner.failures = self.cfg.failure_threshold;
            return;
        }
        inner.failures = inner.failures.saturating_add(1);
        if inner.failures >= self.cfg.failure_threshold {
            inner.open_until = Some(Instant::now() + Duration::from_secs(self.cfg.cooldown_secs));
        }
    }

    pub fn record_misconfig(&self) {
        let mut inner = self.lock();
        inner.failures = self.cfg.failure_threshold;
        inner.open_until =
            Some(Instant::now() + Duration::from_secs(self.cfg.misconfig_cooldown_secs));
        inner.probe_in_flight = false;
    }

    pub fn snapshot(&self) -> BreakerSnapshot {
        let inner = self.lock();
        let now = Instant::now();
        let remaining = inner
            .open_until
            .and_then(|until| until.checked_duration_since(now));
        let state = match inner.open_until {
            None => "closed",
            Some(until) if now < until => "open",
            Some(_) => "half_open",
        };
        BreakerSnapshot {
            state: state.to_string(),
            failures: inner.failures,
            open_remaining_secs: remaining.map(|d| d.as_secs()),
        }
    }
}
