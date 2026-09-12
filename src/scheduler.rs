//! The polling schedule: which family is due, and how long a failure or a
//! server-side throttle pushes its next attempt out. Pure policy — time
//! arrives as milliseconds from the caller, fetches happen outside, so the
//! whole module is testable without a clock, a thread, or a network.
//!
//! This module is the single answer to "how long do we wait?": exponential
//! backoff for plain failures (`Scheduler`) plus server-driven cooldowns for
//! 429s (`RetryPolicy`). Providers report facts (`FetchError.rate_limited`,
//! `retry_after`) and keep no waiting state of their own.
//!
//! The seam to the outside world is `FetchFn`: the poller thread plugs the
//! real providers in, tests plug a scripted fake.

use crate::providers::{Family, FetchError, Snapshot};

/// What a caller plugs in at the fetch seam. Real adapter: `providers::fetch`.
/// Test adapter: any fn returning canned results.
pub type FetchFn = fn(Family) -> Result<Snapshot, FetchError>;

/// `want` sentinel: no family singled out for an immediate poll.
pub const NO_FAMILY: u8 = 0xFF;

/// First/minimum retry delay after a failure, in seconds.
const BACKOFF_BASE: u64 = 5;
/// A dead source must never be hammered faster than this, in seconds.
const BACKOFF_CAP: u64 = 120;

pub struct Scheduler {
    /// Next attempt per family, ms on the caller's clock.
    due: [u64; Family::ALL.len()],
    /// Current failure delay per family, seconds; doubles up to the cap and
    /// resets to the base on any success.
    backoff: [u64; Family::ALL.len()],
}

impl Scheduler {
    pub fn new(now_ms: u64) -> Self {
        Self {
            due: [now_ms; Family::ALL.len()],
            backoff: [BACKOFF_BASE; Family::ALL.len()],
        }
    }

    /// The families to fetch at this tick. `force` is a user-initiated refresh
    /// (everything enabled goes now), `want` is the family the user just
    /// switched to, `mask` is the enabled bitmask.
    pub fn due_families(&self, now_ms: u64, force: bool, want: u8, mask: u8) -> Vec<Family> {
        Family::ALL
            .into_iter()
            .filter(|f| {
                let i = f.idx();
                mask & (1 << i) != 0
                    && (force || want == i as u8 || now_ms >= self.due[i])
            })
            .collect()
    }

    /// Record a fetch outcome and arm the next attempt: successes wait the
    /// user's interval, failures back off exponentially from 5 s to 120 s.
    pub fn report(&mut self, family: Family, ok: bool, now_ms: u64, interval_secs: u64) {
        let i = family.idx();
        self.backoff[i] = if ok {
            BACKOFF_BASE
        } else {
            (self.backoff[i] * 2).min(BACKOFF_CAP)
        };
        let wait = if ok {
            interval_secs.max(1)
        } else {
            self.backoff[i]
        };
        self.due[i] = now_ms + wait * 1000;
    }

    /// Park a family until an absolute moment (a server-driven cooldown end):
    /// the schedule defers to it and the failure backoff ladder stays put.
    pub fn defer_until(&mut self, family: Family, until_ms: u64) {
        self.due[family.idx()] = self.due[family.idx()].max(until_ms);
    }

    /// Seconds until this family is next due (0 when overdue). Test helper and
    /// diagnostics window into the schedule.
    pub fn wait_secs(&self, family: Family, now_ms: u64) -> u64 {
        self.due[family.idx()].saturating_sub(now_ms) / 1000
    }
}

/// How long to sit out sending nothing once a service throttled us — for as
/// long as the server asked, if it said something usable, and five minutes if
/// it did not.
const COOLDOWN_DEFAULT_SECS: u64 = 300;
/// Anthropic has been seen asking for 2708 s; anything beyond an hour is more
/// likely a broken header than a real ban.
const COOLDOWN_MAX_SECS: u64 = 3600;
const COOLDOWN_MIN_SECS: u64 = 30;

/// Server-driven rate-limit cooldowns, per family. The one place that answers
/// "how long do we wait after a 429?": providers report facts, this decides.
pub struct RetryPolicy {
    /// Cooldown end per family, ms on the caller's clock; 0 = none running.
    cooldown_until: [u64; Family::ALL.len()],
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl RetryPolicy {
    pub fn new() -> Self {
        Self {
            cooldown_until: [0; Family::ALL.len()],
        }
    }

    /// Time left of a running cooldown, in seconds, if one is running.
    pub fn cooldown_left_secs(&self, family: Family, now_ms: u64) -> Option<u64> {
        let until = self.cooldown_until[family.idx()];
        (until > now_ms).then(|| (until - now_ms).div_ceil(1000))
    }

    /// When the running cooldown ends, on the caller's clock.
    pub fn cooldown_end_ms(&self, family: Family, now_ms: u64) -> Option<u64> {
        let until = self.cooldown_until[family.idx()];
        (until > now_ms).then_some(until)
    }

    /// Feed one fetch outcome in. A success clears any cooldown; a throttled
    /// failure starts one — as long as the server asked when it said something
    /// usable. A `Retry-After: 0` is not an invitation to retry immediately:
    /// it is the header carrying nothing, and taking it at face value turned
    /// the cooldown into a 30-second one that re-asked the refusing endpoint
    /// all day. Treat it as absent.
    pub fn on_result(&mut self, family: Family, result: &Result<Snapshot, FetchError>, now_ms: u64) {
        let i = family.idx();
        match result {
            Ok(_) => self.cooldown_until[i] = 0,
            Err(e) if e.rate_limited => {
                let secs = e
                    .retry_after
                    .filter(|s| *s > 0)
                    .unwrap_or(COOLDOWN_DEFAULT_SECS)
                    .clamp(COOLDOWN_MIN_SECS, COOLDOWN_MAX_SECS);
                self.cooldown_until[i] = now_ms + secs * 1000;
            }
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{FetchError, Limit, LimitPool, LimitKind};

    const MASK_ALL: u8 = 0b111;

    fn snapshot(family: Family) -> Snapshot {
        Snapshot {
            family,
            limits: vec![Limit {
                title: "t".into(),
                kind: LimitKind::Session,
                pool: LimitPool::Own,
                used_percent: 1.0,
                window: None,
                weekly: None,
            }],
        }
    }

    #[test]
    fn everything_enabled_is_due_at_start() {
        let s = Scheduler::new(0);
        let due = s.due_families(0, false, NO_FAMILY, MASK_ALL);
        assert_eq!(due.len(), 3);
    }

    #[test]
    fn disabled_families_never_come_due() {
        let mut s = Scheduler::new(0);
        s.report(Family::Claude, true, 0, 60);
        s.report(Family::Codex, true, 0, 60);
        s.report(Family::Antigravity, true, 0, 60);
        let due = s.due_families(10 * 60 * 1000, false, NO_FAMILY, 1 << Family::Codex.idx());
        assert_eq!(due, vec![Family::Codex], "only the enabled family polls");
    }

    #[test]
    fn success_waits_the_interval_and_resets_backoff() {
        let mut s = Scheduler::new(0);
        s.report(Family::Claude, true, 0, 60);
        assert_eq!(s.wait_secs(Family::Claude, 30_000), 30);
        // A failure after successes starts from the base, not from history.
        s.report(Family::Claude, false, 60_000, 60);
        assert_eq!(s.wait_secs(Family::Claude, 60_000), 10, "base 5 s doubled once");
        s.report(Family::Claude, true, 70_000, 60);
        s.report(Family::Claude, false, 130_000, 60);
        assert_eq!(s.wait_secs(Family::Claude, 130_000), 10, "success reset the ladder");
    }

    #[test]
    fn failure_backoff_doubles_and_caps_at_two_minutes() {
        let mut s = Scheduler::new(0);
        let mut now = 0u64;
        let mut waits = Vec::new();
        for _ in 0..7 {
            s.report(Family::Claude, false, now, 60);
            waits.push(s.wait_secs(Family::Claude, now));
            now += 1000; // step the clock a little; the wait dominates
        }
        // 10, 20, 40, 80, 120, 120, 120 (the first failure doubles the 5 s base)
        assert_eq!(waits, vec![10, 20, 40, 80, 120, 120, 120]);
    }

    #[test]
    fn force_and_want_jump_the_queue() {
        let mut s = Scheduler::new(0);
        for f in Family::ALL {
            s.report(f, true, 0, 600);
        }
        // Nothing is due for ten minutes…
        assert!(s.due_families(60_000, false, NO_FAMILY, MASK_ALL).is_empty());
        // …but a manual refresh pulls everything enabled in.
        assert_eq!(s.due_families(60_000, true, NO_FAMILY, MASK_ALL).len(), 3);
        // …and switching tools pulls just the new family in.
        let want = Family::Antigravity.idx() as u8;
        assert_eq!(
            s.due_families(60_000, false, want, MASK_ALL),
            vec![Family::Antigravity]
        );
    }

    /// The seam is real: a fake adapter answers without a network, and the
    /// scheduler drives it exactly like the poller thread drives the providers.
    #[test]
    fn a_fake_fetch_adapter_drives_the_same_schedule() {
        fn fake_fetch(family: Family) -> Result<Snapshot, FetchError> {
            match family {
                Family::Codex => Err(FetchError::rate_limited("429")),
                _ => Ok(snapshot(family)),
            }
        }
        let fetch: FetchFn = fake_fetch;
        let mut s = Scheduler::new(0);
        for f in s.due_families(0, false, NO_FAMILY, MASK_ALL).clone() {
            let ok = fetch(f).is_ok();
            s.report(f, ok, 0, 60);
        }
        assert_eq!(s.wait_secs(Family::Claude, 0), 60, "healthy source waits the interval");
        assert_eq!(s.wait_secs(Family::Codex, 0), 10, "throttled source backs off");
    }

    #[test]
    fn a_throttle_starts_a_cooldown_that_holds_then_releases() {
        use super::RetryPolicy;
        let mut p = RetryPolicy::new();
        let err = Err::<Snapshot, _>(FetchError::rate_limited("429"));

        assert_eq!(p.cooldown_left_secs(Family::Claude, 0), None);
        p.on_result(Family::Claude, &err, 0);
        assert_eq!(
            p.cooldown_left_secs(Family::Claude, 0),
            Some(300),
            "no Retry-After means the five-minute default"
        );
        assert_eq!(p.cooldown_left_secs(Family::Claude, 299_000), Some(1));
        assert_eq!(
            p.cooldown_left_secs(Family::Claude, 300_000),
            None,
            "the cooldown must release exactly when it ends"
        );
        // Other families are untouched.
        assert_eq!(p.cooldown_left_secs(Family::Codex, 0), None);
    }

    #[test]
    fn the_server_decides_the_cooldown_length_within_reason() {
        use super::RetryPolicy;
        let mut p = RetryPolicy::new();
        let limited = |secs: Option<u64>| Err::<Snapshot, _>(FetchError::rate_limited_for("429", secs));

        // A real Retry-After is honoured.
        p.on_result(Family::Claude, &limited(Some(120)), 0);
        assert_eq!(p.cooldown_left_secs(Family::Claude, 0), Some(120));

        // `Retry-After: 0` is the header carrying nothing, not an invitation
        // to hammer the endpoint again immediately.
        p.on_result(Family::Codex, &limited(Some(0)), 0);
        assert_eq!(p.cooldown_left_secs(Family::Codex, 0), Some(300));

        // A broken two-hour header is capped at an hour…
        p.on_result(Family::Antigravity, &limited(Some(7200)), 0);
        assert_eq!(p.cooldown_left_secs(Family::Antigravity, 0), Some(3600));
        // …and a five-second one is floored at half a minute.
        p.on_result(Family::Claude, &limited(Some(5)), 0);
        assert_eq!(p.cooldown_left_secs(Family::Claude, 0), Some(30));
    }

    #[test]
    fn a_success_clears_the_cooldown() {
        use super::RetryPolicy;
        let mut p = RetryPolicy::new();
        p.on_result(Family::Claude, &Err::<Snapshot, _>(FetchError::rate_limited("429")), 0);
        assert!(p.cooldown_left_secs(Family::Claude, 0).is_some());
        p.on_result(Family::Claude, &Ok(snapshot(Family::Claude)), 1000);
        assert_eq!(
            p.cooldown_left_secs(Family::Claude, 1000),
            None,
            "the service answered — nothing to sit out"
        );
    }

    #[test]
    fn a_plain_failure_is_not_a_throttle() {
        use super::RetryPolicy;
        let mut p = RetryPolicy::new();
        p.on_result(Family::Claude, &Err::<Snapshot, _>("offline".into()), 0);
        assert_eq!(
            p.cooldown_left_secs(Family::Claude, 0),
            None,
            "only 429s get the server-driven cooldown; the backoff ladder handles the rest"
        );
    }

    #[test]
    fn a_cooldown_outranks_the_backoff_ladder() {
        use super::RetryPolicy;
        let mut s = Scheduler::new(0);
        let mut p = RetryPolicy::new();
        let err = Err::<Snapshot, _>(FetchError::rate_limited_for("429", Some(600)));

        p.on_result(Family::Claude, &err, 0);
        s.report(Family::Claude, false, 0, 60);
        if let Some(until) = p.cooldown_end_ms(Family::Claude, 0) {
            s.defer_until(Family::Claude, until);
        }
        assert_eq!(s.wait_secs(Family::Claude, 0), 600, "the longer wait wins");
        assert!(
            !s.due_families(500_000, false, NO_FAMILY, MASK_ALL)
                .contains(&Family::Claude),
            "mid-cooldown the family is simply not due"
        );
        // A manual refresh does surface the family, but the poller consults
        // the cooldown before fetching and defers again — the scheduler alone
        // never re-arms the wait past the cooldown end.
        assert!(s.due_families(500_000, true, NO_FAMILY, MASK_ALL).contains(&Family::Claude));
    }
}
