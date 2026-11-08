//! #15: per-site request lanes — one shared rate-limiter for *all* network
//! fetches (bookmark enrichment, re-fetch metadata, update checks, and the
//! M3 downloader).
//!
//! Before the M2.6 round-robin each work-loop tracked its own "last request
//! per site" and slept inline; two jobs hitting rule34 at once could still
//! double-request it. A lane per site inside one shared registry gives every
//! caller the next guaranteed-spaced slot, keeps different sites running in
//! parallel, and reacts to Stop within a 100 ms poll slice.
//!
//! Reservations are made up-front: a cancelled wait leaves the lane advanced
//! by at most one slot — harmless politeness, deliberately not rolled back.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct SiteQueue {
    /// site -> the earliest moment the *next* request may start
    lanes: Mutex<BTreeMap<String, Instant>>,
}

impl SiteQueue {
    /// Claim the next slot for `site` and wait until it opens.
    /// Returns `false` if `cancelled` turns true while waiting (Stop/Lock).
    pub async fn reserve<F>(&self, site: &str, rate_ms: i64, cancelled: F) -> bool
    where
        F: Fn() -> bool + Send + Sync,
    {
        let rate = Duration::from_millis(rate_ms.max(0) as u64);
        let start = {
            let mut lanes = self.lanes.lock().unwrap();
            let now = Instant::now();
            let slot = lanes
                .entry(site.to_string())
                .or_insert_with(|| now - rate); // first request: no initial delay
            let start = if *slot < now { now } else { *slot };
            *slot = start + rate;
            start
        };

        // Sleep out the wait in short slices so Stop reacts fast.
        loop {
            let now = Instant::now();
            if now >= start {
                return true;
            }
            if cancelled() {
                return false;
            }
            let remaining = start - now;
            tokio::time::sleep(std::cmp::min(remaining, Duration::from_millis(100))).await;
        }
    }

    /// Forget all lane history (Lock/Nuke: nothing is in flight anymore).
    pub fn clear(&self) {
        self.lanes.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn same_site_requests_are_spaced() {
        let q = SiteQueue::default();
        let never = || false;
        let t0 = Instant::now();
        assert!(q.reserve("rule34", 150, never).await);
        assert!(q.reserve("rule34", 150, never).await);
        assert!(t0.elapsed() >= Duration::from_millis(140), "elapsed {:?}", t0.elapsed());
    }

    #[tokio::test]
    async fn first_request_does_not_wait_and_sites_are_independent() {
        let q = SiteQueue::default();
        let never = || false;
        let t0 = Instant::now();
        // first on rule34 is immediate; e621 is its own lane and also immediate
        assert!(q.reserve("rule34", 5000, never).await);
        assert!(q.reserve("e621", 5000, never).await);
        assert!(t0.elapsed() < Duration::from_millis(300), "elapsed {:?}", t0.elapsed());
    }

    #[tokio::test]
    async fn cancelled_wait_gives_up_promptly() {
        let q = SiteQueue::default();
        let never = || false;
        assert!(q.reserve("ehentai", 30_000, never).await); // claims a +30 s lane
        // cancellation flips after ~150 ms; reserve must give up well before 30 s
        let t0 = Instant::now();
        let ok = q
            .reserve(
                "ehentai",
                30_000,
                || t0.elapsed() >= Duration::from_millis(150),
            )
            .await;
        assert!(!ok);
        assert!(t0.elapsed() < Duration::from_secs(2), "elapsed {:?}", t0.elapsed());
        q.clear();
        assert!(q.lanes.lock().unwrap().is_empty());
    }
}
