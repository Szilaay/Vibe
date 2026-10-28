//! Activity-triggered quota refresh; no idle timer or parallel requests.
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
const INTERVAL: Duration = Duration::from_secs(30);
#[derive(Default)]
pub(crate) struct LiveUsageRefresh {
    revisions: HashMap<u64, u64>,
    pending: bool,
}
impl LiveUsageRefresh {
    pub fn observe(
        &mut self,
        revisions: HashMap<u64, u64>,
        now: Instant,
        last_attempt: Option<Instant>,
        available: bool,
    ) -> bool {
        if revisions.is_empty() {
            self.revisions.clear();
            self.pending = false;
            return false;
        }
        self.pending |= revisions
            .iter()
            .any(|(id, value)| match self.revisions.get(id) {
                Some(old) => old != value,
                None => *value > 0,
            });
        self.revisions = revisions;
        if available
            && self.pending
            && last_attempt.is_none_or(|last| now.saturating_duration_since(last) >= INTERVAL)
        {
            self.pending = false;
            true
        } else {
            false
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refresh_is_activity_driven_coalesced_and_rate_limited() {
        let now = Instant::now();
        let mut gate = LiveUsageRefresh::default();
        assert!(!gate.observe(HashMap::new(), now, None, true));
        assert!(!gate.observe(HashMap::from([(1, 0)]), now, None, true));
        assert!(gate.observe(HashMap::from([(1, 1)]), now, None, true));
        assert!(!gate.observe(
            HashMap::from([(1, 2), (2, 4)]),
            now + Duration::from_secs(10),
            Some(now),
            true
        ));
        assert!(!gate.observe(
            HashMap::from([(1, 3), (2, 4)]),
            now + INTERVAL,
            Some(now),
            false
        ));
        assert!(gate.observe(
            HashMap::from([(1, 3), (2, 4)]),
            now + INTERVAL,
            Some(now),
            true
        ));
        assert!(
            !gate.observe(
                HashMap::from([(1, 3), (2, 4)]),
                now + INTERVAL * 10,
                Some(now + INTERVAL),
                true
            ),
            "idle terminals never poll repeatedly"
        );
        assert!(!gate.observe(HashMap::new(), now + INTERVAL * 11, None, true));
    }
    #[test]
    fn closed_terminals_drop_pending_work_and_failed_attempts_keep_cooldown() {
        let now = Instant::now();
        let mut gate = LiveUsageRefresh::default();
        assert!(!gate.observe(HashMap::from([(1, 5)]), now, Some(now), true));
        assert!(!gate.observe(HashMap::new(), now + INTERVAL, Some(now), true));
        assert!(!gate.observe(HashMap::from([(2, 0)]), now + INTERVAL * 2, Some(now), true));
    }
}
