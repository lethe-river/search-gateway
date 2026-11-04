use std::time::{Duration, Instant};

/// A wall-clock budget shared by a single tool call.
#[derive(Debug, Clone)]
pub struct Budget {
    deadline: Instant,
}

impl Budget {
    pub fn new(total: Duration) -> Self {
        Self {
            deadline: Instant::now() + total,
        }
    }

    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    pub fn remaining_ms(&self) -> u64 {
        self.remaining().as_millis().min(u128::from(u64::MAX)) as u64
    }

    pub fn is_exhausted(&self) -> bool {
        self.remaining().is_zero()
    }

    /// The smaller of the remaining total budget and a per-stage cap.
    pub fn slice(&self, cap: Duration) -> Duration {
        self.remaining().min(cap)
    }

    /// A stage slice that leaves `reserve` untouched for a later stage.
    pub fn slice_reserving(&self, cap: Duration, reserve: Duration) -> Duration {
        self.remaining().saturating_sub(reserve).min(cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserving_a_slice_keeps_time_for_the_last_resort() {
        let budget = Budget::new(Duration::from_secs(30));
        let slice = budget.slice_reserving(
            Duration::from_secs(20),
            Duration::from_secs(8),
        );
        assert!(slice <= Duration::from_secs(20));
        assert!(budget.remaining() > Duration::from_secs(29));
    }

    #[test]
    fn reserve_larger_than_the_budget_yields_no_slice() {
        let budget = Budget::new(Duration::from_millis(5));
        assert!(
            budget
                .slice_reserving(Duration::from_secs(10), Duration::from_secs(8))
                .is_zero()
        );
    }
}
