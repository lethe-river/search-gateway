use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Sliding-window limiter for outbound tool calls. A limit of zero disables it.
pub struct RateLimiter {
    max_per_minute: u32,
    calls: Mutex<VecDeque<Instant>>,
}

impl RateLimiter {
    pub fn new(max_per_minute: u32) -> Self {
        Self {
            max_per_minute,
            calls: Mutex::new(VecDeque::new()),
        }
    }

    /// Returns `Err(retry_after_secs)` when the window is full.
    pub fn acquire(&self) -> Result<(), u64> {
        if self.max_per_minute == 0 {
            return Ok(());
        }
        let mut calls = self.calls.lock().unwrap_or_else(|error| error.into_inner());
        let now = Instant::now();
        while let Some(front) = calls.front() {
            if now.duration_since(*front) >= Duration::from_secs(60) {
                calls.pop_front();
            } else {
                break;
            }
        }
        if calls.len() >= self.max_per_minute as usize {
            let retry = calls
                .front()
                .map(|front| 60u64.saturating_sub(now.duration_since(*front).as_secs()))
                .unwrap_or(60)
                .max(1);
            return Err(retry);
        }
        calls.push_back(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_calls_above_the_window() {
        let limiter = RateLimiter::new(2);
        assert!(limiter.acquire().is_ok());
        assert!(limiter.acquire().is_ok());
        let retry = limiter.acquire().expect_err("third call must be limited");
        assert!((1..=60).contains(&retry));
    }

    #[test]
    fn zero_disables_the_limiter() {
        let limiter = RateLimiter::new(0);
        for _ in 0..100 {
            assert!(limiter.acquire().is_ok());
        }
    }
}
