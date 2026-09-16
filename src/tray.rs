//! System-tray icon and its right-click menu — one of the two adapters over
//! the shared `MenuAction` vocabulary (see menu.rs). This module builds the
//! menu and translates clicks; the effects live in `App::apply_menu_action`.

use crate::i18n::Language;
use crate::icon;
use crate::menu::MenuAction;
use crate::providers::Family;
use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem};
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
    /// "Auto (follow the active window)" — clears the pin.
    auto_item: CheckMenuItem,
    id_auto: MenuId,
    /// One check item per family; which one is ticked mirrors the pinned source.
    sources: Vec<(Family, CheckMenuItem)>,
    id_settings: MenuId,
    settings: MenuItem,
    quit: MenuItem,
}

impl Tray {
    pub fn new(always_on_top: bool, pinned: Option<Family>, lang: Language) -> Result<Self, String> {
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
        let auto_item = CheckMenuItem::new(
            lang.text("Авто (по активному окну)", "Auto (follow active window)"),
            true,
            pinned.is_none(),
            None,
        );
        let sources: Vec<(Family, CheckMenuItem)> = Family::ALL
            .into_iter()
            .map(|f| (f, CheckMenuItem::new(f.name(), true, pinned == Some(f), None)))
            .collect();
        let settings = MenuItem::new(lang.text("Настройки", "Settings"), true, None);
        let quit = MenuItem::with_id(
            MenuId::new(QUIT_MENU_ID),
            lang.text("Закрыть", "Quit"),
            true,
            None,
        );

        let sep_sources = PredefinedMenuItem::separator();
        let sep_quit = PredefinedMenuItem::separator();

        let mut items: Vec<&dyn IsMenuItem> = vec![&refresh, &home, &topmost_item];
        items.push(&sep_sources);
        items.push(&auto_item);
        for (_, item) in &sources {
            items.push(item);
        }
        items.push(&settings);
        items.push(&sep_quit);
        items.push(&quit);
        let _ = menu.append_items(&items);

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
            id_auto: auto_item.id().clone(),
            auto_item,
            sources,
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
        } else if *id == self.id_auto {
            Some(MenuAction::ShowFamily(None))
        } else if let Some((f, _)) = self.sources.iter().find(|(_, item)| item.id() == id) {
            Some(MenuAction::ShowFamily(Some(*f)))
        } else {
            None
        }
    }

    pub fn set_topmost_checked(&self, on: bool) {
        if self.topmost_item.is_checked() != on {
            self.topmost_item.set_checked(on);
        }
    }

    /// Mirror the widget's source choice into the tray checkmarks, and gray out
    /// sources switched off in Settings → Sources.
    pub fn set_source_checked(&self, pinned: Option<Family>, enabled: u8) {
        if self.auto_item.is_checked() != pinned.is_none() {
            self.auto_item.set_checked(pinned.is_none());
        }
        for (f, item) in &self.sources {
            let on = enabled & (1 << f.idx()) != 0;
            let checked = pinned == Some(*f);
            if item.is_checked() != checked {
                item.set_checked(checked);
            }
            if item.is_enabled() != on {
                item.set_enabled(on);
            }
        }
    }

    pub fn set_language(&self, lang: Language) {
        self.refresh.set_text(lang.text("Обновить", "Refresh"));
        self.home.set_text(lang.text("Домой", "Home"));
        self.topmost_item.set_text(lang.text(
            "Показывать поверх всех окон",
            "Show above all windows",
        ));
        self.auto_item.set_text(lang.text(
            "Авто (по активному окну)",
            "Auto (follow active window)",
        ));
        self.settings.set_text(lang.text("Настройки", "Settings"));
        self.quit.set_text(lang.text("Закрыть", "Quit"));
    }

    /// Hover text — also where a pending update is announced.
    pub fn set_tooltip(&self, text: &str) {
        let _ = self.tray.set_tooltip(Some(text));
    }
}
