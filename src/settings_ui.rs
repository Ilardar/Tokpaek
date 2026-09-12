//! The settings window: a borderless panel painted in the same palette as the
//! strip, opened centred on whichever monitor the user called it from.

use crate::app::App;
use crate::i18n::{tr_format, Language};
use crate::providers::Family;
use crate::shortcuts;

use eframe::egui;
use egui::{Color32, RichText, Rounding, Sense, Stroke, Vec2, ViewportCommand};
use std::sync::atomic::Ordering;

const BG: Color32 = Color32::from_rgb(18, 20, 26);
const CARD: Color32 = Color32::from_rgb(30, 33, 42);
const CARD_HI: Color32 = Color32::from_rgb(48, 53, 65);
const ACCENT: Color32 = Color32::from_rgb(110, 210, 146);
const ACCENT_BG: Color32 = Color32::from_rgb(44, 86, 63);
const TEXT: Color32 = Color32::from_rgb(234, 238, 246);
const DIM: Color32 = Color32::from_rgb(176, 184, 200);
const HINT: Color32 = Color32::from_rgb(158, 167, 184);
const WARN: Color32 = Color32::from_rgb(238, 162, 92);

const WIN_W: f32 = 460.0;
const WIN_H: f32 = 600.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Appearance,
    Behavior,
    Sources,
    System,
}

/// Windows' own UI face, in the weight the strip is drawn in. It is read from
/// the system font folder rather than bundled — the licence does not allow
/// shipping it, and it is on every Windows 10/11 machine anyway. egui's bundled
/// font stays behind it as the fallback for anything Segoe has no glyph for,
/// and for the machine where the file is missing.
fn install_font(ctx: &egui::Context) {
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_string());
    let path = std::path::Path::new(&dir).join("Fonts").join("seguisb.ttf");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            crate::diaglog::dbg_log(&format!("font: {} unreadable: {e}", path.display()));
            return;
        }
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert(FONT.to_owned(), egui::FontData::from_owned(bytes));
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, FONT.to_owned());
    ctx.set_fonts(fonts);
}

const FONT: &str = "segoe-ui-semibold";

/// Dark palette shared with the strip. Written into *both* theme slots and the
/// theme pinned to dark: egui keeps a style per theme and switches to the
/// system one as soon as winit reports it, which would otherwise drop this.
pub fn apply_style(ctx: &egui::Context) {
    install_font(ctx);
    let mut style = (*ctx.style()).clone();
    let mut v = egui::Visuals::dark();

    v.panel_fill = BG;
    v.window_fill = BG;
    v.extreme_bg_color = Color32::from_rgb(14, 16, 21);
    v.faint_bg_color = CARD;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.window_rounding = Rounding::same(12.0);
    v.window_stroke = Stroke::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;
    v.window_shadow = egui::epaint::Shadow::NONE;

    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = Rounding::same(7.0);
        w.bg_stroke = Stroke::NONE;
        w.fg_stroke = Stroke::new(1.8_f32, ACCENT);
    }
    v.widgets.noninteractive.rounding = Rounding::same(7.0);
    v.widgets.noninteractive.bg_stroke = Stroke::NONE;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.inactive.bg_fill = CARD_HI;
    v.widgets.inactive.weak_bg_fill = CARD_HI;
    v.widgets.hovered.bg_fill = Color32::from_rgb(60, 66, 80);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(60, 66, 80);
    v.widgets.active.bg_fill = Color32::from_rgb(70, 77, 92);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(70, 77, 92);
    v.selection.bg_fill = ACCENT_BG;
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);

    style.visuals = v;
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.interact_size.y = 22.0;
    style.spacing.scroll = egui::style::ScrollStyle::solid();
    style.spacing.scroll.bar_width = 9.0;

    style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(13.5));
    style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
    style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));

    ctx.options_mut(|o| o.zoom_with_keyboard = false);

    let style: std::sync::Arc<egui::Style> = style.into();
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
}

impl App {
    pub(crate) fn render_settings(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        if !self.show_settings {
            return;
        }
        let mut close = false;
        let mut refresh_now = false;
        let mut check_update = false;

        let vid = egui::ViewportId::from_hash_of("tokpaek-settings");
        let mut builder = egui::ViewportBuilder::default()
            .with_title(lang.text("Токпаёк — Настройки", "Tokpaek — Settings"))
            .with_inner_size([WIN_W, WIN_H])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false);

        if let Some((left, top, right, bottom)) = self.settings_area.or_else(crate::windowing::cursor_work_area) {
            let x = left as f32 + ((right - left) as f32 - WIN_W) / 2.0;
            let y = top as f32 + ((bottom - top) as f32 - WIN_H) / 2.0;
            builder = builder.with_position([x.max(left as f32), y.max(top as f32)]);
        }

        ctx.show_viewport_immediate(vid, builder, |ctx, _class| {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::none()
                        .fill(BG)
                        .rounding(Rounding::same(12.0))
                        .inner_margin(egui::Margin::symmetric(14.0, 12.0)),
                )
                .show(ctx, |ui| {
                    // No footer: the ✕ in the title bar (and Escape) close the
                    // window; a "Close" button next to them is one control too
                    // many.
                    close |= self.title_bar(ui, ctx);
                    ui.add_space(4.0);
                    self.tabs_bar(ui);
                    ui.add_space(6.0);

                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            match self.settings_tab {
                                SettingsTab::Appearance => {
                                    self.appearance_card(ui);
                                }
                                SettingsTab::Behavior => {
                                    self.behavior_card(ui);
                                }
                                SettingsTab::Sources => {
                                    refresh_now |= self.sources_card(ui);
                                }
                                SettingsTab::System => {
                                    check_update |= self.system_card(ui);
                                }
                            }
                        });
                });

            if ctx.input(|i| i.viewport().close_requested())
                || ctx.input(|i| i.key_pressed(egui::Key::Escape))
            {
                close = true;
            }
        });

        if self.settings_hwnd.is_none() {
            self.settings_hwnd = crate::windowing::find_settings_wnd();
            #[cfg(windows)]
            if let Some(h) = self.settings_hwnd {
                crate::windowing::strip_chrome(windows::Win32::Foundation::HWND(h as *mut _));
            }
        }

        // The settings window has its own behavior, independent of the
        // widget's "always on top" choice: while it is open it stays above
        // every window. winit's always_on_top loses to other topmost windows,
        // so re-assert it through Win32 every frame while open.
        if let Some(h) = self.settings_hwnd {
            crate::windowing::set_topmost(h);
        }

        if self.settings_center {
            if let Some(h) = self.settings_hwnd {
                if crate::windowing::center(h, self.settings_area) {
                    self.settings_center = false;
                    #[cfg(windows)]
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
        }

        if refresh_now {
            self.shared.refresh.store(true, Ordering::Relaxed);
        }
        if check_update {
            self.shared.update_now.store(true, Ordering::Relaxed);
        }
        if close {
            self.show_settings = false;
            self.settings_center = false;
            self.settings_hwnd = None;
            self.manual_unhide_until = ctx.input(|i| i.time) + 5.0;
            self.settings.save();
        }
        self.shared
            .interval
            .store(self.settings.poll_secs, Ordering::Relaxed);
        self.shared
            .enabled
            .store(self.settings.enabled_mask(), Ordering::Relaxed);
    }

    fn tabs_bar(&mut self, ui: &mut egui::Ui) {
        let lang = self.settings.language;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let mut tab = |ui: &mut egui::Ui, t: SettingsTab, label: &str| {
                if ui
                    .selectable_label(
                        self.settings_tab == t,
                        RichText::new(label).size(13.0).strong(),
                    )
                    .clicked()
                {
                    self.settings_tab = t;
                }
            };
            tab(ui, SettingsTab::Appearance, lang.text("Вид", "Appearance"));
            tab(ui, SettingsTab::Behavior, lang.text("Поведение", "Behavior"));
            tab(ui, SettingsTab::Sources, lang.text("Источники", "Sources"));
            tab(ui, SettingsTab::System, lang.text("Система", "System"));
        });
    }

    /// Custom chrome: drag anywhere on the bar, ✕ closes.
    fn title_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
        let lang = self.settings.language;
        let mut close = false;
        let bar =
            egui::Rect::from_min_size(ui.max_rect().min, Vec2::new(ui.max_rect().width(), 26.0));
        let drag = ui.interact(bar, ui.id().with("settings-drag"), Sense::click_and_drag());
        if drag.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(lang.text("Токпаёк", "Tokpaek")).size(15.0).strong().color(TEXT));
            ui.label(RichText::new("—").size(15.0).color(HINT));
            ui.label(
                RichText::new(lang.text("Настройки", "Settings"))
                    .size(15.0)
                    .color(DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = close_button(ui);
            });
        });
        close
    }

    fn appearance_card(&mut self, ui: &mut egui::Ui) {
        use crate::config::CirclePalette;
        let lang = self.settings.language;
        card(ui, lang.text("ШКАЛА «КРУГ»", "CIRCLE GAUGE"), |ui| {
            let s = &mut self.settings;

            caption(ui, lang.text("Цветовая палитра", "Color palette"));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let mut pick = |ui: &mut egui::Ui, pal: CirclePalette, label: &str| {
                    if ui.selectable_label(s.circle_palette == pal, label).clicked() {
                        s.circle_palette = pal;
                        s.save();
                    }
                };
                pick(ui, CirclePalette::Gradient, lang.text("Градиент", "Gradient"));
                pick(ui, CirclePalette::Traffic, lang.text("Светофор", "Traffic"));
                pick(ui, CirclePalette::Cyan, lang.text("Бирюзовый", "Cyan"));
                pick(ui, CirclePalette::Monochrome, lang.text("Монохром", "Monochrome"));
            });

            ui.add_space(4.0);
            caption(ui, lang.text("Формат шкалы", "Scale format"));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let mut pick = |ui: &mut egui::Ui, seg: usize, label: &str| {
                    if ui.selectable_label(s.circle_segments == seg, label).clicked() {
                        s.circle_segments = seg;
                        s.save();
                    }
                };
                pick(ui, 10, lang.text("Десятичная", "Decimal"));
                pick(ui, 12, lang.text("Часовая", "Clock"));
                pick(ui, 0, lang.text("Сплошная", "Solid"));
            });
        });

        card(ui, lang.text("РАЗМЕР И ПРОЗРАЧНОСТЬ", "SIZE & TRANSPARENCY"), |ui| {
            let s = &mut self.settings;
            value_row(
                ui,
                lang.text("Размер виджета", "Widget size"),
                &format!("{}px", s.circle_size),
            );
            if full_width_slider(ui, &mut s.circle_size, SIZE_MIN..=SIZE_MAX) {
                s.save();
            }
            // Preset marks under the slider, each under its own point.
            if let Some(v) = slider_marks(
                ui,
                SIZE_MIN as f32,
                SIZE_MAX as f32,
                &SIZE_PRESETS,
                false,
                &|v| v == s.circle_size as f32,
                &|v| format!("{:.0}px", v),
            ) {
                s.circle_size = v as u32;
                s.save();
            }
            ui.label(
                RichText::new(lang.text(
                    "Размер виджета меняется перетаскиванием его края, как у обычного окна.",
                    "Resize the widget by dragging its edge, like any window.",
                ))
                .size(12.0)
                .color(HINT),
            );

            ui.add_space(8.0);
            // The slider carries opacity, the user reads transparency.
            let mut transparency = (1.0 - s.opacity).clamp(0.0, 1.0);
            value_row(
                ui,
                lang.text("Прозрачность", "Transparency"),
                &format!("{:.0}%", transparency * 100.0),
            );
            let changed = full_width_slider(ui, &mut transparency, TRANSP_MIN..=TRANSP_MAX);
            // Preset marks under the slider: 0% on the left end, 80% on the
            // right, each under the point it stands for.
            if let Some(v) = slider_marks(
                ui,
                TRANSP_MIN,
                TRANSP_MAX,
                &TRANSP_PRESETS,
                false,
                &|v| (transparency - v).abs() < 0.005,
                &|v| format!("{:.0}%", v * 100.0),
            ) {
                transparency = v;
                s.opacity = (1.0 - transparency).clamp(OPACITY_MIN, OPACITY_MAX);
                s.save();
            }
            if changed {
                s.opacity = (1.0 - transparency).clamp(OPACITY_MIN, OPACITY_MAX);
                s.save();
            }
        });
    }

    /// Window behavior and the gauge's timers.
    fn behavior_card(&mut self, ui: &mut egui::Ui) {
        let lang = self.settings.language;
        card(ui, lang.text("ТАЙМЕР В ЦЕНТРАЛЬНОЙ ПЛАШКЕ", "MIDDLE PILL TIMER"), |ui| {
            let s = &mut self.settings;
            caption(ui, lang.text("Показывать сброс квоты", "Show quota reset of"));
            // A switch between the two things it can show, so the choice is
            // visible instead of implied by a checkbox label.
            let locked = s.circle_show_claude_gpt;
            ui.add_enabled_ui(!locked, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    let mut pick = |ui: &mut egui::Ui, weekly: bool, label: &str| {
                        if ui
                            .selectable_label(s.circle_show_weekly_reset == weekly, label)
                            .clicked()
                        {
                            s.circle_show_weekly_reset = weekly;
                            s.save();
                        }
                    };
                    pick(ui, false, lang.text("ПЯТЬ ЧАСОВ", "FIVE HOURS"));
                    pick(ui, true, lang.text("СЕМЬ ДНЕЙ", "SEVEN DAYS"));
                });
            });
            ui.label(
                RichText::new(if locked {
                    lang.text(
                        "Недоступно, пока нижняя дуга заменена на Claude / GPT",
                        "Unavailable while the lower arc is replaced with Claude / GPT",
                    )
                } else {
                    lang.text(
                        "Также переключается кликом по виджету.",
                        "Also toggled by clicking the widget.",
                    )
                })
                .size(12.0)
                .color(HINT),
            );

            ui.add_space(6.0);
            if ui
                .checkbox(
                    &mut s.circle_show_claude_gpt,
                    lang.text(
                        "Заменить шкалу «СЕМЬ ДНЕЙ» на Claude / GPT",
                        "Replace SEVEN DAYS with Claude / GPT",
                    ),
                )
                .changed()
            {
                // The tray checkmark follows on the next frame: update() is
                // the one place that mirrors settings into the tray.
                if s.circle_show_claude_gpt {
                    s.circle_show_weekly_reset = false;
                }
                s.save();
            }
            ui.label(
                RichText::new(lang.text(
                    "Отображает на нижней полуокружности лимит сторонних моделей вместо остатка за семь дней Gemini.",
                    "Displays third-party model limits on lower semicircle instead of Gemini seven-day quota.",
                ))
                .size(12.0)
                .color(HINT),
            );
        });

        card(ui, lang.text("ПОВЕДЕНИЕ ОКНА", "WINDOW BEHAVIOR"), |ui| {
            let s = &mut self.settings;
            if ui
                .checkbox(
                    &mut s.smart_focus,
                    lang.text("Умный фокус", "Smart focus"),
                )
                .changed()
            {
                s.save();
            }
            ui.label(
                RichText::new(lang.text(
                    "Показывает виджет, только пока открыто окно Antigravity, Claude или ChatGPT.",
                    "Shows the widget only while an Antigravity, Claude or ChatGPT window is open.",
                ))
                .size(12.0)
                .color(HINT),
            );

            ui.add_space(6.0);
            if ui
                .checkbox(
                    &mut s.always_on_top,
                    lang.text(
                        "Показывать поверх всех окон",
                        "Show above all windows",
                    ),
                )
                .changed()
            {
                s.save();
            }
            ui.label(
                RichText::new(lang.text(
                    "Виджет остаётся выше любого окна. Без этой галки он держится поверх окон приложений, но обычные окна могут его накрыть.",
                    "The widget stays above every window. Without it the widget rides over the watched apps' windows, but other windows can cover it.",
                ))
                .size(12.0)
                .color(HINT),
            );
        });
    }

    fn sources_card(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.settings.language;
        let mut refresh = false;
        let status: Vec<(bool, bool, Option<String>)> = {
            let st = self.shared.states.lock().unwrap();
            Family::ALL
                .iter()
                .map(|f| {
                    let s = &st[f.idx()];
                    (s.online, s.ever, s.error.clone())
                })
                .collect()
        };

        card(ui, lang.text("ИСТОЧНИКИ", "SOURCES"), |ui| {
            for f in Family::ALL {
                let (online, ever, err) = &status[f.idx()];
                ui.horizontal(|ui| {
                    let mut on = self.settings.enabled(f);
                    if ui.checkbox(&mut on, f.name()).changed() {
                        self.settings.set_enabled(f, on);
                        self.settings.save();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (text, col) = if !self.settings.enabled(f) {
                            (lang.text("выключен", "disabled").to_string(), HINT)
                        } else if *online {
                            (
                                lang.text("данные получены", "connected").to_string(),
                                ACCENT,
                            )
                        } else if let Some(e) = err {
                            (short(e), WARN)
                        } else if *ever {
                            (lang.text("нет связи", "offline").to_string(), WARN)
                        } else {
                            (lang.text("опрос…", "loading…").to_string(), HINT)
                        };
                        ui.label(RichText::new(text).size(12.5).color(col));
                    });
                });
            }
        });

        refresh |= self.polling_card(ui);
        refresh
    }

    /// Returns true when "обновить сейчас" was pressed.
    fn polling_card(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.settings.language;
        let mut refresh = false;
        card(ui, lang.text("ОПРОС КВОТ", "QUOTA POLLING"), |ui| {
            let s = &mut self.settings;
            value_row(
                ui,
                lang.text("Интервал опроса", "Polling interval"),
                &tr_format!(lang, "{} секунд", "{} seconds", s.poll_secs),
            );
            // A logarithmic slider: 15…600 s spans 6×, so on a linear track
            // the presets bunch up at the left end. Log spacing puts every
            // mark where its number sits.
            let changed = log_width_slider(ui, &mut s.poll_secs, POLL_MIN..=POLL_MAX);
            // Preset marks under the slider, one under its own point.
            if let Some(v) = slider_marks(
                ui,
                POLL_MIN as f32,
                POLL_MAX as f32,
                &POLL_PRESETS,
                true,
                &|v| v == s.poll_secs as f32,
                &|v| format!("{:.0}", v),
            ) {
                s.poll_secs = v as u64;
                s.save();
            }
            if changed {
                s.save();
            }

            ui.add_space(6.0);
            if ui
                .button(lang.text("Обновить сейчас", "Refresh now"))
                .clicked()
            {
                refresh = true;
            }
        });
        refresh
    }

    /// System settings: Language, Behavior, Windows Integration, Updates, Diagnostics.
    fn system_card(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.settings.language;
        let mut check_update = false;

        card(ui, lang.text("ЯЗЫК ИНТЕРФЕЙСА", "INTERFACE LANGUAGE"), |ui| {
            ui.horizontal(|ui| {
                let mut changed = ui
                    .selectable_value(&mut self.settings.language, Language::Russian, "Русский")
                    .changed();
                changed |= ui
                    .selectable_value(&mut self.settings.language, Language::English, "English")
                    .changed();
                if changed {
                    self.settings.save();
                    if let Some(tray) = &self.tray {
                        tray.set_language(self.settings.language);
                    }
                }
            });
        });

        card(ui, lang.text("ИНТЕГРАЦИЯ С WINDOWS", "WINDOWS INTEGRATION"), |ui| {
            let mut a = self.autostart;
            if ui
                .checkbox(
                    &mut a,
                    lang.text(
                        "Автозапуск при входе (ярлык в Startup)",
                        "Start at sign-in (Startup shortcut)",
                    ),
                )
                .changed()
                && shortcuts::set_autostart(a).is_ok()
            {
                self.autostart = a;
            }
            if ui
                .button(lang.text("Создать ярлык на рабочем столе", "Create desktop shortcut"))
                .clicked()
            {
                let _ = shortcuts::force_desktop_shortcut();
            }

            ui.add_space(4.0);
            if let Some(dir) = crate::config::Settings::dir() {
                caption(ui, lang.text("Папка настроек", "Settings folder"));
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(dir.display().to_string())
                            .size(12.0)
                            .color(HINT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(lang.text("Открыть", "Open"))
                            .clicked()
                        {
                            reveal_path(&dir);
                        }
                    });
                });
            }
        });

        card(ui, lang.text("ВЕРСИЯ И ОБНОВЛЕНИЯ", "VERSION & UPDATES"), |ui| {
            let (checked, available, failed) = {
                let st = self.shared.update.lock().unwrap();
                (st.checked, st.available.clone(), st.error.is_some())
            };

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} {}",
                        lang.text("Токпаёк", "Tokpaek"),
                        crate::update::current()
                    ))
                    .size(13.5)
                    .color(TEXT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (text, col) = match (&available, checked, failed) {
                        (Some(u), _, _) => (
                            tr_format!(lang, "доступна {}", "{} available", u.version),
                            WARN,
                        ),
                        (None, true, false) => (
                            lang.text("актуальная версия", "up to date").to_string(),
                            ACCENT,
                        ),
                        (None, true, true) => (
                            lang.text("проверка не удалась", "check failed").to_string(),
                            HINT,
                        ),
                        _ => (lang.text("проверка…", "checking…").to_string(), HINT),
                    };
                    ui.label(RichText::new(text).size(12.5).color(col));
                });
            });

            ui.horizontal(|ui| {
                if ui
                    .button(lang.text("Проверить обновления", "Check for updates"))
                    .clicked()
                {
                    check_update = true;
                }
                if let Some(u) = &available {
                    ui.hyperlink_to(
                        RichText::new(lang.text("Открыть страницу релиза", "Open release page"))
                            .size(12.5)
                            .color(ACCENT),
                        u.url.clone(),
                    );
                } else {
                    ui.hyperlink_to(
                        RichText::new(lang.text("Страница релизов", "Releases page"))
                            .size(12.5)
                            .color(ACCENT),
                        "https://github.com/Ilardar/Tokpaek/releases",
                    );
                }
            });
        });

        card(ui, lang.text("ДИАГНОСТИКА", "DIAGNOSTICS"), |ui| {
            let mut diag = self.settings.diagnostics;
            if ui
                .checkbox(
                    &mut diag,
                    lang.text("Подробная диагностика", "Detailed diagnostics"),
                )
                .changed()
            {
                self.settings.diagnostics = diag;
                crate::diaglog::set_diagnostics(diag);
                self.settings.save();
            }
            ui.label(
                RichText::new(lang.text(
                    "Пишет в tokpaek-debug.log коды ответов, адрес API и время запросов.",
                    "Logs response codes, API addresses and request times to tokpaek-debug.log.",
                ))
                .size(12.0)
                .color(HINT),
            );
            if ui
                .button(lang.text("Показать файл журнала", "Show log file"))
                .clicked()
            {
                reveal_log();
            }
        });

        check_update
    }
}

// ---------------------------------------------------------------------------
// Small pieces
// ---------------------------------------------------------------------------

/// Section: a dim caption over a rounded card.
fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(5.0);
    ui.label(RichText::new(title).size(12.0).strong().color(DIM));
    ui.add_space(1.0);
    egui::Frame::none()
        .fill(CARD)
        .rounding(Rounding::same(10.0))
        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.5).color(DIM));
}

/// "Label ………… value" — the label in the same face as `caption` (dim,
/// not strong), the value in the accent colour.
fn value_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(12.5).color(DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(value).size(12.5).color(ACCENT));
        });
    });
}

/// A slider that spans the card and shows no value of its own (the row above
/// carries it), so long Russian labels never squeeze it.
fn full_width_slider<T: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut T,
    range: std::ops::RangeInclusive<T>,
) -> bool {
    ui.spacing_mut().slider_width = ui.available_width() - 6.0;
    ui.add(egui::Slider::new(value, range).show_value(false))
        .changed()
}

/// Logarithmic variant: presets spread evenly along the track instead of
/// bunching up at the small end.
fn log_width_slider<T: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut T,
    range: std::ops::RangeInclusive<T>,
) -> bool {
    ui.spacing_mut().slider_width = ui.available_width() - 6.0;
    ui.add(egui::Slider::new(value, range).show_value(false).logarithmic(true))
        .changed()
}

const OPACITY_MIN: f32 = 0.2;
const OPACITY_MAX: f32 = 1.0;

/// Widget size bounds and presets, in logical points.
const SIZE_MIN: u32 = crate::frame_policy::SIZE_MIN;
const SIZE_MAX: u32 = 600;
const SIZE_PRESETS: [f32; 6] = [100.0, 200.0, 300.0, 400.0, 500.0, 600.0];

/// Transparency = 1 − opacity, so its slider runs the other way round.
const TRANSP_MIN: f32 = 0.0;
const TRANSP_MAX: f32 = 0.8;
const TRANSP_PRESETS: [f32; 5] = [0.0, 0.2, 0.4, 0.6, 0.8];

const POLL_MIN: u64 = 15;
const POLL_MAX: u64 = 600;
/// Preset intervals in seconds, matching the marks under the slider.
const POLL_PRESETS: [f32; 6] = [600.0, 300.0, 120.0, 60.0, 30.0, 15.0];

/// Clickable labels positioned under a slider's marks: each sits under the
/// point of the track it stands for. `log` matches a logarithmic slider: the
/// marks spread by log-distance, not linear. Returns the picked value, if any.
/// `on` marks the currently selected label, `label` formats one.
fn slider_marks(
    ui: &mut egui::Ui,
    min: f32,
    max: f32,
    presets: &[f32],
    log: bool,
    on: &dyn Fn(f32) -> bool,
    label: &dyn Fn(f32) -> String,
) -> Option<f32> {
    // The slider is inset by egui's widget padding; match its track closely
    // enough that labels land under their points.
    let track_w = (ui.available_width() - 6.0).max(40.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(track_w, 24.0), Sense::hover());
    let pad = 10.0; // half the handle's width: the value span maps inside it
    let usable = track_w - 2.0 * pad;
    let label_w = 46.0;
    let mut picked = None;

    for &val in presets {
        let frac = if log {
            (val.max(min).ln() - min.ln()) / (max.ln() - min.ln())
        } else {
            (val - min) / (max - min)
        };
        let x = rect.min.x + pad + frac.clamp(0.0, 1.0) * usable;
        let text = label(val);
        let col = if on(val) { ACCENT } else { DIM };
        // Clamped inside the track: the end labels sit flush with the slider's
        // ends instead of hanging over the card.
        let cx = x.clamp(rect.min.x + label_w / 2.0, rect.max.x - label_w / 2.0);
        let label_rect = egui::Rect::from_center_size(
            egui::Pos2::new(cx, rect.center().y),
            Vec2::new(label_w, rect.height()),
        );
        ui.allocate_new_ui(
            egui::UiBuilder::new()
                .max_rect(label_rect)
                .layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)),
            |ui| {
                // Same face as the buttons (13.0 strong in the app style), so
                // every clickable element reads the same.
                let resp = ui.add(
                    egui::Button::new(
                        egui::RichText::new(text).size(13.0).strong().color(col),
                    )
                    .fill(egui::Color32::TRANSPARENT)
                    .rounding(egui::Rounding::same(5.0)),
                );
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if resp.clicked() {
                    picked = Some(val);
                }
            },
        );
    }
    picked
}

/// A hand-drawn ✕ — the bundled font has no reliable glyph for it.
fn close_button(ui: &mut egui::Ui) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    let col = if resp.hovered() {
        Color32::from_rgb(240, 124, 114)
    } else {
        DIM
    };
    if resp.hovered() {
        ui.painter().rect_filled(rect, Rounding::same(6.0), CARD_HI);
    }
    let c = rect.center();
    let r = 4.5;
    let s = Stroke::new(1.6_f32, col);
    ui.painter()
        .line_segment([c + Vec2::new(-r, -r), c + Vec2::new(r, r)], s);
    ui.painter()
        .line_segment([c + Vec2::new(r, -r), c + Vec2::new(-r, r)], s);
    resp.clicked()
}

/// Open Explorer on the log file (or its folder, when nothing has been logged).
fn reveal_log() {
    let Some(path) = crate::diaglog::log_path() else {
        return;
    };
    let arg = if path.exists() {
        format!("/select,{}", path.display())
    } else {
        path.parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("explorer.exe")
            .raw_arg(&arg)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}

/// Open Explorer on a folder.
fn reveal_path(path: &std::path::Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("explorer.exe")
            .arg(path.as_os_str())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}

/// Keep an error readable on one line.
fn short(e: &str) -> String {
    let mut s: String = e.chars().take(30).collect();
    if e.chars().count() > 30 {
        s.push('…');
    }
    s
}


