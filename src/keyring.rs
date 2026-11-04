use std::time::{Duration, Instant};

use crate::config::KeyringConfig;
use crate::model::{ErrorCode, KeySnapshot};

#[derive(Debug, Clone)]
struct KeyState {
    value: String,
    cooldown_until: Option<Instant>,
}

/// Round-robin API key ring with per-key cooldowns.
///
/// The key values never leave this type except as a lease passed to the
/// provider client, and are never logged.
#[derive(Debug)]
pub struct Keyring {
    keys: Vec<KeyState>,
    cursor: usize,
    cfg: KeyringConfig,
}

#[derive(Debug, Clone)]
pub struct KeyLease {
    pub index: usize,
    pub value: String,
}

impl Keyring {
    pub fn new(values: Vec<String>, cfg: KeyringConfig) -> Self {
        Self {
            keys: values
                .into_iter()
                .map(|value| KeyState {
                    value,
                    cooldown_until: None,
                })
                .collect(),
            cursor: 0,
            cfg,
        }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn transport_retry(&self) -> u32 {
        self.cfg.transport_retry
    }

    /// Returns the next key that is not cooling down, advancing the cursor.
    pub fn next(&mut self) -> Option<KeyLease> {
        let len = self.keys.len();
        if len == 0 {
            return None;
        }
        let now = Instant::now();
        for offset in 0..len {
            let index = (self.cursor + offset) % len;
            let state = &self.keys[index];
            if state
                .cooldown_until
                .is_some_and(|until| until > now)
            {
                continue;
            }
            let value = state.value.clone();
            self.cursor = (index + 1) % len;
            return Some(KeyLease { index, value });
        }
        None
    }

    pub fn mark_success(&mut self, _index: usize) {
        // A successful request does not need to clear a cooldown: a key that
        // was cooling down was not selected in the first place.
    }

    pub fn mark_failure(
        &mut self,
        index: usize,
        code: ErrorCode,
        retry_after: Option<Duration>,
    ) {
        let cooldown = match code {
            ErrorCode::Auth => Duration::from_secs(self.cfg.auth_cooldown_secs),
            ErrorCode::QuotaExhausted => Duration::from_secs(self.cfg.quota_cooldown_secs),
            ErrorCode::RateLimited => retry_after
                .unwrap_or_else(|| Duration::from_secs(self.cfg.rate_limit_cooldown_secs))
                .min(Duration::from_secs(3600)),
            _ => return,
        };
        if let Some(state) = self.keys.get_mut(index) {
            state.cooldown_until = Some(Instant::now() + cooldown);
        }
    }

    pub fn all_cooling(&self) -> bool {
        let now = Instant::now();
        !self.keys.is_empty()
            && self
                .keys
                .iter()
                .all(|state| state.cooldown_until.is_some_and(|until| until > now))
    }

    pub fn snapshot(&self) -> Vec<KeySnapshot> {
        let now = Instant::now();
        self.keys
            .iter()
            .enumerate()
            .map(|(index, state)| {
                let remaining = state
                    .cooldown_until
                    .and_then(|until| until.checked_duration_since(now));
                KeySnapshot {
                    index,
                    healthy: remaining.is_none(),
                    cooldown_remaining_secs: remaining.map(|d| d.as_secs()),
                }
            })
            .collect()
    }
}
