//! Persistent Tokpaek settings, stored in %APPDATA%\Tokpaek\settings.json.
//! Falls back to legacy %APPDATA%\Quotty\settings.json if found.

use crate::i18n::Language;
use crate::providers::Family;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

/// At most one settings write per this long, whatever the UI does.
const WRITE_THROTTLE: std::time::Duration = std::time::Duration::from_millis(500);

/// The latest serialized settings waiting for the throttle to elapse.
static PENDING_SAVE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
static LAST_WRITE: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// Color palette for the circle gauge.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CirclePalette {
    #[default]
    Gradient,
    Traffic,
    Cyan,
    Monochrome,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    /// Window opacity, 0.2..=1.0
    pub opacity: f32,
    /// Last window position (screen points). None = let the OS place it.
    pub pos: Option<(f32, f32)>,
    /// Poll interval in seconds.
    pub poll_secs: u64,
    pub claude_enabled: bool,
    pub codex_enabled: bool,
    pub antigravity_enabled: bool,
    /// Pinned family, and the one restored at startup.
    pub family: Family,
    /// A source the user picked by hand in a menu. `Some(f)` pins the widget to
    /// `f` and outranks detection — the foreground app no longer switches it;
    /// `None` is the default "follow the app in front" behavior. Cleared when
    /// the pinned source is switched off in Settings → Sources.
    pub pinned_family: Option<Family>,
    /// Verbose, anonymised logging next to the exe. Off unless asked for.
    pub diagnostics: bool,
    /// Draw the widget only over the apps it watches (Antigravity, Claude,
    /// ChatGPT desktop). Replaces the old foreground-based "smart focus":
    /// visible while at least one of their windows is open and not minimized.
    pub smart_focus: bool,
    /// Keep the window above everything else, not just the watched apps.
    pub always_on_top: bool,
    pub language: Language,
    pub circle_palette: CirclePalette,
    pub circle_segments: usize,
    pub circle_size: u32,
    pub circle_show_claude_gpt: bool,
    pub circle_show_weekly_reset: bool,
    /// Legacy name of `smart_focus` (pre-1.5), kept so an existing
    /// settings.json carries the user's choice across the rename.
    #[serde(default, skip_serializing)]
    auto_hide_on_inactive: Option<bool>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            opacity: 0.8,
            pos: None,
            poll_secs: 60,
            claude_enabled: true,
            codex_enabled: true,
            antigravity_enabled: true,
            family: Family::Claude,
            pinned_family: None,
            diagnostics: false,
            smart_focus: false,
            always_on_top: true,
            language: Language::Russian,
            circle_palette: CirclePalette::Gradient,
            circle_segments: 12,
            circle_size: 220,
            circle_show_claude_gpt: false,
            circle_show_weekly_reset: false,
            auto_hide_on_inactive: None,
        }
    }
}

/// Where `settings.json` may live. `dirs::config_dir()` alone is not enough:
/// depending on how the exe was started (shortcut, Startup folder, shell) it has
/// been seen to resolve to something other than the real Roaming directory, and
/// the app would then silently start from defaults every login.
fn candidate_paths() -> Vec<PathBuf> {
    let mut dirs_list: Vec<PathBuf> = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        dirs_list.push(PathBuf::from(appdata));
    }
    if let Some(cfg) = dirs::config_dir() {
        dirs_list.push(cfg);
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        dirs_list.push(PathBuf::from(profile).join("AppData").join("Roaming"));
    }
    if let Some(home) = dirs::home_dir() {
        dirs_list.push(home.join("AppData").join("Roaming"));
    }
    dirs_list.dedup();
    let mut paths = Vec::new();
    // Prefer Tokpaek
    for d in &dirs_list {
        paths.push(d.join("Tokpaek").join("settings.json"));
    }
    // Fall back to legacy Quotty
    for d in &dirs_list {
        paths.push(d.join("Quotty").join("settings.json"));
    }
    paths
}

impl Settings {
    pub fn dir() -> Option<PathBuf> {
        Self::path().and_then(|p| p.parent().map(|d| d.to_path_buf()))
    }

    /// Resolved once: an existing file wins wherever it is, so load and save
    /// can never end up on different paths.
    fn path() -> Option<PathBuf> {
        static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
        PATH.get_or_init(|| {
            let candidates = candidate_paths();
            // 1. If a Tokpaek settings file exists, use it
            for p in &candidates {
                if p.to_string_lossy().contains("Tokpaek") && std::fs::metadata(p).is_ok() {
                    return Some(p.clone());
                }
            }
            // 2. If a legacy Quotty settings file exists, copy it to the primary Tokpaek path
            let primary = candidates.first().cloned();
            for p in &candidates {
                if p.to_string_lossy().contains("Quotty") && std::fs::metadata(p).is_ok() {
                    if let Some(ref dest) = primary {
                        if let Some(parent) = dest.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if std::fs::copy(p, dest).is_ok() {
                            return Some(dest.clone());
                        }
                    }
                    return Some(p.clone());
                }
            }
            primary
        })
        .clone()
    }

    pub fn load() -> Self {
        let path = Self::path();
        let parse_file = |p: &std::path::Path| -> Option<Settings> {
            let text = std::fs::read_to_string(p).ok()?;
            serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
        };

        let loaded = path.as_deref().and_then(parse_file).or_else(|| {
            path.as_ref()
                .map(|p| p.with_extension("bak"))
                .as_deref()
                .and_then(parse_file)
        });

        let mut s: Settings = match loaded {
            Some(parsed) => parsed,
            None => {
                crate::diaglog::dbg_log(&format!(
                    "settings: could not load from {:?} or backup, using defaults",
                    path
                ));
                Settings::default()
            }
        };
        s.migrate();
        s.sanitize();
        s
    }

    /// Carry choices from older settings files across renamed fields.
    fn migrate(&mut self) {
        if let Some(old) = self.auto_hide_on_inactive.take() {
            // The old foreground-based "smart focus" was the same user intent
            // as the new watched-apps visibility: keep the choice.
            self.smart_focus = old;
        }
    }

    /// Load-time invariants, however the file got into this shape: a value the
    /// UI could never produce (a hand-edited or truncated JSON) must not
    /// survive into the running app.
    pub fn sanitize(&mut self) {
        self.opacity = self.opacity.clamp(0.15, 1.0);
        if self.poll_secs < 15 {
            self.poll_secs = 15;
        }
        // Never leave the user with nothing to show.
        if !(self.claude_enabled || self.codex_enabled || self.antigravity_enabled) {
            self.claude_enabled = true;
        }
        if !self.enabled(self.family) {
            self.family = self.first_enabled();
        }
        // A pin on a source that is switched off means nothing: drop back to
        // following the app in front.
        if self.pinned_family.is_some_and(|f| !self.enabled(f)) {
            self.pinned_family = None;
        }
        // With the lower arc replaced by Claude/GPT there is no seven-day arc,
        // so its timer cannot be showing.
        if self.circle_show_claude_gpt {
            self.circle_show_weekly_reset = false;
        }
    }

    /// Ask for the current settings to land on disk. Writes are throttled to
    /// at most one per `WRITE_THROTTLE`: a slider drag used to serialize and
    /// rename a file per pixel. The latest request always wins — the pending
    /// text is replaced, not queued — and `flush_due` (called once per frame)
    /// performs the deferred write.
    pub fn save(&self) {
        let Ok(raw) = serde_json::to_string_pretty(self) else {
            return;
        };
        if let Ok(mut pending) = PENDING_SAVE.lock() {
            *pending = Some(raw);
        }
        Self::flush_due();
    }

    /// Write the pending settings if the throttle has elapsed. No-op when
    /// nothing is pending, so calling it every frame costs a mutex check.
    pub fn flush_due() {
        let due = LAST_WRITE
            .lock()
            .ok()
            .and_then(|t| *t)
            .is_none_or(|t| t.elapsed() >= WRITE_THROTTLE);
        if due {
            Self::flush_now();
        }
    }

    /// Write the pending settings immediately, ignoring the throttle. Call
    /// before anything that may end the process (quit, crash paths).
    pub fn flush_now() {
        let raw = match PENDING_SAVE.lock() {
            Ok(mut pending) => pending.take(),
            Err(_) => None,
        };
        let Some(raw) = raw else { return };
        if let Some(p) = Self::path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = p.with_extension("tmp");
            let bak = p.with_extension("bak");
            if std::fs::write(&tmp, &raw).is_ok() {
                if p.exists() {
                    let _ = std::fs::copy(&p, &bak);
                }
                let _ = std::fs::rename(&tmp, &p);
            }
        }
        if let Ok(mut t) = LAST_WRITE.lock() {
            *t = Some(std::time::Instant::now());
        }
    }

    pub fn enabled(&self, f: Family) -> bool {
        match f {
            Family::Claude => self.claude_enabled,
            Family::Codex => self.codex_enabled,
            Family::Antigravity => self.antigravity_enabled,
        }
    }

    pub fn set_enabled(&mut self, f: Family, on: bool) {
        match f {
            Family::Claude => self.claude_enabled = on,
            Family::Codex => self.codex_enabled = on,
            Family::Antigravity => self.antigravity_enabled = on,
        }
        if !(self.claude_enabled || self.codex_enabled || self.antigravity_enabled) {
            // Refuse to turn the last one off.
            self.set_enabled(f, true);
        }
        if !self.enabled(self.family) {
            self.family = self.first_enabled();
        }
        if self.pinned_family.is_some_and(|f| !self.enabled(f)) {
            self.pinned_family = None;
        }
    }

    pub fn first_enabled(&self) -> Family {
        Family::ALL
            .into_iter()
            .find(|f| self.enabled(*f))
            .unwrap_or(Family::Claude)
    }

    /// Bitmask of enabled families, as handed to the poller thread.
    pub fn enabled_mask(&self) -> u8 {
        Family::ALL
            .into_iter()
            .filter(|f| self.enabled(*f))
            .fold(0u8, |m, f| m | (1 << f.idx()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_values_no_ui_could_produce() {
        let mut s = Settings {
            opacity: 5.0,
            poll_secs: 1,
            ..Settings::default()
        };
        s.sanitize();
        assert_eq!(s.opacity, 1.0);
        assert_eq!(s.poll_secs, 15, "polling below 15 s hammers the services");

        let mut s = Settings {
            opacity: -1.0,
            ..Settings::default()
        };
        s.sanitize();
        assert_eq!(s.opacity, 0.15);
    }

    #[test]
    fn the_last_enabled_family_refuses_to_turn_off() {
        let mut s = Settings {
            claude_enabled: true,
            codex_enabled: false,
            antigravity_enabled: false,
            family: Family::Claude,
            ..Settings::default()
        };
        s.set_enabled(Family::Claude, false);
        assert!(s.claude_enabled, "nothing on screen is not an option");

        s.codex_enabled = true;
        s.set_enabled(Family::Claude, false);
        assert!(!s.claude_enabled, "with a second source, it does turn off");
        assert_eq!(s.family, Family::Codex, "and the active family follows");
    }

    #[test]
    fn sanitize_moves_the_family_onto_an_enabled_source() {
        let mut s = Settings {
            claude_enabled: false,
            codex_enabled: true,
            antigravity_enabled: false,
            family: Family::Claude,
            ..Settings::default()
        };
        s.sanitize();
        assert_eq!(s.family, Family::Codex);

        // All off cannot survive: Claude comes back on.
        let mut s = Settings {
            claude_enabled: false,
            codex_enabled: false,
            antigravity_enabled: false,
            family: Family::Antigravity,
            ..Settings::default()
        };
        s.sanitize();
        assert!(s.claude_enabled);
        assert_eq!(s.family, Family::Claude);
    }

    #[test]
    fn the_enabled_mask_matches_the_flags() {
        let s = Settings {
            claude_enabled: true,
            codex_enabled: false,
            antigravity_enabled: true,
            ..Settings::default()
        };
        assert_eq!(
            s.enabled_mask(),
            (1 << Family::Claude.idx()) | (1 << Family::Antigravity.idx())
        );
    }

    #[test]
    fn a_replaced_scale_has_no_seven_day_timer() {
        let mut s = Settings {
            circle_show_claude_gpt: true,
            circle_show_weekly_reset: true,
            ..Settings::default()
        };
        s.sanitize();
        assert!(
            !s.circle_show_weekly_reset,
            "with the lower arc replaced, the seven-day timer has nothing to show"
        );
    }

    #[test]
    fn a_pin_on_a_disabled_source_is_cleared() {
        // Turning the pinned source off in Sources drops the pin: the widget
        // goes back to following the foreground app rather than showing a
        // source that is switched off.
        let mut s = Settings {
            claude_enabled: true,
            codex_enabled: false,
            antigravity_enabled: true,
            pinned_family: Some(Family::Antigravity),
            ..Settings::default()
        };
        s.set_enabled(Family::Antigravity, false);
        assert_eq!(s.pinned_family, None);

        // The same invariant is enforced on a hand-edited file at load time.
        let mut s = Settings {
            antigravity_enabled: false,
            pinned_family: Some(Family::Antigravity),
            ..Settings::default()
        };
        s.sanitize();
        assert_eq!(s.pinned_family, None);
    }
}
