//! System-tray icon and its right-click menu — one of the two adapters over
//! the shared `MenuAction` vocabulary (see menu.rs). This module builds the
//! menu and translates clicks; the effects live in `App::apply_menu_action`.

use crate::i18n::Language;
use crate::icon;
use crate::menu::MenuAction;
use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Fixed id of the quit item, so the tray event handler can exit instantly
/// without waiting for a UI frame.
pub const QUIT_MENU_ID: &str = "tokpaek-quit";

pub struct Tray {
    tray: TrayIcon,
    id_refresh: MenuId,
    refresh: MenuItem,
    id_home: MenuId,
    home: MenuItem,
    topmost_item: CheckMenuItem,
    id_topmost: MenuId,
    id_settings: MenuId,
    settings: MenuItem,
    quit: MenuItem,
}

impl Tray {
    pub fn new(always_on_top: bool, lang: Language) -> Result<Self, String> {
        let ic = Icon::from_rgba(icon::rgba(), icon::SIZE, icon::SIZE)
            .map_err(|e| format!("icon: {e}"))?;

        let menu = Menu::new();

        let refresh = MenuItem::new(lang.text("Обновить", "Refresh"), true, None);
        let home = MenuItem::new(lang.text("Домой", "Home"), true, None);
        let topmost_item = CheckMenuItem::new(
            lang.text("Показывать поверх всех окон", "Show above all windows"),
            true,
            always_on_top,
            None,
        );
        let settings = MenuItem::new(lang.text("Настройки", "Settings"), true, None);
        let quit = MenuItem::with_id(MenuId::new(QUIT_MENU_ID), lang.text("Закрыть", "Quit"), true, None);

        let _ = menu.append_items(&[
            &refresh,
            &home,
            &topmost_item,
            &settings,
            &PredefinedMenuItem::separator(),
            &quit,
        ]);

        let tray = TrayIconBuilder::new()
            .with_tooltip(lang.text("Токпаёк", "Tokpaek"))
            .with_icon(ic)
            .with_menu(Box::new(menu))
            .build()
            .map_err(|e| format!("tray build: {e}"))?;

        Ok(Self {
            tray,
            id_refresh: refresh.id().clone(),
            refresh,
            id_home: home.id().clone(),
            home,
            id_topmost: topmost_item.id().clone(),
            topmost_item,
            id_settings: settings.id().clone(),
            settings,
            quit,
        })
    }

    /// What a menu event means, with the new check-state already read from the
    /// item the user clicked.
    pub fn action(&self, id: &MenuId) -> Option<MenuAction> {
        if *id == self.quit.id().clone() {
            Some(MenuAction::Quit)
        } else if *id == self.id_refresh {
            Some(MenuAction::Refresh)
        } else if *id == self.id_home {
            Some(MenuAction::Home)
        } else if *id == self.id_settings {
            Some(MenuAction::OpenSettings)
        } else if *id == self.id_topmost {
            Some(MenuAction::AlwaysOnTop(self.topmost_item.is_checked()))
        } else {
            None
        }
    }

    pub fn set_topmost_checked(&self, on: bool) {
        if self.topmost_item.is_checked() != on {
            self.topmost_item.set_checked(on);
        }
    }

    pub fn set_language(&self, lang: Language) {
        self.refresh.set_text(lang.text("Обновить", "Refresh"));
        self.home.set_text(lang.text("Домой", "Home"));
        self.topmost_item.set_text(lang.text(
            "Показывать поверх всех окон",
            "Show above all windows",
        ));
        self.settings.set_text(lang.text("Настройки", "Settings"));
        self.quit.set_text(lang.text("Закрыть", "Quit"));
    }

    /// Hover text — also where a pending update is announced.
    pub fn set_tooltip(&self, text: &str) {
        let _ = self.tray.set_tooltip(Some(text));
    }
}
