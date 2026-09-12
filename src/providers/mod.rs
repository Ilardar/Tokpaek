//! Quota providers. One module per tool family; each knows how to read its own
//! quota from whatever the installed tool leaves on this machine (an encrypted
//! token, a plain OAuth file, a locally running language server).
//!
//! Everything above this module only ever sees `Family` / `Snapshot` / `Limit`.

pub mod antigravity;
pub mod claude;
pub mod codex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A tool family. One family covers every surface of the same product (app,
/// IDE and CLI) because they all bill against the same account quota.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Family {
    Claude,
    Codex,
    Antigravity,
}

impl Family {
    pub const ALL: [Family; 3] = [Family::Claude, Family::Codex, Family::Antigravity];

    /// Stable index, used for the per-family arrays and the enabled-bitmask.
    pub fn idx(self) -> usize {
        match self {
            Family::Claude => 0,
            Family::Codex => 1,
            Family::Antigravity => 2,
        }
    }

    /// Family name alone (no plan/tier) — what the header shows in "family only".
    pub fn name(self) -> &'static str {
        match self {
            Family::Claude => "Claude",
            Family::Codex => "Codex",
            Family::Antigravity => "Antigravity",
        }
    }
}

/// What a limit measures. The gauge and the weekly-reset lookup branch on
/// this instead of sniffing provider-minted title strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitKind {
    /// A short rolling window (Claude's 5 hours, Codex's primary window).
    Session,
    /// A seven-day quota (Claude's weekly, Codex's secondary window,
    /// Antigravity's weekly lockout while it holds).
    Weekly,
}

/// Which pool a limit draws from. `Own` for families with a single pool;
/// Antigravity splits its quota between Gemini models and third-party models.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitPool {
    Own,
    Gemini,
    ClaudeGpt,
}

/// One quota window as we want to render it.
#[derive(Clone, Debug)]
pub struct Limit {
    pub title: String,
    pub kind: LimitKind,
    pub pool: LimitPool,
    /// Quota consumed, 0.0..=100.0
    pub used_percent: f32,
    /// None while the service reports no window yet — a 5-hour limit only
    /// starts counting at the first request of a session. The row still belongs
    /// on screen: the limit exists and stands at 0 %, there is just no clock to
    /// draw against.
    pub window: Option<LimitWindow>,
    /// Weekly quota reported independently of the session quota.
    pub weekly: Option<WeeklyQuota>,
}

#[derive(Clone, Copy, Debug)]
pub struct WeeklyQuota {
    pub remaining_percent: f32,
    pub resets_at: Option<DateTime<Utc>>,
}

impl Limit {
    /// Only tests read this today; kept next to the data it describes.
    #[cfg(test)]
    pub fn weekly_exhausted(&self) -> bool {
        self.weekly.is_some_and(|w| w.remaining_percent <= 0.0)
    }
}

/// The stretch of time a limit is measured over.
#[derive(Clone, Copy, Debug)]
pub struct LimitWindow {
    /// Synthesized as `resets_at` minus the window length; no API returns it.
    /// `None` when that subtraction lands in the future — the reset is further
    /// out than the window is long, which nothing about the window can explain
    /// (a local clock running behind will do it, G25).
    pub start: Option<DateTime<Utc>>,
    pub resets_at: DateTime<Utc>,
}

impl LimitWindow {
    /// A window of known length ending at `resets_at`.
    pub fn ending_at(
        resets_at: DateTime<Utc>,
        len: chrono::Duration,
        now: DateTime<Utc>,
    ) -> LimitWindow {
        let start = resets_at - len;
        LimitWindow {
            start: (start <= now).then_some(start),
            resets_at,
        }
    }

    /// Time-marker helpers: only tests read them since the circle gauge drew
    /// over the window math; kept here where the window semantics live.
    #[cfg(test)]
    pub fn marker_frac(&self, now: DateTime<Utc>) -> f32 {
        self.elapsed_frac(now).unwrap_or(0.0)
    }

    #[cfg(test)]
    pub fn elapsed_frac(&self, now: DateTime<Utc>) -> Option<f32> {
        let start = self.start?;
        let total = (self.resets_at - start).num_seconds().max(1);
        let elapsed = (now - start).num_seconds().clamp(0, total);
        Some((elapsed as f32 / total as f32).clamp(0.0, 1.0))
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub family: Family,
    pub limits: Vec<Limit>,
}

/// Why a fetch failed. The distinction that matters to the UI is whether the
/// numbers we already have are still meaningful: a throttled request says
/// nothing about the quota itself, so the last values stay on screen.
///
/// Providers report facts; the waiting decision belongs to `retry::RetryPolicy`,
/// not to them.
#[derive(Clone, Debug)]
pub struct FetchError {
    pub msg: String,
    /// The service refused to answer *for now* (HTTP 429).
    pub rate_limited: bool,
    /// The server's own `Retry-After` header, in seconds, when it sent one.
    /// Only meaningful together with `rate_limited`.
    pub retry_after: Option<u64>,
}

impl From<String> for FetchError {
    fn from(msg: String) -> Self {
        Self {
            msg,
            rate_limited: false,
            retry_after: None,
        }
    }
}

impl From<&str> for FetchError {
    fn from(msg: &str) -> Self {
        Self::from(msg.to_string())
    }
}

impl FetchError {
    pub fn rate_limited(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            rate_limited: true,
            retry_after: None,
        }
    }

    /// A throttled refusal that carries the server's requested pause.
    pub fn rate_limited_for(msg: impl Into<String>, retry_after: Option<u64>) -> Self {
        Self {
            msg: msg.into(),
            rate_limited: true,
            retry_after,
        }
    }
}

/// Fetch a fresh snapshot for one family.
pub fn fetch(family: Family) -> Result<Snapshot, FetchError> {
    match family {
        Family::Claude => claude::fetch(),
        Family::Codex => codex::fetch(),
        Family::Antigravity => antigravity::fetch(),
    }
}

/// Human title for a quota window of the given length, so every provider names
/// its windows the same way.
pub fn window_title(seconds: i64) -> String {
    match seconds {
        s if s <= 0 => "limit".into(),
        s if (17000..=19000).contains(&s) => "5-hour limit".into(),
        s if (600_000..=700_000).contains(&s) => "Weekly · all models".into(),
        s if (2_500_000..=2_700_000).contains(&s) => "Monthly limit".into(),
        s if s % 86_400 == 0 => format!("{}-day limit", s / 86_400),
        s => format!("{}-hour limit", (s + 1800) / 3600),
    }
}

