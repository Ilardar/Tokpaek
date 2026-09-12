//! The one vocabulary of menu actions, shared by both menu adapters: the tray
//! menu (tray.rs) and the widget's Win32 context menu (app.rs). Each action's
//! effect is written exactly once, in `App::apply_menu_action`; the menus only
//! present the items and translate clicks back into `MenuAction`.

/// What the user picked in either menu. Check actions carry the new
/// check-state, read from the menu itself, so handlers never touch items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// Re-poll every enabled family right away.
    Refresh,
    /// Back to the default position (the screen's top-left corner).
    Home,
    /// "Show above all windows", with its new state.
    AlwaysOnTop(bool),
    OpenSettings,
    /// Flush state and exit the process.
    Quit,
}
