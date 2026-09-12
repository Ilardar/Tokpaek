//! The per-frame decisions of the gauge window, as pure functions: whether to
//! hide, which z-band to sit in, where a dropped window lands, how often to
//! repaint. `App::update` supplies the facts and executes the verdict — Win32
//! calls and painting stay there, the interesting choices are testable here.
//!
//! `decide` is the single answer to "visible? which band?": every visibility
//! and z-order bug lived in scattered sites that each asserted part of it.

/// Which z-band the window belongs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZBand {
    /// Above every window, including other topmost ones.
    Topmost,
    /// An ordinary window: anything may cover it.
    Normal,
}

/// Everything one frame's window decision needs; no field reaches into `App`.
pub struct WindowFacts {
    /// The user asked for the widget to be minimized.
    pub manual_hidden: bool,
    pub smart_focus: bool,
    pub always_on_top: bool,
    /// At least one watched app (Antigravity, Claude, ChatGPT) has a visible,
    /// non-minimized window.
    pub watched_app_open: bool,
    /// The foreground window belongs to a watched app — the one the user is
    /// looking at right now, which the widget must not sink under.
    pub foreground_watched: bool,
    pub settings_open: bool,
    /// A drag of the widget itself is in progress.
    pub interacting: bool,
    /// egui input time (seconds since process start).
    pub time: f64,
    pub launch_grace_secs: f64,
    pub manual_unhide_until: f64,
}

/// One frame's window verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verdict {
    /// Paint the widget and take mouse input.
    pub visible: bool,
    /// When hidden, the window lets clicks through.
    pub passthrough: bool,
    pub band: ZBand,
    pub repaint_ms: u32,
}

/// The single window decision: facts in, verdict out.
///
/// Visibility: a manual minimize always wins; otherwise Smart Focus keeps the
/// widget only while a watched app is open — never mid-drag (passthrough would
/// break the drag), never while settings are open, not during the launch grace
/// period, and not while the user just unhid it by hand.
///
/// Z-band: "Show above all windows" is the topmost band outright. Smart Focus
/// without it is topmost *only while a watched app is in the foreground* —
/// that is what keeps the widget above Antigravity without making it cover
/// everything else. A live foreground test is a hard signal; parking above the
/// watched window in the normal band was a losing race against Windows raising
/// the clicked window.
pub fn decide(f: &WindowFacts) -> Verdict {
    let visible = !f.manual_hidden && !should_hide(f);
    // The band is decided independently of visibility, so unhiding never
    // flashes the window through the wrong z-band for a frame.
    let band = if f.always_on_top || (f.smart_focus && f.foreground_watched) {
        ZBand::Topmost
    } else {
        ZBand::Normal
    };
    Verdict {
        visible,
        passthrough: !visible,
        band,
        repaint_ms: repaint_period_ms(f.settings_open, !visible),
    }
}

/// Smart Focus: hide when no watched app has an open window. The window stays
/// put for the first seconds after launch so the user sees it, while settings
/// are open, while the user just unhid it by hand — and never mid-drag.
pub fn should_hide(input: &WindowFacts) -> bool {
    input.smart_focus
        && !input.settings_open
        && !input.watched_app_open
        && !input.interacting
        && input.time > input.launch_grace_secs
        && input.time >= input.manual_unhide_until
}

/// Where a window dropped at (x, y) with size (w, h) comes to rest inside the
/// monitor work area: snap to an edge the user clearly meant, then keep at
/// least half of it on screen. Returns the adjusted origin.
pub fn snap_to_work_area(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    work_area: (f32, f32, f32, f32),
) -> (f32, f32) {
    let (wl, wt, wr, wb) = work_area;
    let snap_dist = 14.0_f32;
    let mut x = x;
    let mut y = y;

    // Edge snapping (within 14px of screen edges)
    if (x - wl).abs() < snap_dist {
        x = wl;
    } else if ((x + w) - wr).abs() < snap_dist {
        x = wr - w;
    }
    if (y - wt).abs() < snap_dist {
        y = wt;
    } else if ((y + h) - wb).abs() < snap_dist {
        y = wb - h;
    }

    // Safety bounds: keep at least 50% of the widget visible on screen.
    x = x.clamp(wl - w * 0.5, wr - w * 0.5);
    y = y.clamp(wt, wb - h * 0.5);
    (x, y)
}

/// How often the window needs to wake itself, in ms. Settings open: smooth,
/// interactive. Hidden: a slow heartbeat so Smart Focus can re-evaluate.
/// Visible and idle: the countdown only changes once a minute.
pub fn repaint_period_ms(settings_open: bool, idle_hidden: bool) -> u32 {
    if settings_open {
        60
    } else if idle_hidden {
        500
    } else {
        10_000
    }
}

/// Half-width of the resize grip band around the visible circle's rim.
pub const RESIZE_BORDER: f32 = 12.0;

/// Which edge (or corner) the pointer is grabbing for a resize. The widget
/// *looks* like a circle, so the grip is a band around the circle's rim — not
/// around the square window: at the diagonals the rim sits well inside the
/// window's corner bands, and square-based detection made those drags move
/// the window instead. Coordinates are window-local. `None` inside the disc
/// (that stays the drag/click area) and outside the rim band.
pub fn resize_edge(size: (f32, f32), pos: (f32, f32)) -> Option<ResizeZone> {
    let (w, h) = size;
    let (x, y) = pos;
    if x < 0.0 || x > w || y < 0.0 || y > h {
        return None;
    }
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (dx, dy) = (x - cx, y - cy);
    let d = (dx * dx + dy * dy).sqrt();
    let r = w.min(h) / 2.0;
    if (d - r).abs() > RESIZE_BORDER {
        return None;
    }
    // Angle from the centre, screen coords (y down): 0° = east. Every 45°
    // sector is one of the eight grips.
    let a = dy.atan2(dx).to_degrees();
    Some(match a {
        a if (-22.5..22.5).contains(&a) => ResizeZone::Right,
        a if (22.5..67.5).contains(&a) => ResizeZone::BottomRight,
        a if (67.5..112.5).contains(&a) => ResizeZone::Bottom,
        a if (112.5..157.5).contains(&a) => ResizeZone::BottomLeft,
        a if a >= 157.5 || a < -157.5 => ResizeZone::Left,
        a if (-157.5..-112.5).contains(&a) => ResizeZone::TopLeft,
        a if (-112.5..-67.5).contains(&a) => ResizeZone::Top,
        _ => ResizeZone::TopRight,
    })
}

/// One of the eight ways a window can be pulled larger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeZone {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl ResizeZone {
    /// The viewport command direction: the OS resize loop takes over the drag
    /// exactly like it does for a decorated window.
    pub fn direction(self) -> egui::viewport::ResizeDirection {
        use egui::viewport::ResizeDirection::*;
        match self {
            ResizeZone::Left => West,
            ResizeZone::Right => East,
            ResizeZone::Top => North,
            ResizeZone::TopLeft => NorthWest,
            ResizeZone::TopRight => NorthEast,
            ResizeZone::Bottom => South,
            ResizeZone::BottomLeft => SouthWest,
            ResizeZone::BottomRight => SouthEast,
        }
    }

    /// The cursor a normal window shows on this edge.
    pub fn cursor(self) -> egui::CursorIcon {
        use egui::CursorIcon::*;
        match self {
            ResizeZone::Left | ResizeZone::Right => ResizeHorizontal,
            ResizeZone::Top | ResizeZone::Bottom => ResizeVertical,
            ResizeZone::TopLeft | ResizeZone::BottomRight => ResizeNorthWest,
            ResizeZone::TopRight | ResizeZone::BottomLeft => ResizeNorthEast,
        }
    }
}

/// Widget size bounds, in logical points.
pub const SIZE_MIN: u32 = 100;
pub const SIZE_MAX: u32 = 600;

/// The widget is resizable like a normal window: winit supplies the edge
/// grips, and this decides what a frame does about the size it sees.
/// `viewport_w` is the window's current inner width (`None` until known),
/// `applied` is the size we last asked for.
///
/// Returns `Some(size)` when the user has resized the window by hand and the
/// new square size should be adopted (and persisted); `None` when nothing
/// changed.
pub fn adopt_resize(viewport_w: Option<f32>, applied: f32) -> Option<u32> {
    let w = viewport_w?;
    // Tolerate sub-pixel drift and the OS's own rounding: only a deliberate
    // drag moves the window by more than two points.
    if (w - applied).abs() <= 2.0 {
        return None;
    }
    Some((w.round() as u32).clamp(SIZE_MIN, SIZE_MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> WindowFacts {
        WindowFacts {
            manual_hidden: false,
            smart_focus: true,
            always_on_top: false,
            watched_app_open: false,
            foreground_watched: false,
            settings_open: false,
            interacting: false,
            time: 30.0,
            launch_grace_secs: 15.0,
            manual_unhide_until: 0.0,
        }
    }

    #[test]
    fn smart_focus_hides_only_when_no_watched_app_is_open() {
        assert!(should_hide(&facts()), "no watched app open, past the grace period: hide");

        let mut v = facts();
        v.smart_focus = false;
        assert!(!should_hide(&v), "the user switched Smart Focus off");

        let mut v = facts();
        v.settings_open = true;
        assert!(!should_hide(&v), "never hide while settings are open");

        let mut v = facts();
        v.watched_app_open = true;
        assert!(!should_hide(&v), "a watched app window is open — stays on screen");

        let mut v = facts();
        v.interacting = true;
        assert!(!should_hide(&v), "never mid-drag: hiding would break the drag");

        let mut v = facts();
        v.time = 10.0;
        assert!(!should_hide(&v), "inside the launch grace period");

        let mut v = facts();
        v.manual_unhide_until = 60.0;
        assert!(!should_hide(&v), "the user just unhid it by hand");
    }

    #[test]
    fn the_verdict_folds_every_fact_into_one_answer() {
        // Smart Focus, nothing watched: hidden, click-through, normal band.
        let v = decide(&facts());
        assert_eq!(
            v,
            Verdict { visible: false, passthrough: true, band: ZBand::Normal, repaint_ms: 500 }
        );

        // A watched app open and in the foreground: visible *and* topmost.
        let mut f = facts();
        f.watched_app_open = true;
        f.foreground_watched = true;
        let v = decide(&f);
        assert!(v.visible);
        assert_eq!(v.band, ZBand::Topmost, "rides above the watched app");
        assert_eq!(v.repaint_ms, 10_000);

        // Watched app open but another window is in front: visible, normal
        // band — other windows may cover it.
        let mut f = facts();
        f.watched_app_open = true;
        f.foreground_watched = false;
        assert_eq!(decide(&f).band, ZBand::Normal);

        // "Above all windows" wins regardless of everything else.
        let mut f = facts();
        f.always_on_top = true;
        assert_eq!(decide(&f).band, ZBand::Topmost, "topmost even while hidden");
        f.watched_app_open = true;
        f.foreground_watched = false;
        assert_eq!(decide(&f).band, ZBand::Topmost);

        // Manual minimize beats a watched app being open…
        let mut f = facts();
        f.manual_hidden = true;
        f.watched_app_open = true;
        f.foreground_watched = true;
        let v = decide(&f);
        assert!(!v.visible && v.passthrough);
        assert_eq!(v.band, ZBand::Topmost, "the band survives hiding, so unhiding does not flash");

        // …and settings open beats Smart Focus hiding.
        let mut f = facts();
        f.settings_open = true;
        assert!(decide(&f).visible);
        assert_eq!(decide(&f).repaint_ms, 60);

        // No Smart Focus, no always-on-top: an ordinary visible window.
        let mut f = facts();
        f.smart_focus = false;
        let v = decide(&f);
        assert_eq!(
            v,
            Verdict { visible: true, passthrough: false, band: ZBand::Normal, repaint_ms: 10_000 }
        );
    }

    #[test]
    fn a_drop_near_an_edge_snaps_to_it() {
        let wa = (0.0, 0.0, 1000.0, 800.0);
        // 10px from the left edge, 12px from the top: both snap.
        assert_eq!(snap_to_work_area(10.0, 12.0, 100.0, 100.0, wa), (0.0, 0.0));
        // Near the right/bottom edges: the whole window snaps flush.
        assert_eq!(snap_to_work_area(895.0, 100.0, 100.0, 100.0, wa), (900.0, 100.0));
        assert_eq!(snap_to_work_area(100.0, 695.0, 100.0, 100.0, wa), (100.0, 700.0));
        // Far from any edge: untouched.
        assert_eq!(snap_to_work_area(500.0, 400.0, 100.0, 100.0, wa), (500.0, 400.0));
    }

    #[test]
    fn a_drop_mostly_off_screen_keeps_half_visible() {
        let wa = (0.0, 0.0, 1000.0, 800.0);
        // Dragged past the right edge: half may hang over, no more.
        assert_eq!(snap_to_work_area(2000.0, 100.0, 100.0, 100.0, wa), (950.0, 100.0));
        // Dragged above the top: the top edge is a hard stop.
        assert_eq!(snap_to_work_area(100.0, -500.0, 100.0, 100.0, wa), (100.0, 0.0));
        // Dragged below the bottom: half may hang under.
        assert_eq!(snap_to_work_area(100.0, 2000.0, 100.0, 100.0, wa), (100.0, 750.0));
        // A secondary monitor with negative coordinates works the same.
        let wa2 = (-1920.0, 0.0, 0.0, 1080.0);
        assert_eq!(snap_to_work_area(-3000.0, 100.0, 100.0, 100.0, wa2), (-1970.0, 100.0));
    }

    #[test]
    fn the_repaint_period_follows_the_window_state() {
        assert_eq!(repaint_period_ms(true, false), 60);
        assert_eq!(repaint_period_ms(false, true), 500);
        assert_eq!(repaint_period_ms(false, false), 10_000);
        // Settings win over hidden, should both ever be true.
        assert_eq!(repaint_period_ms(true, true), 60);
    }

    #[test]
    fn the_rim_is_grabbable_and_the_body_is_not() {
        // 200×200 window, inscribed disc: r = 100, rim band ±12 around it.
        let size = (200.0, 200.0);
        // On the rim, from the east round to the north-east: one grip per 45°.
        assert_eq!(resize_edge(size, (200.0, 100.0)), Some(ResizeZone::Right));
        assert_eq!(resize_edge(size, (6.0, 100.0)), Some(ResizeZone::Left), "just inside the rim still grabs");
        assert_eq!(resize_edge(size, (100.0, 8.0)), Some(ResizeZone::Top));
        assert_eq!(resize_edge(size, (100.0, 192.0)), Some(ResizeZone::Bottom));
        let d = 100.0 / std::f32::consts::SQRT_2; // rim point at 45°
        assert_eq!(resize_edge(size, (100.0 + d, 100.0 - d)), Some(ResizeZone::TopRight));
        assert_eq!(resize_edge(size, (100.0 + d, 100.0 + d)), Some(ResizeZone::BottomRight));
        assert_eq!(resize_edge(size, (100.0 - d, 100.0 - d)), Some(ResizeZone::TopLeft));
        assert_eq!(resize_edge(size, (100.0 - d, 100.0 + d)), Some(ResizeZone::BottomLeft));
        // Inside the disc: drag area.
        assert_eq!(resize_edge(size, (100.0, 100.0)), None, "the body drags");
        assert_eq!(resize_edge(size, (160.0, 100.0)), None, "well inside the rim");
        // Square corners are nowhere near the circular rim: they drag.
        assert_eq!(resize_edge(size, (3.0, 3.0)), None, "a corner is outside the rim band");
        assert_eq!(resize_edge(size, (-5.0, 100.0)), None, "outside is nothing");
    }

    #[test]
    fn a_hand_resize_is_adopted_and_drift_is_not() {
        // Sub-pixel / OS rounding drift: ignored.
        assert_eq!(adopt_resize(Some(220.8), 220.0), None);
        // A deliberate drag: adopted, and clamped to the widget bounds.
        assert_eq!(adopt_resize(Some(264.0), 220.0), Some(264));
        assert_eq!(adopt_resize(Some(50.0), 220.0), Some(SIZE_MIN));
        assert_eq!(adopt_resize(Some(1200.0), 220.0), Some(SIZE_MAX));
        // Nothing known yet: no verdict.
        assert_eq!(adopt_resize(None, 220.0), None);
    }
}
