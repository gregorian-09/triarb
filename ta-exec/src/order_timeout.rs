use std::collections::HashMap;
use std::time::{Duration, Instant};
use ta_core::OpportunityId;

/// How long to wait for a leg to fill before considering it timed out.
#[derive(Debug, Clone, Copy)]
pub struct OrderTimeoutConfig {
    pub submission_timeout: Duration,
}

impl Default for OrderTimeoutConfig {
    fn default() -> Self {
        Self {
            submission_timeout: Duration::from_secs(5),
        }
    }
}

/// Tracks order submission timestamps and detects timeouts.
pub struct OrderTimeoutTracker {
    /// opp_id -> list of (leg_idx, submitted_at)
    pending: HashMap<OpportunityId, Vec<(usize, Instant)>>,
    config: OrderTimeoutConfig,
}

impl OrderTimeoutTracker {
    pub fn new(config: OrderTimeoutConfig) -> Self {
        Self {
            pending: HashMap::new(),
            config,
        }
    }

    /// Record a leg submission.
    pub fn record_submission(&mut self, opp_id: OpportunityId, leg_idx: usize) {
        self.pending
            .entry(opp_id)
            .or_default()
            .push((leg_idx, Instant::now()));
    }

    /// Mark a leg as resolved (filled or failed) — removes it from timeout tracking.
    pub fn resolve_leg(&mut self, opp_id: &OpportunityId, leg_idx: usize) {
        if let Some(legs) = self.pending.get_mut(opp_id) {
            legs.retain(|(idx, _)| *idx != leg_idx);
            if legs.is_empty() {
                self.pending.remove(opp_id);
            }
        }
    }

    /// Resolve all legs for an opportunity (e.g. fully filled or abandoned).
    pub fn resolve_opportunity(&mut self, opp_id: &OpportunityId) {
        self.pending.remove(opp_id);
    }

    /// Returns legs that have exceeded the submission timeout.
    pub fn check_timeouts(&self) -> Vec<(OpportunityId, usize)> {
        let now = Instant::now();
        let mut timed_out = Vec::new();
        for (opp_id, legs) in &self.pending {
            for (leg_idx, submitted_at) in legs {
                if now.saturating_duration_since(*submitted_at) > self.config.submission_timeout {
                    timed_out.push((*opp_id, *leg_idx));
                }
            }
        }
        timed_out
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ta_core::{ArbitrageOpportunity, Triangle};

    fn dummy_id(n: u8) -> OpportunityId {
        let opp = ArbitrageOpportunity {
            triangle: Triangle {
                leg_a: (format!("A{n}"), "B".into()),
                leg_b: ("B".into(), "C".into()),
                leg_c: ("C".into(), "A".into()),
                opportunity_bps: 10.0,
            },
            routes: Vec::new(),
            expected_profit_bps: 5.0,
            ts_ns: 0,
        };
        OpportunityId::from_opportunity(&opp)
    }

    #[test]
    fn test_record_and_resolve() {
        let cfg = OrderTimeoutConfig {
            submission_timeout: Duration::from_secs(60),
        };
        let mut tracker = OrderTimeoutTracker::new(cfg);
        let id = dummy_id(1);

        tracker.record_submission(id, 0);
        assert_eq!(tracker.pending_count(), 1);

        tracker.resolve_leg(&id, 0);
        assert!(tracker.is_empty());
    }

    #[test]
    fn test_timeout_detected() {
        let cfg = OrderTimeoutConfig {
            submission_timeout: Duration::from_nanos(1),
        };
        let mut tracker = OrderTimeoutTracker::new(cfg);
        let id = dummy_id(1);

        tracker.record_submission(id, 0);
        // give the timeout a moment to elapse
        std::thread::sleep(std::time::Duration::from_micros(10));

        let timeouts = tracker.check_timeouts();
        assert_eq!(timeouts.len(), 1);
        assert_eq!(timeouts[0], (id, 0));
    }

    #[test]
    fn test_no_timeout_within_duration() {
        let cfg = OrderTimeoutConfig {
            submission_timeout: Duration::from_secs(60),
        };
        let tracker = OrderTimeoutTracker::new(cfg);

        // nothing submitted yet
        assert!(tracker.check_timeouts().is_empty());
    }

    #[test]
    fn test_resolve_opportunity_clears_all() {
        let cfg = OrderTimeoutConfig {
            submission_timeout: Duration::from_secs(60),
        };
        let mut tracker = OrderTimeoutTracker::new(cfg);
        let id = dummy_id(1);

        tracker.record_submission(id, 0);
        tracker.record_submission(id, 1);
        tracker.resolve_opportunity(&id);

        assert!(tracker.is_empty());
    }
}
