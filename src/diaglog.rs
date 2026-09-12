//! Opt-in diagnostics log, written next to the exe. A generic facility used
//! app-wide (providers, config, settings) — deliberately independent of the
//! provider data contract.
//!
//! Two kinds of line: `dbg_log` always writes (failure paths — the file exists
//! to answer "why did this machine find nothing"), `diag` writes only while the
//! user has diagnostics switched on.

/// Verbose logging, off unless the user turns it on in the settings.
static DIAGNOSTICS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_diagnostics(on: bool) {
    DIAGNOSTICS.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn diagnostics_on() -> bool {
    DIAGNOSTICS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Where both kinds of log line go.
pub fn log_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("tokpaek-debug.log"))
}

/// Best-effort append to the log next to the exe. Failure paths always write
/// here — it exists to diagnose "why did this machine find nothing".
pub fn dbg_log(msg: &str) {
    // A failure is rare and worth having on disk even if the app dies next.
    write_log(msg, true);
}

/// A line that is only interesting while diagnosing: written when the user has
/// switched diagnostics on, and never otherwise.
pub fn diag(msg: &str) {
    if diagnostics_on() {
        write_log(msg, false);
    }
}

/// Kept short on purpose: the file is for looking at *today's* behaviour, and
/// nobody prunes it by hand.
const LOG_KEEP_HOURS: i64 = 24;
/// Backstop for a machine that manages to produce a day of noise.
const LOG_MAX_BYTES: usize = 512 * 1024;
/// Lines are batched: a poll writes several of them, and one write per poll
/// beats one per line for a file that lives on the user's SSD.
const FLUSH_AFTER: std::time::Duration = std::time::Duration::from_secs(5);
const FLUSH_LINES: usize = 32;
const PRUNE_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

static PENDING: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
static LAST_FLUSH: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
static LAST_PRUNE: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

fn write_log(msg: &str, urgent: bool) {
    let line = format!(
        "{}  {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        scrub(msg)
    );
    let batch = {
        let Ok(mut pending) = PENDING.lock() else {
            return;
        };
        pending.push(line);
        let due = urgent
            || pending.len() >= FLUSH_LINES
            || LAST_FLUSH
                .lock()
                .ok()
                .and_then(|t| *t)
                .is_none_or(|t| t.elapsed() >= FLUSH_AFTER);
        if due {
            std::mem::take(&mut *pending)
        } else {
            Vec::new()
        }
    };
    write_batch(batch);
}

/// Push whatever is buffered to disk. Called from the poller so a quiet period
/// cannot leave the last lines in memory.
pub fn flush_log() {
    let batch = match PENDING.lock() {
        Ok(mut pending) => std::mem::take(&mut *pending),
        Err(_) => return,
    };
    write_batch(batch);
}

fn write_batch(lines: Vec<String>) {
    if lines.is_empty() {
        return;
    }
    let Some(path) = log_path() else { return };
    if let Ok(mut t) = LAST_FLUSH.lock() {
        *t = Some(std::time::Instant::now());
    }

    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let mut buf = lines.join("\n");
        buf.push('\n');
        let _ = f.write_all(buf.as_bytes());
    }
    prune_if_due(&path);
}

/// Drop anything older than a day — at most once an hour, so the rewrite costs
/// far less than the appends it cleans up after.
fn prune_if_due(path: &std::path::Path) {
    {
        let Ok(mut last) = LAST_PRUNE.lock() else {
            return;
        };
        if last.is_some_and(|t| t.elapsed() < PRUNE_EVERY) {
            return;
        }
        *last = Some(std::time::Instant::now());
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let cutoff = (chrono::Local::now() - chrono::Duration::hours(LOG_KEEP_HOURS)).naive_local();

    let mut keeping = false;
    let mut kept = String::with_capacity(text.len());
    for line in text.lines() {
        // Lines without a stamp belong to the entry above them.
        if let Some(stamp) = line
            .get(..19)
            .and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok())
        {
            keeping = stamp >= cutoff;
        }
        if keeping {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    // Backstop for a day that is somehow still enormous: keep the tail.
    if kept.len() > LOG_MAX_BYTES {
        let cut = kept.len() - LOG_MAX_BYTES;
        let start = kept[cut..].find('\n').map_or(kept.len(), |i| cut + i + 1);
        kept = kept[start..].to_string();
    }
    if kept.len() != text.len() {
        let _ = std::fs::write(path, kept);
    }
}

/// Strip anything that identifies the machine or its account: the log is meant
/// to be sendable as-is. Secrets keep a short prefix so two different tokens can
/// still be told apart in one file.
pub fn scrub(msg: &str) -> String {
    let mut out = msg.to_string();
    if let Ok(profile) = std::env::var("USERPROFILE") {
        out = out.replace(&profile, "%USERPROFILE%");
        if let Some(user) = profile.rsplit(['\\', '/']).next() {
            if user.len() >= 3 {
                out = out.replace(user, "<user>");
            }
        }
    }
    for marker in ["sk-ant-", "ya29.", "rt.1.", "eyJ"] {
        out = mask_after(&out, marker);
    }
    out
}

/// Keep `marker` plus six characters, drop the rest of the secret.
fn mask_after(text: &str, marker: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(marker) {
        let (head, tail) = rest.split_at(at + marker.len());
        out.push_str(head);
        let secret_len = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
            .unwrap_or(tail.len());
        let keep = tail
            .char_indices()
            .nth(6)
            .map_or(secret_len, |(i, _)| i.min(secret_len));
        out.push_str(&tail[..keep]);
        out.push('…');
        rest = &tail[secret_len..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::{mask_after, scrub};

    #[test]
    fn secrets_keep_only_a_short_prefix() {
        let out = mask_after("token sk-ant-oat01-ABCDEFGHIJKLMNOP failed", "sk-ant-");
        assert!(out.starts_with("token sk-ant-oat01-…"), "{out}");
        assert!(!out.contains("ABCDEF"), "{out}");
        assert!(out.ends_with(" failed"), "{out}");
    }

    #[test]
    fn the_user_is_not_named() {
        let Ok(profile) = std::env::var("USERPROFILE") else {
            return;
        };
        let out = scrub(&format!(r"reading {profile}\Tokpaek\settings.json"));
        assert!(!out.contains(&profile), "{out}");
        assert!(out.contains("%USERPROFILE%"), "{out}");
    }
}
