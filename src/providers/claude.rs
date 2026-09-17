//! Claude module: reads the Claude Desktop app's locally-stored OAuth token
//! (Chromium `os_crypt` AES-256-GCM, key wrapped with Windows DPAPI) and queries
//! the account usage endpoint for the 5-hour and weekly quota windows.
//!
//! Covers Claude Code / the Claude CLI too: on Windows they run inside the
//! Desktop app's account, so the same token and the same quota apply.

use crate::diaglog::{dbg_log, diag, diagnostics_on}; use super::{Family, FetchError, Limit, LimitKind, LimitPool, LimitWindow, Snapshot};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Token retrieval
// ---------------------------------------------------------------------------

/// Every directory Claude Desktop might keep its data in.
///
/// Two install kinds have to be covered:
/// * the classic installer, which uses `%APPDATA%\Claude` — but different launch
///   contexts can make `dirs::config_dir()` disagree with the `APPDATA` env var,
///   so several roots are tried and validated on disk;
/// * the Microsoft Store (MSIX) build, which never touches the real `%APPDATA%`
///   at all. Inside its package container that path is redirected, so from any
///   other process the data is at
///   `%LOCALAPPDATA%\Packages\<package>\LocalCache\Roaming\Claude`.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(a) = std::env::var("APPDATA") {
        roots.push(PathBuf::from(a));
    }
    if let Some(c) = dirs::config_dir() {
        roots.push(c);
    }
    if let Ok(u) = std::env::var("USERPROFILE") {
        roots.push(PathBuf::from(u).join("AppData").join("Roaming"));
    }
    if let Some(h) = dirs::home_dir() {
        roots.push(h.join("AppData").join("Roaming"));
    }
    roots.dedup();

    let mut dirs_list: Vec<PathBuf> = roots.into_iter().map(|r| r.join("Claude")).collect();
    msix_dirs(&mut dirs_list);
    dirs_list.dedup();
    dirs_list
}

/// Data directories of Store-installed Claude packages.
fn msix_dirs(out: &mut Vec<PathBuf>) {
    let mut locals: Vec<PathBuf> = Vec::new();
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        locals.push(PathBuf::from(l));
    }
    if let Some(l) = dirs::data_local_dir() {
        locals.push(l);
    }
    locals.dedup();

    for local in locals {
        let Ok(packages) = std::fs::read_dir(local.join("Packages")) else {
            continue;
        };
        for entry in packages.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if !(name.starts_with("claude") || name.contains("anthropic")) {
                continue;
            }
            let cache = entry.path().join("LocalCache");
            out.push(cache.join("Roaming").join("Claude"));
            out.push(cache.join("Local").join("Claude"));
        }
    }
}

struct ClaudeFiles {
    local_state: String,
    config: String,
}

/// Read a file, retrying briefly to ride out the moment when Claude Desktop
/// atomically replaces it (write-temp + rename can make the path transiently
/// unreadable).
fn read_with_retry(path: &std::path::Path) -> std::io::Result<String> {
    let mut last = None;
    for _ in 0..5 {
        match std::fs::read_to_string(path) {
            Ok(s) => return Ok(s),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "unreadable")))
}

/// Find the Claude Desktop data dir by actually reading its two files (not just
/// `is_file()`, which is racy). With both a classic and a Store install present,
/// the one whose `config.json` was written last wins — the other is a leftover
/// holding a stale token.
fn find_claude_files() -> Result<ClaudeFiles, String> {
    let mut diag: Vec<String> = Vec::new();
    let mut tried: Vec<String> = Vec::new();
    let mut best: Option<(std::time::SystemTime, ClaudeFiles)> = None;

    for dir in candidate_dirs() {
        // Skip absent directories outright: retrying reads there would add up
        // to half a second each, and there is nothing to race with.
        let Ok(config_meta) = std::fs::metadata(dir.join("config.json")) else {
            tried.push(dir.display().to_string());
            continue;
        };
        let ls = read_with_retry(&dir.join("Local State"));
        let cfg = read_with_retry(&dir.join("config.json"));
        match (ls, cfg) {
            (Ok(local_state), Ok(config)) => {
                let stamp = config_meta
                    .modified()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                if best.as_ref().is_none_or(|(t, _)| stamp > *t) {
                    best = Some((
                        stamp,
                        ClaudeFiles {
                            local_state,
                            config,
                        },
                    ));
                }
            }
            (ls, cfg) => {
                diag.push(format!(
                    "candidate {:?}: Local State={} config.json={}",
                    dir,
                    ls.map(|_| "ok".into()).unwrap_or_else(|e| e.to_string()),
                    cfg.map(|_| "ok".into()).unwrap_or_else(|e| e.to_string()),
                ));
                tried.push(dir.display().to_string());
            }
        }
    }
    if let Some((_, files)) = best {
        return Ok(files);
    }
    // Only touch the log when we actually fail.
    dbg_log("--- find_claude_files() FAILED ---");
    dbg_log(&format!("APPDATA env = {:?}", std::env::var("APPDATA")));
    dbg_log(&format!("dirs::config_dir() = {:?}", dirs::config_dir()));
    for d in &diag {
        dbg_log(d);
    }
    tried.dedup();
    Err(format!(
        "Claude data dir not found. Открыт ли Claude Desktop? Искал: {}",
        tried.join(" | ")
    ))
}

#[derive(Deserialize)]
struct LocalState {
    os_crypt: OsCrypt,
}
#[derive(Deserialize)]
struct OsCrypt {
    encrypted_key: String,
}

/// Unwrap the Chromium AES key from the `Local State` JSON using DPAPI.
#[cfg(windows)]
fn master_key(local_state_json: &str) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    let ls: LocalState =
        serde_json::from_str(local_state_json).map_err(|e| format!("parse Local State: {e}"))?;
    let mut key = B64
        .decode(ls.os_crypt.encrypted_key.as_bytes())
        .map_err(|e| format!("b64 key: {e}"))?;
    // Strip the "DPAPI" prefix (5 bytes).
    if key.len() < 5 || &key[..5] != b"DPAPI" {
        return Err("unexpected key prefix".into());
    }
    let mut blob = key.split_off(5);

    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: blob.len() as u32,
            pbData: blob.as_mut_ptr(),
        };
        let mut out_blob = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(&in_blob, None, None, None, None, 0, &mut out_blob)
            .map_err(|e| format!("CryptUnprotectData: {e}"))?;

        let out = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(out_blob.pbData as *mut _));
        Ok(out)
    }
}

#[cfg(not(windows))]
fn master_key(_local_state_json: &str) -> Result<Vec<u8>, String> {
    Err("Claude token decryption is only implemented on Windows".into())
}

#[derive(Deserialize)]
struct TokenEntry {
    token: String,
    #[serde(default)]
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
    #[serde(default)]
    #[serde(rename = "rateLimitTier")]
    rate_limit_tier: Option<String>,
    /// Milliseconds since the epoch.
    #[serde(default)]
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
}

pub struct OauthToken {
    pub access: String,
    pub subscription: Option<String>,
    pub tier: Option<String>,
    /// Which cache it came from — "V2" or "V1", for the diagnostics log.
    pub source: &'static str,
}

/// Decrypt `config.json`'s `oauth:tokenCache` into all usable tokens, ordered by
/// preference (subscription + `user:profile` scope first). config.json can hold
/// several OAuth entries (different app registrations) — some are stale/rate-
/// limited/wrong-scope, so `fetch()` tries them in order until one works.
/// Decrypt one `v10` Chromium blob into the map of scope → token entry.
fn decrypt_cache(
    cipher: &Aes256Gcm,
    encoded: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let raw = B64
        .decode(encoded.as_bytes())
        .map_err(|e| format!("b64 cache: {e}"))?;
    if raw.len() < 3 + 12 + 16 || &raw[..3] != b"v10" {
        return Err("unexpected token cache format".into());
    }
    let plain = cipher
        .decrypt(Nonce::from_slice(&raw[3..15]), &raw[15..])
        .map_err(|_| "AES-GCM decrypt failed".to_string())?;
    serde_json::from_slice(&plain).map_err(|e| format!("parse token json: {e}"))
}

pub fn load_tokens() -> Result<Vec<OauthToken>, String> {
    let files = find_claude_files()?;
    let key_bytes = master_key(&files.local_state)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));

    let cfg: serde_json::Value =
        serde_json::from_str(&files.config).map_err(|e| format!("parse config.json: {e}"))?;

    let now_ms = Utc::now().timestamp_millis();
    let mut scored: Vec<(u8, OauthToken)> = Vec::new();
    let mut last_err = String::new();

    // Newer Claude Desktop builds keep the live tokens in `oauth:tokenCacheV2`
    // and leave the old cache behind. Both are read: V1 alone still works today,
    // but its entries are the stale ones — including tokens that authenticate
    // yet carry no plan, which is why the header could lose "Max 20×".
    for (cache_key, fresh) in [("oauth:tokenCacheV2", true), ("oauth:tokenCache", false)] {
        let Some(enc) = cfg.get(cache_key).and_then(|v| v.as_str()) else {
            continue;
        };
        let json = match decrypt_cache(&cipher, enc) {
            Ok(j) => j,
            Err(e) => {
                last_err = format!("{cache_key}: {e}");
                continue;
            }
        };

        for (scope_key, val) in json.iter() {
            let entry: TokenEntry = match serde_json::from_value(val.clone()) {
                Ok(e) => e,
                Err(_) => continue,
            };
            // The usage endpoint needs the profile scope; inference-only tokens 403.
            if !scope_key.contains("user:profile") {
                continue;
            }
            // An expired entry would only cost a 401 round-trip.
            if entry.expires_at.is_some_and(|ms| ms <= now_ms) {
                continue;
            }
            let has_sub = entry.subscription_type.is_some();
            let score = (fresh as u8) * 4 + (has_sub as u8) * 2 + 1;
            scored.push((
                score,
                OauthToken {
                    access: entry.token,
                    subscription: entry.subscription_type,
                    tier: entry.rate_limit_tier,
                    source: if fresh { "V2" } else { "V1" },
                },
            ));
        }
    }
    if scored.is_empty() && !last_err.is_empty() {
        return Err(last_err);
    }
    scored.sort_by_key(|a| std::cmp::Reverse(a.0)); // stable: keeps file order within a score
    let tokens: Vec<OauthToken> = scored.into_iter().map(|(_, t)| t).collect();
    if tokens.is_empty() {
        return Err("no usable OAuth token (profile scope) in config.json".into());
    }
    Ok(tokens)
}

// ---------------------------------------------------------------------------
// Claude CLI token (`~/.claude.json`)
// ---------------------------------------------------------------------------

/// A machine with only the Claude CLI installed has no Desktop `config.json` to
/// decrypt, but the CLI keeps its own OAuth token in plain JSON at
/// `~/.claude.json` (`oauthAccount.accessToken`). It authenticates the same
/// account, so the same usage endpoint answers for it. The CLI refreshes the
/// token itself; we re-read the file every poll, exactly like the Codex module
/// re-reads `auth.json`.
fn cli_config_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(u) = std::env::var("USERPROFILE") {
        paths.push(PathBuf::from(u).join(".claude.json"));
    }
    if let Some(h) = dirs::home_dir() {
        paths.push(h.join(".claude.json"));
    }
    // CLAUDE_CONFIG_DIR relocates the CLI's config directory on some setups.
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            paths.push(PathBuf::from(dir).join(".claude.json"));
        }
    }
    paths.dedup();
    paths
}

#[derive(Deserialize)]
struct CliConfig {
    #[serde(default, rename = "oauthAccount")]
    oauth_account: Option<CliOauthAccount>,
}

#[derive(Deserialize)]
struct CliOauthAccount {
    #[serde(default, rename = "accessToken")]
    access_token: Option<String>,
    /// Milliseconds since the epoch.
    #[serde(default, rename = "expiresAt")]
    expires_at: Option<i64>,
    /// Space-separated scope list. Absent on older CLI versions.
    #[serde(default)]
    scope: Option<String>,
}

/// Parse the Claude CLI config into a usable token, or say why not. Pure over
/// the file text so the expiry/scope rules are testable without touching disk.
fn parse_cli_token(raw: &str, now_ms: i64) -> Result<OauthToken, String> {
    let cfg: CliConfig =
        serde_json::from_str(raw).map_err(|e| format!("parse .claude.json: {e}"))?;
    let Some(acct) = cfg.oauth_account else {
        return Err("no oauthAccount in .claude.json".into());
    };
    let access = acct
        .access_token
        .filter(|t| !t.trim().is_empty())
        .ok_or("oauthAccount has no accessToken")?;
    // `expiresAt` is normally ms; accept a bare-seconds value from an older or
    // hand-edited file rather than misreading it as already expired.
    if let Some(raw_exp) = acct.expires_at {
        let exp_ms = if raw_exp < 1_000_000_000_000 {
            raw_exp * 1000
        } else {
            raw_exp
        };
        if exp_ms <= now_ms {
            return Err("accessToken expired".into());
        }
    }
    // The usage endpoint needs the profile scope. Honour a scope field when
    // present; without one, assume the CLI's default login scope, which
    // includes `user:profile`.
    if let Some(scope) = &acct.scope {
        if !scope.split_whitespace().any(|s| s == "user:profile") {
            return Err("scope lacks user:profile".into());
        }
    }
    Ok(OauthToken {
        access,
        subscription: None,
        tier: None,
        source: "cli",
    })
}

/// Read `~/.claude.json` and turn it into the token the usage endpoint needs.
fn load_cli_token() -> Result<OauthToken, String> {
    let mut tried: Vec<String> = Vec::new();
    for path in cli_config_candidates() {
        let raw = match read_with_retry(&path) {
            Ok(r) => r,
            Err(e) => {
                tried.push(format!("{} ({e})", path.display()));
                continue;
            }
        };
        match parse_cli_token(&raw, Utc::now().timestamp_millis()) {
            Ok(t) => {
                dbg_log(&format!("claude cli: token from {}", path.display()));
                return Ok(t);
            }
            Err(e) => tried.push(format!("{}: {e}", path.display())),
        }
    }
    Err(format!("Claude CLI config not usable ({})", tried.join(" | ")))
}

// ---------------------------------------------------------------------------
// Usage endpoint
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct UsageWindow {
    #[serde(default)]
    utilization: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Deserialize)]
struct UsageResponse {
    #[serde(default)]
    five_hour: Option<UsageWindow>,
    #[serde(default)]
    seven_day: Option<UsageWindow>,
}

fn parse_ts(s: &Option<String>) -> Option<DateTime<Utc>> {
    let s = s.as_ref()?;
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// The access token that last returned 200, so we try it first next poll and
/// don't repeatedly hit stale/rate-limited entries.
static LAST_GOOD: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// What a failed request means for the rest of the poll.
enum Failure {
    /// This token is not the one — try the next entry.
    NextToken,
    /// Nothing else will work this poll (transport error, bad response).
    Stop,
    /// The endpoint refused this request as too frequent. Carries the server's
    /// own `Retry-After`, in seconds, when it sent one. Another entry may still
    /// be answered — the refusal is not always account-wide — but if they all
    /// come back like this, asking again soon only deepens it.
    RateLimited(Option<u64>),
}

/// One usage request.
fn request_usage(access: &str) -> Result<UsageResponse, (Failure, String)> {
    match ureq::get("https://api.anthropic.com/api/oauth/usage")
        .set("Authorization", &format!("Bearer {access}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("anthropic-version", "2023-06-01")
        .set("User-Agent", "Tokpaek/0.1")
        .timeout(std::time::Duration::from_secs(20))
        .call()
    {
        Ok(resp) => resp
            .into_json()
            .map_err(|e| (Failure::Stop, format!("parse usage: {e}"))),
        Err(ureq::Error::Status(429, resp)) => {
            let retry_after = resp
                .header("retry-after")
                .and_then(|v| v.trim().parse::<u64>().ok());
            Err((
                Failure::RateLimited(retry_after),
                match retry_after {
                    Some(s) => format!("status 429, retry-after {s}s"),
                    None => "status 429".to_string(),
                },
            ))
        }
        Err(ureq::Error::Status(code, _)) => {
            // 401/403 are per-token: config.json holds entries that are
            // inference-only or stale, and the next one may still work.
            let what = if matches!(code, 401 | 403) {
                Failure::NextToken
            } else {
                Failure::Stop
            };
            Err((what, format!("status {code}")))
        }
        // Transport error (offline, DNS, TLS): same for every token → stop.
        Err(e) => Err((Failure::Stop, format!("network: {e}"))),
    }
}

/// Which address the usage endpoint currently resolves to. Worth a line in the
/// log: a machine reaching Anthropic through an unblocking relay shares that
/// relay's rate limit with everyone else behind it, which looks exactly like a
/// broken token from the inside.
fn endpoint_ip() -> String {
    use std::net::ToSocketAddrs;
    ("api.anthropic.com", 443)
        .to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|| "unresolved".into())
}

/// Fetch a fresh snapshot: reads tokens from disk and tries each against the
/// usage endpoint until one succeeds. Waiting after a 429 is not this
/// module's job — it reports `rate_limited` plus the server's `Retry-After`,
/// and the poller's `RetryPolicy` decides the cooldown.
pub fn fetch() -> Result<Snapshot, FetchError> {
    let mut tokens = match load_tokens() {
        Ok(t) => t,
        // No Claude Desktop token cache: a machine with only the Claude CLI
        // still has a token in ~/.claude.json — fall back to it.
        Err(desktop_err) => match load_cli_token() {
            Ok(t) => {
                dbg_log(&format!(
                    "claude: no Desktop token ({desktop_err}), using CLI token"
                ));
                vec![t]
            }
            Err(cli_err) => return Err(format!("{desktop_err}; {cli_err}").into()),
        },
    };

    // Try the previously-working token first.
    if let Some(good) = LAST_GOOD.lock().unwrap().clone() {
        if let Some(pos) = tokens.iter().position(|t| t.access == good) {
            tokens.swap(0, pos);
        }
    }

    if diagnostics_on() {
        let kinds: Vec<String> = tokens
            .iter()
            .map(|t| format!("{}/{}", t.source, t.subscription.as_deref().unwrap_or("-")))
            .collect();
        diag(&format!(
            "claude: {} token(s) [{}], api.anthropic.com -> {}",
            tokens.len(),
            kinds.join(", "),
            endpoint_ip()
        ));
    }

    let mut last_err = "no token tried".to_string();
    // Set once some entry was refused as too frequent, holding the longest
    // `Retry-After` any of them asked for. Only a poll where *nothing* got
    // through reports a throttle.
    let mut throttled: Option<Option<u64>> = None;
    for tok in &tokens {
        let started = std::time::Instant::now();
        match request_usage(&tok.access) {
            Ok(usage) => {
                *LAST_GOOD.lock().unwrap() = Some(tok.access.clone());
                diag(&format!(
                    "claude: 200 in {} ms (token {}/{})",
                    started.elapsed().as_millis(),
                    tok.source,
                    tok.subscription.as_deref().unwrap_or("-")
                ));
                return Ok(build_snapshot(usage));
            }
            Err((what, msg)) => {
                dbg_log(&format!(
                    "claude: token {}/{} -> {msg} in {} ms",
                    tok.source,
                    tok.subscription.as_deref().unwrap_or("-"),
                    started.elapsed().as_millis()
                ));
                last_err = msg;
                match what {
                    Failure::NextToken => continue,
                    Failure::Stop => break,
                    // Not necessarily account-wide: config.json can hold an
                    // entry the endpoint keeps refusing while the next one is
                    // answered in the same second. Treating the first 429 as
                    // final left the strip on a stale number for a whole day.
                    Failure::RateLimited(retry_after) => {
                        let longest = throttled.unwrap_or(None).max(retry_after);
                        throttled = Some(longest);
                        continue;
                    }
                }
            }
        }
    }
    *LAST_GOOD.lock().unwrap() = None;
    if let Some(retry_after) = throttled {
        diag("claude: every token rate limited");
        return Err(FetchError::rate_limited_for(
            "лимит запросов Claude",
            retry_after,
        ));
    }
    Err(format!("usage request: {last_err}").into())
}

fn build_snapshot(usage: UsageResponse) -> Snapshot {
    let now = Utc::now();
    let mut limits = Vec::new();

    // A window that has not started yet arrives as `resets_at: null`. Keep the
    // row — dropping it made the 5-hour limit vanish from the strip until the
    // first request of the session.
    //
    // The start is *derived*, so it can land ahead of `now`: on a machine whose
    // clock runs behind, every reset looks further out than its window is long
    // (G25). `ending_at` decides what to do with a start it cannot place.
    if let Some(w) = usage.five_hour {
        limits.push(Limit {
            title: "5-hour limit".into(),
            kind: LimitKind::Session,
            pool: LimitPool::Own,
            used_percent: w.utilization.unwrap_or(0.0) as f32,
            window: parse_ts(&w.resets_at)
                .map(|reset| LimitWindow::ending_at(reset, chrono::Duration::hours(5), now)),
            weekly: None,
        });
    }
    if let Some(w) = usage.seven_day {
        limits.push(Limit {
            title: "Weekly · all models".into(),
            kind: LimitKind::Weekly,
            pool: LimitPool::Own,
            used_percent: w.utilization.unwrap_or(0.0) as f32,
            window: parse_ts(&w.resets_at)
                .map(|reset| LimitWindow::ending_at(reset, chrono::Duration::days(7), now)),
            weekly: None,
        });
    }
    for l in &limits {
        diag(&format!("claude: {}", describe(l, now)));
    }

    Snapshot {
        family: Family::Claude,
        limits,
    }
}

/// One limit as the log wants it: the numbers the strip drew and where they came
/// from, so "why is that row shaped like that" is answerable after the fact.
fn describe(l: &Limit, now: DateTime<Utc>) -> String {
    let when = match l.window {
        None => "no window".to_string(),
        Some(w) => format!(
            "resets {} (in {} min), start {}",
            w.resets_at.to_rfc3339(),
            (w.resets_at - now).num_minutes(),
            match w.start {
                Some(s) => s.to_rfc3339(),
                None => "unplaceable — reset is further out than the window".into(),
            }
        ),
    };
    format!("{} {:.0}%, {when}", l.title, l.used_percent)
}

#[cfg(test)]
mod tests {
    /// A 5-hour window only starts at the first request of a session; until
    /// then the service sends `resets_at: null`. The row must still appear.
    #[test]
    fn a_window_that_has_not_started_still_makes_a_row() {
        let usage: super::UsageResponse = serde_json::from_str(
            r#"{"five_hour":{"utilization":0.0,"resets_at":null},
                "seven_day":{"utilization":73.0,"resets_at":"2026-08-24T02:59:59+00:00"}}"#,
        )
        .expect("parse");

        let snap = super::build_snapshot(usage);
        assert_eq!(snap.limits.len(), 2, "both rows belong on screen");
        assert_eq!(snap.limits[0].title, "5-hour limit");
        assert!(
            snap.limits[0].window.is_none(),
            "no clock for a window that has not started"
        );
        assert!(snap.limits[1].window.is_some(), "the weekly one is running");
    }

    /// A `five_hour` counter that clears in seven — what a local clock hours
    /// behind does to every countdown (G25). The row keeps its reset time,
    /// claims no start it cannot know, and puts its time marker at the beginning
    /// of the bar, so what is already spent reads as spend ahead of it (D10).
    #[test]
    fn a_reset_beyond_the_window_leaves_the_start_unplaced() {
        let reset = chrono::Utc::now() + chrono::Duration::minutes(7 * 60);
        let usage: super::UsageResponse = serde_json::from_str(&format!(
            r#"{{"five_hour":{{"utilization":12.0,"resets_at":"{}"}}}}"#,
            reset.to_rfc3339()
        ))
        .expect("parse");

        let snap = super::build_snapshot(usage);
        let w = snap.limits[0]
            .window
            .expect("the reset time is still known");
        assert_eq!(w.resets_at.timestamp(), reset.timestamp());
        assert!(w.start.is_none(), "a start in the future is not a start");
        let now = chrono::Utc::now();
        assert!(
            w.elapsed_frac(now).is_none(),
            "and nothing that can be called progress"
        );
        assert_eq!(w.marker_frac(now), 0.0, "the marker goes to the left edge");
        assert!(
            snap.limits[0].used_percent / 100.0 > w.marker_frac(now) + 0.02,
            "so the spend is past the marker — the bar reads as overspend"
        );
    }

    /// The CLI token parser: the rules a ~/.claude.json token must pass before
    /// the usage endpoint is asked. Verifies the expiry normalization (a
    /// bare-seconds value is not misread as already-expired) and the profile
    /// scope gate.
    #[test]
    fn cli_token_is_parsed_and_gated() {
        let now_ms = 1_800_000_000_000; // a fixed, real-world-shaped "now"

        // A normal ms-expiry token with the profile scope parses.
        let ok = super::parse_cli_token(
            r#"{"oauthAccount":{"accessToken":"tok","expiresAt":1900000000000,"scope":"claude:all user:profile"}}"#,
            now_ms,
        )
        .expect("valid");
        assert_eq!(ok.access, "tok");
        assert_eq!(ok.source, "cli");

        // No scope field (older CLI): assume the default login scope.
        assert!(super::parse_cli_token(
            r#"{"oauthAccount":{"accessToken":"tok","expiresAt":1900000000000}}"#,
            now_ms,
        )
        .is_ok());

        // Expired (ms) is rejected.
        assert!(super::parse_cli_token(
            r#"{"oauthAccount":{"accessToken":"tok","expiresAt":1000000}}"#,
            now_ms,
        )
        .is_err());

        // A seconds-typed expiry in the future survives normalization.
        let secs = (now_ms / 1000) + 3600;
        assert!(super::parse_cli_token(
            &format!(r#"{{"oauthAccount":{{"accessToken":"tok","expiresAt":{secs}}}}}"#),
            now_ms,
        )
        .is_ok());

        // A scope without user:profile cannot answer the usage endpoint.
        assert!(super::parse_cli_token(
            r#"{"oauthAccount":{"accessToken":"tok","expiresAt":1900000000000,"scope":"claude:all"}}"#,
            now_ms,
        )
        .is_err());

        // Missing oauthAccount / empty token are not usable.
        assert!(super::parse_cli_token(r#"{}"#, now_ms).is_err());
        assert!(super::parse_cli_token(
            r#"{"oauthAccount":{"accessToken":""}}"#,
            now_ms,
        )
        .is_err());
    }

    /// Not a test — the tool that answers "what does the service actually say".
    /// Ignored, because it spends a real request against the live endpoint:
    /// `cargo test --release probe_usage_body -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn probe_usage_body() {
        println!("now utc = {}", chrono::Utc::now().to_rfc3339());
        for tok in &super::load_tokens().expect("tokens") {
            match ureq::get("https://api.anthropic.com/api/oauth/usage")
                .set("Authorization", &format!("Bearer {}", tok.access))
                .set("anthropic-beta", "oauth-2025-04-20")
                .set("anthropic-version", "2023-06-01")
                .set("User-Agent", "Tokpaek/0.1")
                .timeout(std::time::Duration::from_secs(20))
                .call()
            {
                Ok(resp) => {
                    println!(
                        "{}: 200 {}",
                        tok.source,
                        resp.into_string().unwrap_or_default()
                    );
                    return;
                }
                Err(ureq::Error::Status(code, resp)) => {
                    let retry = resp.header("retry-after").map(str::to_string);
                    println!("{}: status {code}, retry-after {retry:?}", tok.source);
                }
                Err(e) => println!("{}: {e}", tok.source),
            }
        }
    }
}
