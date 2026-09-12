//! The Tokpaek window (a compact, movable, translucent circle gauge) plus tray wiring.

use crate::active;
use crate::config::Settings;
use crate::i18n::Language;
use crate::providers::{self, Family, Snapshot};
use crate::shortcuts;
use crate::tray::Tray;
use crate::update::{self, UpdateState};
use crate::windowing;

use eframe::egui;
use egui::{PointerButton, Pos2, Sense, Vec2, ViewportCommand};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Latest fetch state of one family. `last` keeps the most recent *successful*
/// snapshot so we can keep drawing structure (titles, reset times) while offline.
#[derive(Default)]
pub struct FetchState {
    pub last: Option<Snapshot>,
    pub online: bool,
    pub ever: bool,
    pub error: Option<String>,
    /// The last poll failed only because the service is throttling us. The
    /// numbers we already have are still true, so they stay on screen.
     pub rate_limited: bool,
}



pub struct Shared {
    /// One state per family, indexed by `Family::idx`.
    pub states: Mutex<Vec<FetchState>>,
    pub refresh: AtomicBool,
    pub interval: AtomicU64,
    /// Bitmask of families the poller should keep fresh.
    pub enabled: AtomicU8,
    /// Family to poll right now (set when the user switches tools).
    pub want: AtomicU8,
    /// Result of the last GitHub release check.
    pub update: Mutex<UpdateState>,
    /// Set to run an update check without waiting for the 8-hour timer.
    pub update_now: AtomicBool,
}

pub struct App {
    pub(crate) settings: Settings,
    pub(crate) shared: Arc<Shared>,
    pub(crate) tray: Option<Tray>,
    pub(crate) show_settings: bool,
    pub(crate) settings_tab: crate::settings_ui::SettingsTab,
    /// Set on open: the settings window still has to be moved to the monitor
    /// the user called it from.
    pub(crate) settings_center: bool,
    /// Work area of that monitor, captured when the window was opened.
    pub(crate) settings_area: Option<(i32, i32, i32, i32)>,
    /// The settings window itself, resolved once it exists (see
    /// `own_settings_window`); it is ours to move and to re-chrome.
    pub(crate) settings_hwnd: Option<isize>,
    pub(crate) autostart: bool,
    /// Family currently on screen.
    pub(crate) active: Family,
    is_ai_active: bool,
    /// The foreground window belongs to a watched app (published by the
    /// watcher thread; the frame never probes Win32 itself).
    foreground_watched: bool,
    /// Size we last asked the OS for, so we only resize when it changes.
    applied_size: Vec2,
    /// Nothing to watch: the strip is still there, but paints nothing and lets
    /// clicks through (see `set_click_through`).
    idle_hidden: bool,
    /// The user asked for the widget to be minimized (tray / context menu).
    /// Distinct from Smart Focus hiding: `idle_hidden` is recomputed every
    /// frame from `should_hide`, so a hand-toggle written straight into it was
    /// undone on the next frame — this is why "hide" never worked.
    manual_hidden: bool,
    pub(crate) manual_unhide_until: f64,
    /// Throttle for persisting the auto-switched family.
    last_family_save: f64,
    /// Tray menu events, delivered via a handler that also wakes the UI so
    /// changes are applied immediately, not on next hover.
    menu_rx: std::sync::mpsc::Receiver<tray_icon::menu::MenuEvent>,
    tray_rx: std::sync::mpsc::Receiver<tray_icon::TrayIconEvent>,
    /// Last time (s) we re-asserted always-on-top so the taskbar can't cover us.
    last_topmost: f64,
    /// Native window handle, resolved lazily from `eframe::Frame`.
    hwnd: Option<isize>,
    /// Period the Win32 backstop timer is currently armed at.
    timer_period: u32,
    /// Tray hover text we last set, so we only touch the icon on a change.
    tooltip: String,
    /// A drag we started ourselves is in progress; only then is the window's
    /// new position worth persisting.
    dragging: bool,
}

/// Pull the Win32 HWND out of eframe's frame (None on other platforms).
fn native_hwnd(frame: &eframe::Frame) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

pub(crate) static NATIVE_HWND: AtomicIsize = AtomicIsize::new(0);
pub(crate) static SUBCLASSED: AtomicBool = AtomicBool::new(false);
pub(crate) static ACTIVATE_MSG: AtomicU32 = AtomicU32::new(0);
pub(crate) static INSTANCE_ACTIVATED: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
unsafe extern "system" fn instance_subclass_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
    _uid_subclass: usize,
    _dw_ref_data: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Shell::DefSubclassProc;

    let activate_msg = ACTIVATE_MSG.load(Ordering::Relaxed);
    if activate_msg != 0 && msg == activate_msg {
        INSTANCE_ACTIVATED.store(true, Ordering::Relaxed);
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, false);
    }

    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// The widget's right-click menu — the second adapter over the shared
/// `MenuAction` vocabulary (the first is tray.rs). Returns the picked action.
#[cfg(windows)]
fn show_context_menu(hwnd: isize, state: &ContextMenuState) -> Option<crate::menu::MenuAction> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, SetForegroundWindow,
        TrackPopupMenu, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, TPM_LEFTALIGN,
        TPM_RETURNCMD, TPM_TOPALIGN,
    };

    // Command ids: 1 refresh, 9 home, 8 always-on-top, 4 settings, 7 quit.
    let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
    unsafe {
        let hmenu = CreatePopupMenu().ok()?;

        let _ = AppendMenuW(
            hmenu,
            MF_STRING,
            1,
            &HSTRING::from(state.lang.text("Обновить", "Refresh")),
        );
        let _ = AppendMenuW(
            hmenu,
            MF_STRING,
            9,
            &HSTRING::from(state.lang.text("Домой", "Home")),
        );
        let _ = AppendMenuW(
            hmenu,
            MF_STRING | check(state.always_on_top),
            8,
            &HSTRING::from(state.lang.text(
                "Показывать поверх всех окон",
                "Show above all windows",
            )),
        );
        let _ = AppendMenuW(
            hmenu,
            MF_STRING,
            4,
            &HSTRING::from(state.lang.text("Настройки", "Settings")),
        );
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(
            hmenu,
            MF_STRING,
            7,
            &HSTRING::from(state.lang.text("Закрыть", "Quit")),
        );

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(HWND(hwnd as *mut _));
        let cmd = TrackPopupMenu(
            hmenu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
            pt.x,
            pt.y,
            0,
            HWND(hwnd as *mut _),
            None,
        );
        let _ = DestroyMenu(hmenu);
        use crate::menu::MenuAction;
        match cmd.0 as usize {
            1 => Some(MenuAction::Refresh),
            9 => Some(MenuAction::Home),
            // A check item click flips it; carry the new state.
            8 => Some(MenuAction::AlwaysOnTop(!state.always_on_top)),
            4 => Some(MenuAction::OpenSettings),
            7 => Some(MenuAction::Quit),
            _ => None,
        }
    }
}

/// The facts the widget context menu is drawn from.
struct ContextMenuState {
    always_on_top: bool,
    lang: Language,
}

#[cfg(not(windows))]
fn show_context_menu(_hwnd: isize, _state: &ContextMenuState) -> Option<crate::menu::MenuAction> {
    None
}

// ---------------------------------------------------------------------------
// Repaint backstop
//
// While a modal Win32 loop is running — the tray's own right-click menu is one —
// winit's event loop is not, so the wake-up it scheduled for our next animation
// frame never fires and the strip freezes until some input reaches it again.
// A window timer keeps working inside those loops: WM_TIMER is dispatched by the
// modal pump and, with a TIMERPROC, needs no window procedure of our own. It
// invalidates the window (→ WM_PAINT → winit's RedrawRequested → a repaint) but
// only when egui has *not* painted within the last period, so in normal
// operation this costs nothing.
// ---------------------------------------------------------------------------

const REPAINT_TIMER_ID: usize = 0x9101;
static START: OnceLock<std::time::Instant> = OnceLock::new();
static LAST_PAINT_MS: AtomicU64 = AtomicU64::new(0);
static TIMER_GRACE_MS: AtomicU64 = AtomicU64::new(500);

fn uptime_ms() -> u64 {
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}

#[cfg(windows)]
unsafe extern "system" fn repaint_tick(
    hwnd: windows::Win32::Foundation::HWND,
    _msg: u32,
    _id: usize,
    _time: u32,
) {
    let since = uptime_ms().saturating_sub(LAST_PAINT_MS.load(Ordering::Relaxed));
    if since >= TIMER_GRACE_MS.load(Ordering::Relaxed) {
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, false);
    }
}

#[cfg(windows)]
fn arm_repaint_timer(hwnd: isize, period_ms: u32) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::SetTimer;
    TIMER_GRACE_MS.store(period_ms as u64, Ordering::Relaxed);
    unsafe {
        SetTimer(
            HWND(hwnd as *mut _),
            REPAINT_TIMER_ID,
            period_ms,
            Some(repaint_tick),
        );
    }
}

#[cfg(not(windows))]
fn arm_repaint_timer(_hwnd: isize, _period_ms: u32) {}

// ---------------------------------------------------------------------------

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, settings: Settings) -> Self {
        crate::settings_ui::apply_style(&cc.egui_ctx);
        crate::diaglog::set_diagnostics(settings.diagnostics);
        crate::diaglog::diag(&format!(
            "--- tokpaek {} started, diagnostics on ---",
            update::current()
        ));

        let shared = Arc::new(Shared {
            states: Mutex::new(
                (0..Family::ALL.len())
                    .map(|_| FetchState::default())
                    .collect(),
            ),
            refresh: AtomicBool::new(false),
            interval: AtomicU64::new(settings.poll_secs),
            enabled: AtomicU8::new(settings.enabled_mask()),
            want: AtomicU8::new(crate::scheduler::NO_FAMILY),
            update: Mutex::new(UpdateState::default()),
            update_now: AtomicBool::new(false),
        });
        spawn_poller(shared.clone(), cc.egui_ctx.clone());
        spawn_update_checker(shared.clone(), cc.egui_ctx.clone());

        active::spawn_watcher(cc.egui_ctx.clone());

        let autostart = shortcuts::is_autostart_enabled();
        let tray = Tray::new(settings.always_on_top, settings.language).ok();

        // Route tray menu events through our own channel and wake the UI on each
        // one, so a menu choice is applied immediately instead of on the next
        // timer tick / mouse hover.
        //
        // Quit is the one action handled right here, on the tray thread:
        // routing it through a frame meant waiting for the event loop to wake
        // up (and sometimes a second click) — the slow-exit bug. Settings are
        // flushed on their 500 ms throttle, and flushed once more here, so
        // exiting immediately loses nothing.
        let (menu_tx, menu_rx) = std::sync::mpsc::channel();
        let wake = cc.egui_ctx.clone();
        tray_icon::menu::MenuEvent::set_event_handler(Some(move |ev: tray_icon::menu::MenuEvent| {
            if ev.id == tray_icon::menu::MenuId(crate::tray::QUIT_MENU_ID.to_string()) {
                Settings::flush_now();
                crate::diaglog::flush_log();
                std::process::exit(0);
            }
            let _ = menu_tx.send(ev);
            wake.request_repaint();
        }));

        let (tray_tx, tray_rx) = std::sync::mpsc::channel();
        let wake_tray = cc.egui_ctx.clone();
        tray_icon::TrayIconEvent::set_event_handler(Some(move |ev| {
            let _ = tray_tx.send(ev);
            wake_tray.request_repaint();
        }));

        Self {
            active: settings.family,
            settings,
            shared,
            tray,
            show_settings: false,
            settings_tab: crate::settings_ui::SettingsTab::Appearance,
            settings_center: false,
            settings_area: None,
            settings_hwnd: None,
            autostart,
            is_ai_active: true,
            foreground_watched: false,
            applied_size: Vec2::ZERO,
            idle_hidden: false,
            manual_hidden: false,
            manual_unhide_until: 0.0,
            last_family_save: f64::MIN,
            menu_rx,
            tray_rx,
            last_topmost: 0.0,
            hwnd: None,
            timer_period: 0,
            tooltip: String::new(),
            dragging: false,
        }
    }

    /// `from_strip`: opened by right-clicking the strip, so the strip's own
    /// monitor is the one the user is looking at. From the tray menu we only
    /// have the pointer to go by.
    pub(crate) fn open_settings(&mut self, from_strip: bool) {
        self.show_settings = true;
        self.settings_center = true;
        // Only clear the *manual* flag: `idle_hidden` belongs to the hide
        // transition in update(), which is what flips MousePassthrough back
        // off. Writing idle_hidden here directly left the window painted but
        // still click-through — visible yet impossible to interact with.
        self.manual_hidden = false;
        self.manual_unhide_until = f64::MAX;
        self.settings_area = from_strip
            .then(|| self.hwnd.and_then(crate::windowing::window_work_area))
            .flatten()
            .or_else(crate::windowing::cursor_work_area);
        #[cfg(windows)]
        if let Some(h) = self.hwnd {
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::InvalidateRect(
                    windows::Win32::Foundation::HWND(h as *mut _),
                    None,
                    false,
                );
            }
            // Z-order itself is the verdict executor's job — facts only here.
        }
        #[cfg(windows)]
        if let Some(h) = self.settings_hwnd {
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{
                    BringWindowToTop, SetForegroundWindow,
                };
                let hwnd = windows::Win32::Foundation::HWND(h as *mut _);
                let _ = BringWindowToTop(hwnd);
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }

    /// The one place every menu action's effect lives — the tray menu and the
    /// widget's context menu are adapters that translate clicks into
    /// `MenuAction`, and nothing else.
    fn apply_menu_action(&mut self, action: crate::menu::MenuAction, ctx: &egui::Context) {
        use crate::menu::MenuAction;
        match action {
            MenuAction::Quit => {
                // Persist everything, then exit directly: winit's window
                // teardown plus the tray icon's Shell_NotifyIcon delete could
                // hang for seconds, and nothing else needs a graceful
                // shutdown once state is on disk.
                Settings::flush_now();
                crate::diaglog::flush_log();
                std::process::exit(0);
            }
            MenuAction::Refresh => {
                self.shared.refresh.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            }
            MenuAction::Home => {
                // Back to the default spot: the screen's top-left corner.
                self.settings.pos = Some((0.0, 0.0));
                self.settings.save();
                #[cfg(windows)]
                if let Some(h) = self.hwnd {
                    windowing::home(h);
                }
                ctx.request_repaint();
            }
            MenuAction::OpenSettings => {
                self.open_settings(false);
                ctx.request_repaint();
            }
            // The check state arrives from the menu item itself; settings
            // follow it, and the verdict executor applies the z-band.
            MenuAction::AlwaysOnTop(on) => {
                self.settings.always_on_top = on;
                self.settings.save();
                // Expire the z-order cadence so the new band lands next frame.
                self.last_topmost = f64::MIN;
                ctx.request_repaint();
            }
        }
    }

    fn handle_tray_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.menu_rx.try_recv() {
            let Some(tray) = &self.tray else { continue };
            let Some(action) = tray.action(&ev.id) else {
                continue;
            };
            self.apply_menu_action(action, ctx);
        }

        while let Ok(ev) = self.tray_rx.try_recv() {
            match ev {
                tray_icon::TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                }
                | tray_icon::TrayIconEvent::DoubleClick {
                    button: tray_icon::MouseButton::Left,
                    ..
                } => {
                    if self.show_settings {
                        self.show_settings = false;
                        self.settings.save();
                    } else {
                        self.open_settings(false);
                    }
                    ctx.request_repaint();
                }
                _ => {}
            }
        }
    }

    /// Follow the foreground window (or the active family) and, on a switch,
    /// ask the poller to refresh that family right away. Detection itself runs
    /// on the watcher thread (active.rs); this only reads its published answer,
    /// so the frame never walks the process list.
    fn update_active(&mut self, t: f64) {
        let active_status = active::published();
        self.is_ai_active = active_status.is_ai;
        self.foreground_watched = active_status.foreground_watched;

        let target = match active_status.family {
            Some(f) if self.settings.enabled(f) => f,
            _ => self.active,
        };
        if target == self.active {
            return;
        }
        self.active = target;
        self.settings.family = target;
        self.shared
            .want
            .store(target.idx() as u8, Ordering::Relaxed);
        // Persisted so the next launch starts on the tool you were using — but
        // not on every alt-tab.
        if t - self.last_family_save > 30.0 {
            self.last_family_save = t;
            self.settings.save();
        }
    }



    /// Announce a pending update on the tray icon, where it can be seen without
    /// opening anything.
    fn sync_tooltip(&mut self) {
        let lang = self.settings.language;
        let name = lang.text("Токпаёк", "Tokpaek");
        let want = match &self.shared.update.lock().unwrap().available {
            Some(u) => match lang {
                crate::i18n::Language::Russian => {
                    format!("{name} {} — доступно обновление {}", update::current(), u.version)
                }
                crate::i18n::Language::English => {
                    format!("{name} {} — update {} available", update::current(), u.version)
                }
            },
            None => format!("{name} {}", update::current()),
        };
        if want != self.tooltip {
            if let Some(t) = &self.tray {
                t.set_tooltip(&want);
            }
            self.tooltip = want;
        }
    }

    /// (online, stale, snapshot) of the family on screen.
    fn active_state(&self) -> crate::gauge::ActiveState {
        let st = self.shared.states.lock().unwrap();
        let s = &st[self.active.idx()];
        // Guard against a stale slot: only draw a snapshot that says it belongs
        // to the family we're showing.
        let last = s.last.clone().filter(|snap| snap.family == self.active);
        crate::gauge::ActiveState {
            // Throttled with data in hand: keep showing it rather than dashes.
            stale: !s.online && s.rate_limited && last.is_some(),
            online: s.online,
            last,
        }
    }
}

/// Background poller: keeps every enabled family fresh, each on its own
/// schedule, so one dead source (an IDE that isn't running) can't drag the
/// others into a fast retry loop. The scheduling policy itself lives in
/// `scheduler::Scheduler` (pure, tested); this loop is just its executor:
/// read the control words, ask what is due, fetch through the adapter, report.
fn spawn_poller(shared: Arc<Shared>, ctx: egui::Context) {
    spawn_poller_with(shared, ctx, providers::fetch);
}

fn spawn_poller_with(shared: Arc<Shared>, ctx: egui::Context, fetch: crate::scheduler::FetchFn) {
    std::thread::spawn(move || {
        use crate::scheduler::{RetryPolicy, Scheduler};
        let start = std::time::Instant::now();
        let now_ms = || start.elapsed().as_millis() as u64;
        let mut sched = Scheduler::new(now_ms());
        let mut retry = RetryPolicy::new();
        loop {
            let force = shared.refresh.swap(false, Ordering::Relaxed);
            let want = shared.want.swap(crate::scheduler::NO_FAMILY, Ordering::Relaxed);
            let mask = shared.enabled.load(Ordering::Relaxed);
            let interval = shared.interval.load(Ordering::Relaxed).max(5);
            let mut changed = false;

            for family in sched.due_families(now_ms(), force, want, mask) {
                // A server-driven cooldown outranks our own schedule: ask
                // nothing until it passes; the last numbers stay on screen.
                if let Some(until) = retry.cooldown_end_ms(family, now_ms()) {
                    sched.defer_until(family, until);
                    continue;
                }
                let result = fetch(family);
                let ok = result.is_ok();
                retry.on_result(family, &result, now_ms());
                {
                    let mut st = shared.states.lock().unwrap();
                    let s = &mut st[family.idx()];
                    match result {
                        Ok(snap) => {
                            s.last = Some(snap);
                            s.online = true;
                            s.ever = true;
                            s.error = None;
                            s.rate_limited = false;
                        }
                        Err(e) => {
                            s.online = false;
                            s.rate_limited = e.rate_limited;
                            s.error = Some(match e.rate_limited {
                                // Say how long the pause is, now that one
                                // module owns the answer.
                                true => {
                                    let mins = retry
                                        .cooldown_left_secs(family, now_ms())
                                        .map(|secs| (secs / 60) + 1)
                                        .unwrap_or(1);
                                    format!("лимит запросов, пауза {mins} мин")
                                }
                                false => e.msg,
                            });
                        }
                    }
                }
                changed = true;
                sched.report(family, ok, now_ms(), interval);
                // The cooldown outranks the failure ladder: defer *after*
                // report so the longer of the two waits wins.
                if let Some(until) = retry.cooldown_end_ms(family, now_ms()) {
                    sched.defer_until(family, until);
                }
            }

            if changed {
                ctx.request_repaint();
            }
            crate::diaglog::flush_log();
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    });
}

/// Checks GitHub for a newer release on demand (when shared.update_now is set).
fn spawn_update_checker(shared: Arc<Shared>, ctx: egui::Context) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if !shared.update_now.swap(false, Ordering::Relaxed) {
            continue;
        }
        let result = update::check();
        {
            let mut st = shared.update.lock().unwrap();
            st.checked = true;
            match result {
                Ok(found) => {
                    st.available = found;
                    st.error = None;
                }
                Err(e) => st.error = Some(e),
            }
        }
        ctx.request_repaint();
    });
}


impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        LAST_PAINT_MS.store(uptime_ms(), Ordering::Relaxed);
        // Land any settings write whose throttle has elapsed (see config.rs:
        // save() only buffers while writes are throttled).
        Settings::flush_due();
        if self.hwnd.is_none() {
            self.hwnd = native_hwnd(frame);
            if let Some(h) = self.hwnd {
                NATIVE_HWND.store(h, Ordering::Relaxed);
                #[cfg(windows)]
                if !SUBCLASSED.swap(true, Ordering::Relaxed) {
                    use windows::core::w;
                    use windows::Win32::Foundation::HWND;
                    use windows::Win32::UI::Shell::SetWindowSubclass;
                    use windows::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW;
                    let msg = unsafe { RegisterWindowMessageW(w!("Tokpaek_ActivateInstance")) };
                    ACTIVATE_MSG.store(msg, Ordering::Relaxed);
                    unsafe {
                        let _ = SetWindowSubclass(
                            HWND(h as *mut _),
                            Some(instance_subclass_proc),
                            1001,
                            0,
                        );
                    }
                }
            }
        }

        #[cfg(windows)]
        if INSTANCE_ACTIVATED.swap(false, Ordering::Relaxed) {
            // Facts only — the verdict executor below performs the transition
            // (passthrough, repaint, z-band).
            self.manual_hidden = false;
            self.manual_unhide_until = f64::MAX;
            self.open_settings(false);
            ctx.request_repaint();
        }

        self.handle_tray_events(ctx);

        let anim_t = ctx.input(|i| i.time);
        self.update_active(anim_t);

        // One verdict for the whole window (see frame_policy::decide):
        // visible? click-through? which z-band? how often to repaint? This
        // loop is only its executor — every toggle site (tray, menus,
        // settings, activation) changes facts, never the window directly.
        let verdict = crate::frame_policy::decide(&crate::frame_policy::WindowFacts {
            manual_hidden: self.manual_hidden,
            smart_focus: self.settings.smart_focus,
            always_on_top: self.settings.always_on_top,
            watched_app_open: self.is_ai_active,
            foreground_watched: self.foreground_watched,
            settings_open: self.show_settings,
            interacting: self.dragging,
            time: anim_t,
            launch_grace_secs: 15.0,
            manual_unhide_until: self.manual_unhide_until,
        });

        if verdict.visible != !self.idle_hidden {
            self.idle_hidden = !verdict.visible;
            ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(verdict.passthrough));
            ctx.request_repaint();
            if !verdict.visible {
                // Hiding must repaint *now*: with click-through on but the
                // last painted frame still on screen, the widget looked
                // present yet inert until some unrelated wake-up cleared it.
                #[cfg(windows)]
                if let Some(h) = self.hwnd {
                    unsafe {
                        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(
                            windows::Win32::Foundation::HWND(h as *mut _),
                            None,
                            false,
                        );
                    }
                }
            }
        }

        // The widget resizes by dragging its edges like a normal window:
        // adopt what the user dragged into the settings (pure decision in
        // frame_policy::adopt_resize), then re-assert the square below.
        let inner_rect = ctx.input(|i| i.viewport().inner_rect);
        let viewport_w = inner_rect.map(|r| r.width());
        if let Some(size) = crate::frame_policy::adopt_resize(viewport_w, self.applied_size.x) {
            self.settings.circle_size = size;
            self.settings.save();
        }

        let want = Vec2::splat(self.settings.circle_size as f32);
        if (want - self.applied_size).length() > 0.5 {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(want));
            self.applied_size = want;
        }
        // The OS resize loop drags width and height independently, and
        // winit's InnerSize can round differently — pin the square in
        // physical pixels once per frame, whichever path changed the size.
        // Z-order is *not* this call's business: the verdict executor below
        // owns the band.
        #[cfg(windows)]
        if let Some(h) = self.hwnd {
            windowing::place_square(h, self.settings.circle_size as f32 * ctx.pixels_per_point());
        }

        if let Some(tray) = &self.tray {
            // The one place the tray checkmarks learn the settings — every
            // toggle site (settings window, widget click, context menu, tray
            // menu) only changes `settings`, this frame applies them.
            tray.set_topmost_checked(self.settings.always_on_top);
        }

        // Z-order comes from the verdict — one decision, one executor.
        // NOTE: egui/winit `ViewportCommand::WindowLevel` does NOT work here —
        // winit diffs window flags and returns early when unchanged, so no
        // SetWindowPos is issued. Win32 directly.
        // Re-asserted every frame while Smart Focus is on (the WinEvent hook
        // wakes us the moment the foreground changes), otherwise on a 0.7 s
        // cadence because the taskbar shares the topmost band.
        let z_due = self.settings.smart_focus || anim_t - self.last_topmost >= 0.7;
        if !self.idle_hidden && z_due {
            if let Some(h) = self.hwnd {
                match verdict.band {
                    crate::frame_policy::ZBand::Topmost => windowing::set_topmost(h),
                    crate::frame_policy::ZBand::Normal => windowing::drop_topmost(h),
                }
            }
            self.last_topmost = anim_t;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                // Hidden: paint nothing (the window is transparent, so it leaves
                // no trace) and take no input either.
                if self.idle_hidden {
                    return;
                }
                let full = ui.max_rect();
                // Mouse-resize like a normal window: the outer band of the
                // square reacts to the pointer (resize cursors) and a drag
                // started there hands the window to the OS resize loop. The
                // rest of the square stays the drag/click area.
                //
                // The zone for a *started* drag comes from where the button
                // went down (`press_origin`, fixed at the press), not from any
                // live pointer position: `drag_started_by` fires a few pixels
                // into the gesture, and by then both hover_pos and
                // interact_pointer_pos have often left the edge band — which
                // is what made edge drags move the window instead.
                let edge_at = |pos: Pos2| {
                    crate::frame_policy::resize_edge(
                        (full.width(), full.height()),
                        (pos.x - full.min.x, pos.y - full.min.y),
                    )
                };
                if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                    if let Some(zone) = edge_at(pos) {
                        ui.ctx().set_cursor_icon(zone.cursor());
                    }
                }
                let resp = ui.interact(full, ui.id().with("strip-drag"), Sense::click_and_drag());
                if resp.drag_started_by(PointerButton::Primary) {
                    let press_zone = ctx
                        .input(|i| i.pointer.press_origin())
                        .and_then(edge_at);
                    match press_zone {
                        Some(zone) => {
                            ctx.send_viewport_cmd(ViewportCommand::BeginResize(zone.direction()));
                        }
                        None => {
                            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                            self.dragging = true;
                        }
                    }
                }
                if resp.clicked_by(PointerButton::Primary) {
                    // The seven-day timer only switches while the seven-day
                    // arc exists (the same rule that grays the menu item out).
                    if !self.settings.circle_show_claude_gpt {
                        self.settings.circle_show_weekly_reset =
                            !self.settings.circle_show_weekly_reset;
                        self.settings.save();
                        ctx.request_repaint();
                    }
                }
                if resp.clicked_by(PointerButton::Secondary) {
                    #[cfg(windows)]
                    {
                        if let Some(h) = self.hwnd {
                            let state = ContextMenuState {
                                always_on_top: self.settings.always_on_top,
                                lang: self.settings.language,
                            };
                            if let Some(action) = show_context_menu(h, &state) {
                                self.apply_menu_action(action, ctx);
                            }
                        } else {
                            self.open_settings(true);
                        }
                    }
                    #[cfg(not(windows))]
                    {
                        self.open_settings(true);
                    }
                }
                crate::gauge::draw(ui, &self.settings, self.active_state());
            });

        // Persist the window position when the user finishes moving it — and
        // only then. Saving on any release once let the placement the shell
        // imposes on a shortcut launch overwrite the user's own position.
        //
        // Everything here is *physical* pixels via Win32: egui's logical
        // coordinates are relative to one screen and mixing them with the
        // work area of another (different DPI) sent the window off-screen
        // when it was dropped on a second monitor.
        if self.dragging && ctx.input(|i| i.pointer.any_released()) {
            self.dragging = false;
            #[cfg(windows)]
            if let Some(h) = self.hwnd {
                if let Some(pos) = windowing::snap(h) {
                    // Stored in logical points: what the startup viewport
                    // builder consumes.
                    let ppp = ctx.pixels_per_point();
                    let p = (pos.0 / ppp, pos.1 / ppp);
                    if self.settings.pos != Some(p) {
                        self.settings.pos = Some(p);
                        self.settings.save();
                    }
                }
            }
            #[cfg(not(windows))]
            let _ = &ctx;
        }

        self.sync_tooltip();
        self.render_settings(ctx);

        // Energy efficiency / 0% CPU: the period comes from the same verdict;
        // input, window changes and network updates wake egui on-demand.
        let period = verdict.repaint_ms;
        if let Some(h) = self.hwnd {
            if self.timer_period != period {
                arm_repaint_timer(h, period);
                self.timer_period = period;
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(period as u64));
    }
}

