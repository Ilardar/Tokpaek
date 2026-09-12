//! Which tool family the user is working in right now.
//!
//! The rule is "whatever was in the foreground last": we look at the foreground
//! window's process, and — when that process is only a host (a terminal, or an
//! editor with an embedded terminal) — at what it is running underneath, so a
//! CLI session counts as its own family.
//!
//! Detection runs on its own thread (`spawn_watcher`), woken instantly by the
//! WinEvent foreground hook and on a 2-second timer for what no event
//! announces (a CLI started inside the terminal already in focus). The UI only
//! reads `published()`. Classification (`classify_exe`) and the family choice
//! (`pick_family`) are pure functions, testable without Win32.

use crate::providers::Family;
use crate::winproc;

/// Result of one detection pass, published by the watcher thread. All three
/// facts cross the same seam with the same freshness — the frame loop must not
/// probe Win32 itself, or its answers can disagree with these within a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveStatus {
    /// At least one watched app has a visible, non-minimized window: the
    /// Smart Focus visibility fact.
    pub is_ai: bool,
    /// The *foreground* window belongs to a watched app: the Smart Focus
    /// z-band fact (ride above it only while the user is looking at it).
    pub foreground_watched: bool,
    /// Detected family, if a specific tool or CLI was recognized.
    pub family: Option<Family>,
}

/// How an exe name classifies, without any Win32 involved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExeKind {
    /// The window *is* the tool.
    Direct(Family),
    /// A supported editor: AI-active workspace, may host a CLI.
    Editor,
    /// A terminal or shell: AI-active workspace, may host a CLI.
    Terminal,
    /// Neither: the widget has no business staying on screen.
    Other,
}

/// Exe name → family, for windows that *are* the tool.
fn direct(exe: &str) -> Option<Family> {
    let lower = exe.to_lowercase();
    let base = lower.strip_suffix(".exe").unwrap_or(&lower);
    match base {
        "claude" => Some(Family::Claude),
        "codex" | "chatgpt" => Some(Family::Codex),
        "antigravity" | "antigravity ide" | "agy" | "opencode" => Some(Family::Antigravity),
        _ => None,
    }
}

/// Supported code editors that count as AI-active workspaces.
fn is_editor(exe: &str) -> bool {
    let lower = exe.to_lowercase();
    let base = lower.strip_suffix(".exe").unwrap_or(&lower);
    matches!(
        base,
        "code"
            | "code - exploration"
            | "code - insiders"
            | "vscodium"
            | "cursor"
            | "windsurf"
            | "zed"
            | "opencode"
            | "devenv"
            | "idea"
            | "idea64"
            | "pycharm"
            | "pycharm64"
            | "webstorm"
            | "webstorm64"
            | "clion"
            | "clion64"
            | "rider"
            | "rider64"
            | "rustrover"
            | "rustrover64"
            | "goland"
            | "goland64"
            | "phpstorm"
            | "phpstorm64"
            | "datagrip"
            | "datagrip64"
            | "androidstudio64"
            | "studio64"
            | "sublime_text"
            | "notepad++"
            | "kate"
            | "neovim-qt"
            | "nvim-qt"
    )
}

/// Console hosts own the window but not the CLI: `conhost.exe` is a *child* of
/// the program whose console it draws, so the search has to start one level up.
fn is_console_host(exe: &str) -> bool {
    let lower = exe.to_lowercase();
    let base = lower.strip_suffix(".exe").unwrap_or(&lower);
    matches!(base, "conhost" | "openconsole")
}

/// Windows that merely *host* a terminal / shell: look at their process tree instead.
fn is_terminal(exe: &str) -> bool {
    let lower = exe.to_lowercase();
    let base = lower.strip_suffix(".exe").unwrap_or(&lower);
    matches!(
        base,
        "windowsterminal"
            | "windowsterminalpreview"
            | "openconsole"
            | "conhost"
            | "cmd"
            | "powershell"
            | "pwsh"
            | "bash"
            | "sh"
            | "mintty"
            | "alacritty"
            | "wezterm-gui"
            | "hyper"
            | "tabby"
            | "kitty"
            | "warp"
            | "ghostty"
            | "conemu64"
            | "cmder"
    )
}

/// The pure classification: an exe name decides everything except what runs
/// underneath a host window.
pub fn classify_exe(exe: &str) -> ExeKind {
    if let Some(f) = direct(exe) {
        ExeKind::Direct(f)
    } else if is_editor(exe) {
        ExeKind::Editor
    } else if is_terminal(exe) {
        ExeKind::Terminal
    } else {
        ExeKind::Other
    }
}

/// Which family wins among the CLI matches found in a process tree.
///
/// The previously shown family keeps priority while it is still alive anywhere
/// in the tree: a `codex` started in another tab must not yank the widget away
/// from the `claude` session the user is looking at. Only when it is gone does
/// the most recently started match (highest pid) take over.
pub fn pick_family(matches: &[(u32, Family)], prev: Option<Family>) -> Option<Family> {
    if let Some(prev) = prev {
        if matches.iter().any(|(_, f)| *f == prev) {
            return Some(prev);
        }
    }
    matches.iter().max_by_key(|(pid, _)| *pid).map(|(_, f)| *f)
}

/// The family-switch debounce: a *new* family must be seen on two consecutive
/// checks before the widget follows it; the family already shown (or none at
/// all) commits immediately. Returns (family to show, pending to remember).
pub fn debounce(
    raw: Option<Family>,
    shown: Option<Family>,
    pending: Option<Family>,
) -> (Option<Family>, Option<Family>) {
    match raw {
        // Nothing in the tree: commit as-is.
        None => (None, None),
        // What we already show, or a second consecutive sighting of the same
        // new family: commit.
        Some(f) if Some(f) == shown || pending == Some(f) => (Some(f), None),
        // First sighting of a new family: hold the shown one, remember the
        // candidate.
        Some(f) => (shown, Some(f)),
    }
}

/// The apps the widget watches. It draws over their windows while at least
/// one of them is open and not minimized — no matter what sits in the
/// foreground. Exe base names, lower-cased.
const WATCHED_EXES: [&str; 3] = ["antigravity", "claude", "chatgpt"];

/// Is an exe one of the watched apps?
pub fn is_watched(exe: &str) -> bool {
    let lower = exe.to_lowercase();
    let base = lower.strip_suffix(".exe").unwrap_or(&lower);
    WATCHED_EXES.contains(&base)
}

/// True when at least one watched app has a visible, non-minimized top-level
/// window. This is the new Smart Focus rule: the widget stays on screen while
/// Antigravity, Claude or the ChatGPT desktop app is open, even when the
/// foreground is something else — a foreground-based rule hid the widget the
/// moment the user alt-tabbed, which read as a glitch.
#[cfg(windows)]
fn watched_apps_open() -> bool {
    watched_window_frontmost().is_some()
}

#[cfg(not(windows))]
fn watched_apps_open() -> bool {
    false
}

/// The frontmost visible, non-minimized window of a watched app (EnumWindows
/// walks top-level windows in z-order, so the first match is the front one).
#[cfg(windows)]
fn watched_window_frontmost() -> Option<isize> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    };

    struct Search {
        my_pid: u32,
        found: Option<isize>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let s = &mut *(lparam.0 as *mut Search);
        if s.found.is_some() {
            return BOOL(0);
        }
        // Our own windows are not "the app being used".
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == s.my_pid {
            return BOOL(1);
        }
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return BOOL(1);
        }
        if let Some(exe) = process_name(pid) {
            if is_watched(&exe) {
                s.found = Some(hwnd.0 as isize);
                return BOOL(0);
            }
        }
        BOOL(1)
    }

    let mut search = Search {
        my_pid: unsafe { GetCurrentProcessId() },
        found: None,
    };
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut search as *mut _ as isize));
    }
    search.found
}

#[cfg(not(windows))]
pub fn watched_window_hwnd() -> Option<isize> {
    None
}

/// How long a failed foreground read may stand in for the last known status
/// before the detector admits it knows nothing. An unknown state must not hide
/// the widget (the user may well be working) and must not switch families.
const UNKNOWN_AFTER_SECS: f64 = 3.0;

/// The detection state machine, driven by `tick` from the watcher thread with
/// its own wall clock — never from the render loop, so its cadence does not
/// depend on repaint periods.
pub struct Detector {
    last_pid: u32,
    last_exe: String,
    last_status: ActiveStatus,
    /// A tree-derived family seen once but not yet published: publishing needs
    /// the same answer twice in a row, so a one-tick tree flicker (a CLI
    /// restarting, a shell spawning children) cannot flip the widget.
    pending_family: Option<Family>,
    unknown_since: f64,
}

impl Default for Detector {
    fn default() -> Self {
        Self {
            last_pid: 0,
            last_exe: String::new(),
            last_status: ActiveStatus {
                is_ai: true,
                foreground_watched: false,
                family: None,
            },
            pending_family: None,
            unknown_since: f64::MAX,
        }
    }
}

impl Detector {
    /// One detection pass against the live OS.
    pub fn tick(&mut self, now: f64) -> ActiveStatus {
        let facts = Facts {
            watched_open: watched_apps_open(),
            foreground_pid: foreground_pid(),
            own_pid: std::process::id(),
            exe_name: |pid| process_name(pid),
            tree: |pid, hop| tree_matches(pid, hop),
        };
        self.tick_with(facts, now)
    }

    /// The detection state machine, pure over injected facts: the watcher
    /// thread plugs in Win32, tests plug in scripted closures.
    ///
    /// Three answers, one freshness:
    /// - `is_ai` — Smart Focus visibility: a watched app is open, not minimized;
    /// - `foreground_watched` — Smart Focus z-band: the foreground window is
    ///   itself a watched app's;
    /// - `family` — which quota to show: the foreground app when it is one of
    ///   the watched tools, else a watched CLI under a terminal/editor, else
    ///   whatever we last settled on.
    pub fn tick_with<F, T>(&mut self, f: Facts<F, T>, now: f64) -> ActiveStatus
    where
        F: Fn(u32) -> Option<String>,
        T: Fn(u32, bool) -> Vec<(u32, Family)>,
    {
        let Some(pid) = f.foreground_pid else {
            // No foreground to read (a switch in flight): the visibility
            // answer stands on its own; the family stays put.
            return self.unknown(now, "no foreground window", f.watched_open);
        };

        // Our own window in the foreground (the user is dragging the widget or
        // clicking the settings): keep whatever we last knew — the user's
        // attention has not moved anywhere.
        if pid == f.own_pid {
            self.unknown_since = f64::MAX;
            return ActiveStatus {
                is_ai: f.watched_open,
                foreground_watched: self.last_status.foreground_watched,
                family: self.last_status.family,
            };
        }

        // Re-read the exe only when the foreground process changed; the name
        // of a live pid cannot change under us.
        if pid != self.last_pid {
            match (f.exe_name)(pid) {
                Some(name) => {
                    self.last_exe = name.to_lowercase();
                    self.last_pid = pid;
                    self.pending_family = None;
                }
                // A process we cannot name (elevation, a race with exit): we
                // know the foreground changed, so the old answer is dead — but
                // not what replaced it.
                None => {
                    return self.unknown(now, "unreadable foreground process", f.watched_open);
                }
            }
        }

        let foreground_watched = is_watched(&self.last_exe);

        // Which family to show: the foreground app when it is one of the
        // watched tools, otherwise a watched CLI running under a terminal or
        // editor in the foreground; anything else leaves the family alone.
        let family = match classify_exe(&self.last_exe) {
            ExeKind::Direct(fam) => Some(fam),
            ExeKind::Editor | ExeKind::Terminal => {
                let hop_parent = is_console_host(&self.last_exe);
                let matches = (f.tree)(self.last_pid, hop_parent);
                let raw = pick_family(&matches, None);
                let (debounced, pending) =
                    debounce(raw, self.last_status.family, self.pending_family);
                self.pending_family = pending;
                pick_family(&matches, debounced)
            }
            // Foreground is neither a watched tool nor a host: keep showing
            // whatever family we last settled on.
            ExeKind::Other => self.last_status.family,
        };

        self.publish(ActiveStatus {
            is_ai: f.watched_open,
            foreground_watched,
            family,
        })
    }

    fn publish(&mut self, status: ActiveStatus) -> ActiveStatus {
        self.last_status = status;
        self.unknown_since = f64::MAX;
        status
    }

    /// The foreground could not be read. Hold the last family *and* z-band
    /// fact for a moment — window switches race the hook, and dropping the
    /// band instantly would sink the widget under a watched app it still
    /// sits on top of — then drop both; visibility does not depend on the
    /// foreground at all and stays as measured.
    fn unknown(&mut self, now: f64, why: &str, visible: bool) -> ActiveStatus {
        crate::diaglog::diag(&format!("active: unknown foreground ({why})"));
        if self.unknown_since == f64::MAX {
            self.unknown_since = now;
        }
        let (family, foreground_watched) = if now - self.unknown_since > UNKNOWN_AFTER_SECS {
            self.last_pid = 0; // force a re-read once it becomes readable
            self.last_status.family = None;
            self.last_status.foreground_watched = false;
            (None, false)
        } else {
            (
                self.last_status.family,
                self.last_status.foreground_watched,
            )
        };
        ActiveStatus {
            is_ai: visible,
            foreground_watched,
            family,
        }
    }
}

/// The facts one detection pass needs, injected so the state machine is
/// testable without Win32.
pub struct Facts<F, T> {
    pub watched_open: bool,
    pub foreground_pid: Option<u32>,
    pub own_pid: u32,
    pub exe_name: F,
    pub tree: T,
}

/// The latest answer, published by the watcher thread for the UI to read.
/// `is_ai: true` until the first tick: the widget must not blink away at
/// startup while detection is still cold.
static PUBLISHED: std::sync::Mutex<ActiveStatus> = std::sync::Mutex::new(ActiveStatus {
    is_ai: true,
    foreground_watched: false,
    family: None,
});

/// The UI's only window into detection.
pub fn published() -> ActiveStatus {
    *PUBLISHED.lock().unwrap_or_else(|e| e.into_inner())
}

fn publish_status(s: ActiveStatus) {
    if let Ok(mut guard) = PUBLISHED.lock() {
        *guard = s;
    }
}

/// Wall clock in seconds for the detector.
fn now_secs() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(std::time::Instant::now);
    start.elapsed().as_secs_f64()
}

/// Timer id and period of the watcher's own re-check while a host window
/// (terminal/editor) may be starting or stopping CLIs underneath us.
#[cfg(windows)]
const DETECT_TIMER_ID: usize = 0x9201;
/// Fast enough that minimizing/restoring a watched app feels instant; EnumWindows
/// stops at the first watched window, so the scan is cheap.
#[cfg(windows)]
const DETECT_TIMER_MS: u32 = 500;

#[cfg(windows)]
static WAKE_CTX: std::sync::OnceLock<eframe::egui::Context> = std::sync::OnceLock::new();

#[cfg(windows)]
unsafe extern "system" fn foreground_event_proc(
    _h_win_event_hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    event: u32,
    _hwnd: windows::Win32::Foundation::HWND,
    _id_object: i32,
    _id_child: i32,
    _id_event_thread: u32,
    _dwms_event_time: u32,
) {
    use windows::Win32::UI::WindowsAndMessaging::{
        EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART,
    };
    // React to what actually changes visibility: a new foreground window, and
    // any window being minimized or restored. Everything else in the hooked
    // range is ignored — the 500 ms timer is the backstop.
    if !matches!(
        event,
        EVENT_SYSTEM_FOREGROUND | EVENT_SYSTEM_MINIMIZESTART | EVENT_SYSTEM_MINIMIZEEND
    ) {
        return;
    }
    // The foreground changed: re-detect right now instead of waiting for the
    // timer, and wake the UI unconditionally — the frame loop's z-order
    // decision (topmost only while a watched app is in front) depends on the
    // foreground even when the published status does not change.
    detect_and_publish();
    if let Some(ctx) = WAKE_CTX.get() {
        ctx.request_repaint();
    }
}

/// One detection pass, publishing the result. Shared by the WinEvent hook and
/// the watcher's timer; both run on the watcher thread, so the mutex only
/// guards against a tick re-entering while Win32 pumps messages.
#[cfg(windows)]
fn detect_and_publish() {
    static DETECTOR: std::sync::OnceLock<std::sync::Mutex<Detector>> = std::sync::OnceLock::new();
    let detector = DETECTOR.get_or_init(|| std::sync::Mutex::new(Detector::default()));
    let Ok(mut d) = detector.lock() else {
        return;
    };
    let before = published();
    let status = d.tick(now_secs());
    drop(d);
    publish_status(status);
    if status != before {
        if let Some(ctx) = WAKE_CTX.get() {
            ctx.request_repaint();
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn detect_tick_proc(
    _hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    _timer_id: usize,
    _time: u32,
) {
    use windows::Win32::UI::WindowsAndMessaging::WM_TIMER;
    if msg == WM_TIMER {
        detect_and_publish();
    }
}

#[cfg(windows)]
unsafe extern "system" fn detect_wnd_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Detection runs here, off the render loop: the WinEvent hook fires the
/// moment the foreground changes, and a 2-second timer catches what no event
/// announces — a CLI started or stopped inside the terminal already in focus.
#[cfg(windows)]
pub fn spawn_watcher(ctx: eframe::egui::Context) {
    let _ = WAKE_CTX.set(ctx);
    std::thread::spawn(move || {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent};
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassW, SetTimer,
            TranslateMessage, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, MSG, WNDCLASSW,
            WINEVENT_OUTOFCONTEXT,
        };

        unsafe {
            // One hook over the range containing the events that change
            // visibility (foreground change, minimize, restore); the callback
            // ignores everything else.
            let hook = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_MINIMIZEEND,
                None,
                Some(foreground_event_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            if hook.is_invalid() {
                return;
            }

            // A message-only window to hang the timer on: GetMessageW returns
            // for WM_TIMER even with no visible UI of our own.
            let class = windows::core::w!("TokpaekDetectWnd");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(detect_wnd_proc),
                lpszClassName: class,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                Default::default(),
                class,
                windows::core::w!(""),
                Default::default(),
                0,
                0,
                0,
                0,
                HWND(-3isize as *mut _), // HWND_MESSAGE
                None,
                None,
                None,
            );
            if let Ok(hwnd) = hwnd {
                let _ = SetTimer(hwnd, DETECT_TIMER_ID, DETECT_TIMER_MS, Some(detect_tick_proc));
            }

            // First answer immediately, so the UI does not wait for the timer.
            detect_and_publish();

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = UnhookWinEvent(hook);
        }
    });
}

#[cfg(not(windows))]
pub fn spawn_watcher(_ctx: eframe::egui::Context) {}

// ---------------------------------------------------------------------------
// Win32
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn foreground_pid() -> Option<u32> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId,
    };

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }

        // UWP apps show an ApplicationFrameWindow that belongs to the shell;
        // the real process owns a child window. Take the first child pid that
        // is not the frame's — searching for one specific CoreWindow class
        // missed apps that do not use it.
        let mut class_buf = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut class_buf);
        if len > 0 {
            let class_name = String::from_utf16_lossy(&class_buf[..len as usize]);
            if class_name == "ApplicationFrameWindow" {
                let mut after = HWND(std::ptr::null_mut());
                while let Ok(child) = FindWindowExW(hwnd, after, None, None) {
                    if child.0.is_null() {
                        break;
                    }
                    let mut child_pid = 0u32;
                    GetWindowThreadProcessId(child, Some(&mut child_pid));
                    if child_pid != 0 && child_pid != pid {
                        return Some(child_pid);
                    }
                    after = child;
                }
            }
        }

        Some(pid)
    }
}

#[cfg(not(windows))]
fn foreground_pid() -> Option<u32> {
    None
}

fn process_name(pid: u32) -> Option<String> {
    if let Some(full) = winproc::image_path(pid) {
        if let Some(name) = full.rsplit(['\\', '/']).next() {
            return Some(name.to_string());
        }
    }
    let procs = winproc::snapshot();
    procs.into_iter().find(|p| p.pid == pid).map(|p| p.name)
}

/// Every (pid, family) CLI match in the process tree under `root`, in
/// snapshot order. With `hop_parent`, the walk starts at the root's parent
/// instead: a console host's CLI is its sibling, not its child.
fn tree_matches(root: u32, hop_parent: bool) -> Vec<(u32, Family)> {
    let procs = winproc::snapshot();
    let root = if hop_parent {
        procs
            .iter()
            .find(|p| p.pid == root)
            .map(|p| p.parent)
            .filter(|p| *p != 0)
            .unwrap_or(root)
    } else {
        root
    };

    // Breadth-first over the descendants of `root`.
    let mut frontier = vec![root];
    let mut seen = vec![root];
    let mut out = Vec::new();
    while let Some(parent) = frontier.pop() {
        for p in &procs {
            if p.parent != parent || seen.contains(&p.pid) {
                continue;
            }
            seen.push(p.pid);
            frontier.push(p.pid);
            if let Some(f) = direct(&p.name) {
                out.push((p.pid, f));
            }
        }
        // A runaway tree would only cost us time; the process list is finite.
        if seen.len() > 4096 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_is_pure_and_complete() {
        assert_eq!(classify_exe("claude.exe"), ExeKind::Direct(Family::Claude));
        assert_eq!(classify_exe("Claude.EXE"), ExeKind::Direct(Family::Claude));
        assert_eq!(classify_exe("codex"), ExeKind::Direct(Family::Codex));
        assert_eq!(classify_exe("chatgpt.exe"), ExeKind::Direct(Family::Codex));
        assert_eq!(classify_exe("antigravity.exe"), ExeKind::Direct(Family::Antigravity));
        assert_eq!(classify_exe("opencode.exe"), ExeKind::Direct(Family::Antigravity));
        assert_eq!(classify_exe("code.exe"), ExeKind::Editor);
        assert_eq!(classify_exe("cursor.exe"), ExeKind::Editor);
        assert_eq!(classify_exe("windowsterminal.exe"), ExeKind::Terminal);
        assert_eq!(classify_exe("pwsh.exe"), ExeKind::Terminal);
        assert_eq!(classify_exe("conhost.exe"), ExeKind::Terminal);
        assert_eq!(classify_exe("chrome.exe"), ExeKind::Other);
        assert_eq!(classify_exe("explorer.exe"), ExeKind::Other);
    }

    #[test]
    fn the_shown_family_survives_while_it_is_alive_in_the_tree() {
        let matches = vec![(100, Family::Claude), (500, Family::Codex)];
        // A newer codex pid does not steal the widget from a live claude.
        assert_eq!(pick_family(&matches, Some(Family::Claude)), Some(Family::Claude));
        // Once claude is gone, the newest match takes over.
        assert_eq!(pick_family(&matches, Some(Family::Antigravity)), Some(Family::Codex));
        assert_eq!(pick_family(&matches, None), Some(Family::Codex));
        assert_eq!(pick_family(&[], Some(Family::Claude)), None);
    }

    #[test]
    fn a_new_family_needs_two_consecutive_sightings() {
        let shown = Some(Family::Claude);
        let codex = Some(Family::Codex);

        // First tick: the tree says codex, we show claude — hold, remember.
        let (family, pending) = debounce(codex, shown, None);
        assert_eq!(family, shown, "one sighting does not flip the widget");
        assert_eq!(pending, codex);

        // Second tick with the same answer: committed.
        let (family, pending) = debounce(codex, shown, pending);
        assert_eq!(family, codex, "two sightings commit");
        assert_eq!(pending, None);

        // A flicker (different answer next tick) resets the pending sighting.
        let (_, pending) = debounce(codex, shown, None);
        let (family, pending) = debounce(Some(Family::Antigravity), shown, pending);
        assert_eq!(family, shown, "an unstable tree keeps the shown family");
        assert_eq!(pending, Some(Family::Antigravity));

        // Back to the shown family commits immediately and clears pending.
        let (family, pending) = debounce(shown, shown, Some(Family::Codex));
        assert_eq!(family, shown);
        assert_eq!(pending, None);

        // An empty tree does not latch a pending family.
        let (family, pending) = debounce(None, shown, Some(Family::Codex));
        assert_eq!(family, None);
        assert_eq!(pending, None);
    }

    type NameFn = Box<dyn Fn(u32) -> Option<String>>;
    type TreeFn = Box<dyn Fn(u32, bool) -> Vec<(u32, Family)>>;

    /// Build facts with a fixed foreground exe and watched-open flag.
    fn facts(exe: Option<&'static str>, watched_open: bool) -> Facts<NameFn, TreeFn> {
        Facts {
            watched_open,
            foreground_pid: exe.map(|_| 4242),
            own_pid: 1,
            exe_name: Box::new(move |_| exe.map(|e| e.to_string())),
            tree: Box::new(|_, _| Vec::new()),
        }
    }

    #[test]
    fn a_watched_foreground_reports_both_visibility_and_z_band() {
        let mut d = Detector::default();
        let s = d.tick_with(facts(Some("antigravity.exe"), true), 0.0);
        assert!(s.is_ai, "a watched app is open");
        assert!(s.foreground_watched, "and it is the one in front");
        assert_eq!(s.family, Some(Family::Antigravity));
    }

    #[test]
    fn a_non_watched_foreground_keeps_the_widget_up_but_drops_the_band() {
        let mut d = Detector::default();
        // Antigravity is open somewhere, but a browser is in front.
        let s = d.tick_with(facts(Some("chrome.exe"), true), 0.0);
        assert!(s.is_ai, "still visible: a watched app is open");
        assert!(!s.foreground_watched, "but not above the browser");
    }

    #[test]
    fn an_unreadable_foreground_holds_then_drops_the_band() {
        let mut d = Detector::default();
        // A watched app in front…
        d.tick_with(facts(Some("claude.exe"), true), 0.0);
        // …then the foreground goes unreadable: the band is *held* briefly, so
        // the widget does not sink under the app it still sits on top of while
        // a window switch races the hook.
        let s = d.tick_with(facts(None, true), 0.5);
        assert!(s.foreground_watched, "held within the unknown grace window");
        assert!(s.is_ai, "visibility stands on its own");
        // After the grace window it drops, rather than float over an unknown
        // window forever.
        let s = d.tick_with(facts(None, true), 10.0);
        assert!(!s.foreground_watched, "an unknown foreground eventually is not a watched one");
    }

    #[test]
    fn the_widget_its_own_foreground_does_not_change_the_answer() {
        let mut d = Detector::default();
        d.tick_with(facts(Some("codex.exe"), true), 0.0);
        // Dragging the widget brings our own process to the front.
        let own = Facts {
            watched_open: true,
            foreground_pid: Some(1), // == own_pid
            own_pid: 1,
            exe_name: |_| Some("chrome.exe".to_string()),
            tree: |_, _| Vec::new(),
        };
        let s = d.tick_with(own, 0.5);
        assert_eq!(s.family, Some(Family::Codex), "the family is left alone");
    }
}
