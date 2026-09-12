//! The gauge's face: what the widget shows and how it is drawn. Reset-time
//! formatting (with RU pluralization), arc selection, Oklch palette math and
//! the annular mesh live here — app.rs keeps the frame loop and window state.
//!
//! The public surface is one verb: `draw(ui, settings, state)`.

use crate::config::Settings;
use crate::i18n::Language;
use crate::providers::{Limit, LimitKind, LimitPool, Snapshot};

use chrono::{DateTime, Duration, Local, Utc};
use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

/// What the gauge needs to know about the family it is drawing.
pub struct ActiveState {
    pub online: bool,
    /// Values on screen are the last good ones; the service is throttling us.
    pub stale: bool,
    pub last: Option<Snapshot>,
}

fn minutes_word_ru(mins: i64) -> &'static str {
    let m10 = mins % 10;
    let m100 = mins % 100;
    if (11..=19).contains(&m100) {
        "минут"
    } else if m10 == 1 {
        "минуту"
    } else if (2..=4).contains(&m10) {
        "минуты"
    } else {
        "минут"
    }
}

fn hours_word_ru(hours: i64) -> &'static str {
    let h10 = hours % 10;
    let h100 = hours % 100;
    if (11..=19).contains(&h100) {
        "часов"
    } else if h10 == 1 {
        "час"
    } else if (2..=4).contains(&h10) {
        "часа"
    } else {
        "часов"
    }
}

fn day_of_week_ru(weekday: chrono::Weekday) -> &'static str {
    match weekday {
        chrono::Weekday::Mon => "в понедельник",
        chrono::Weekday::Tue => "во вторник",
        chrono::Weekday::Wed => "в среду",
        chrono::Weekday::Thu => "в четверг",
        chrono::Weekday::Fri => "в пятницу",
        chrono::Weekday::Sat => "в субботу",
        chrono::Weekday::Sun => "в воскресенье",
    }
}

fn day_of_week_en(weekday: chrono::Weekday) -> &'static str {
    match weekday {
        chrono::Weekday::Mon => "on Monday",
        chrono::Weekday::Tue => "on Tuesday",
        chrono::Weekday::Wed => "on Wednesday",
        chrono::Weekday::Thu => "on Thursday",
        chrono::Weekday::Fri => "on Friday",
        chrono::Weekday::Sat => "on Saturday",
        chrono::Weekday::Sun => "on Sunday",
    }
}

/// The three ways one reset time gets written, plus the facts behind them.
/// Typed on purpose: `expired` used to travel as the literal string
/// "обновление…" that callers compared against.
pub struct ResetParts {
    /// The clock time, e.g. "01:16" or "Wed 4:03".
    pub abs: String,
    /// The countdown, e.g. "2h 5m". The circle gauge doesn't draw it; kept
    /// because the tests pin the countdown contract.
    #[allow(dead_code)]
    pub rel: String,
    /// The countdown's leading unit alone; likewise test-only for now.
    #[allow(dead_code)]
    pub tiny: String,
    /// Total minutes until reset.
    pub mins: i64,
    /// The clock ran out; the next poll brings the new window.
    pub expired: bool,
}

/// chrono's `%a` is always English; the Russian sentence must not read
/// "в Wed 22:28".
fn weekday_short_ru(abs: &str) -> String {
    let (head, rest) = abs.split_once(' ').unwrap_or(("", abs));
    let ru = match head {
        "Mon" => "пн",
        "Tue" => "вт",
        "Wed" => "ср",
        "Thu" => "чт",
        "Fri" => "пт",
        "Sat" => "сб",
        "Sun" => "вс",
        _ => head,
    };
    if head.is_empty() {
        abs.to_string()
    } else {
        format!("{ru} {rest}")
    }
}

/// The countdown text without its "Сброс "/"Reset " prefix — the part the
/// context menu and the hover tooltip re-use verbatim, so nothing ever has to
/// strip a formatted sentence back apart.
pub fn reset_body(lang: Language, parts: &ResetParts) -> String {
    if parts.expired {
        return lang.text("обновление…", "updating…").to_string();
    }
    let abs = match lang {
        Language::Russian => weekday_short_ru(&parts.abs),
        Language::English => parts.abs.clone(),
    };
    let abs_clean = abs.strip_prefix('0').unwrap_or(&abs);
    // Beyond a day, raw minutes read as "через 10001 минуту": switch to the
    // day/hour countdown (`rel`), which is also the shortest wording.
    if parts.mins >= 1440 {
        return match lang {
            Language::Russian => format!("в {abs_clean} через {}", parts.rel),
            Language::English => format!("at {abs_clean} in {}", parts.rel),
        };
    }
    match lang {
        Language::Russian => {
            let w = minutes_word_ru(parts.mins);
            format!("в {abs_clean} через {} {w}", parts.mins)
        }
        Language::English => {
            let w = if parts.mins == 1 { "minute" } else { "minutes" };
            format!("at {abs_clean} in {} {w}", parts.mins)
        }
    }
}

pub fn format_reset_time(lang: Language, parts: &ResetParts) -> String {
    let body = reset_body(lang, parts);
    if parts.expired {
        body
    } else {
        match lang {
            Language::Russian => format!("Сброс {body}"),
            Language::English => format!("Reset {body}"),
        }
    }
}

pub fn get_weekly_reset(last: Option<&Snapshot>) -> Option<DateTime<Utc>> {
    let limits = last?.limits.as_slice();
    // 1. Check if any limit has an explicit `weekly.resets_at` (Gemini)
    for l in limits {
        if let Some(w) = &l.weekly {
            if let Some(r) = w.resets_at {
                return Some(r);
            }
        }
    }
    // 2. Check if any limit is typed as weekly (Claude 7-day, ChatGPT weekly, etc.)
    for l in limits {
        if l.kind == LimitKind::Weekly {
            if let Some(win) = &l.window {
                return Some(win.resets_at);
            }
        }
    }
    None
}

/// Which limits fill the gauge's two arcs. The top arc shows the rolling
/// session window (the family's own pool, or Gemini for Antigravity); the
/// bottom arc shows the Claude/GPT pool when the service splits quotas, and
/// the second limit otherwise (Claude's weekly row, Codex's secondary window).
/// Pure: providers typed the limits, the gauge only selects.
pub fn select_arcs(limits: &[Limit]) -> (Option<&Limit>, Option<&Limit>) {
    let top = limits
        .iter()
        .find(|l| {
            l.kind == LimitKind::Session && matches!(l.pool, LimitPool::Own | LimitPool::Gemini)
        })
        .or_else(|| limits.first());
    let bottom = limits
        .iter()
        .find(|l| matches!(l.pool, LimitPool::ClaudeGpt))
        .or_else(|| limits.get(1));
    (top, bottom)
}

/// The weekly countdown without its "Сброс "/"Reset " prefix, so the context
/// menu and the tooltip can re-use it without parsing a formatted sentence.
pub fn weekly_reset_body(
    lang: Language,
    reset: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> String {
    let Some(reset) = reset else {
        return lang.text("нет таймера", "no timer").to_string();
    };

    let rem = reset - now;
    let mins = rem.num_minutes();
    let local = reset.with_timezone(&Local);
    let now_local = now.with_timezone(&Local);

    if mins <= 0 {
        return lang.text("сейчас…", "resetting…").to_string();
    }

    let time_str = local.format("%-H:%M").to_string();

    // 1. If less than 300 minutes (< 5 hours): "в 4:03 через # минут"
    if mins < 300 {
        match lang {
            Language::Russian => {
                let w = minutes_word_ru(mins);
                format!("в {} через {} {}", time_str, mins, w)
            }
            Language::English => {
                let w = if mins == 1 { "minute" } else { "minutes" };
                format!("at {} in {} {}", time_str, mins, w)
            }
        }
    } else {
        use chrono::Datelike;
        let is_same_day = local.date_naive() == now_local.date_naive();
        let total_hours = mins / 60;

        // 2. If it is the reset day (e.g. Wednesday): "в 4:03 через # часов"
        if is_same_day {
            match lang {
                Language::Russian => {
                    let w = hours_word_ru(total_hours);
                    format!("в {} через {} {}", time_str, total_hours, w)
                }
                Language::English => {
                    let w = if total_hours == 1 { "hour" } else { "hours" };
                    format!("at {} in {} {}", time_str, total_hours, w)
                }
            }
        } else {
            // 3. If other day (> 24h): "в среду через # часов"
            match lang {
                Language::Russian => {
                    let day_name = day_of_week_ru(local.weekday());
                    let w = hours_word_ru(total_hours);
                    format!("{} через {} {}", day_name, total_hours, w)
                }
                Language::English => {
                    let day_name = day_of_week_en(local.weekday());
                    let w = if total_hours == 1 { "hour" } else { "hours" };
                    format!("{} in {} {}", day_name, total_hours, w)
                }
            }
        }
    }
}

pub fn format_weekly_reset_time(
    lang: Language,
    reset: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> String {
    let body = weekly_reset_body(lang, reset, now);
    if reset.is_none() || reset.is_some_and(|r| r - now <= Duration::zero()) {
        // "Сброс: нет таймера" / "Сброс сейчас…" keep their own shapes.
        return if reset.is_none() {
            lang.text("Сброс: нет таймера", "Reset: no timer").to_string()
        } else {
            lang.text("Сброс сейчас…", "Reset: resetting…").to_string()
        };
    }
    match lang {
        Language::Russian => format!("Сброс {body}"),
        Language::English => format!("Reset {body}"),
    }
}

/// Shrink `size` until `text` measures no wider than `max_w`, but never below
/// 65% of it — below that the text is unreadable and overflowing is the lesser
/// evil. Measurement uses the painter's own font table, so it matches what
/// will actually be drawn.
pub fn fit_font_size(painter: &egui::Painter, text: &str, size: f32, max_w: f32) -> f32 {
    let floor = size * 0.65;
    let mut s = size;
    while s > floor {
        let w = painter.layout_no_wrap(text.to_string(), FontId::proportional(s), Color32::WHITE)
            .size()
            .x;
        if w <= max_w {
            break;
        }
        s = (s * 0.92).max(floor);
    }
    s
}

pub fn draw(ui: &mut egui::Ui, settings: &Settings, state: ActiveState) {
        let op = settings.opacity;
        let full = ui.max_rect();
        let painter = ui.painter().clone();
        let lang = settings.language;

        // Strictly lock geometry to a 1:1 square centered in the window, preventing any stretching/distortion
        let side = full.width().min(full.height());
        let center = full.center();
        let cx = center.x;
        let cy = center.y;
        let s = side / 200.0;

        // No square card: the backdrop is a disc inscribed in the ring, so
        // nothing shows outside the circle, but the gauge still has a body
        // behind the pill and the percentages.
        let bg_a = (op * 240.0) as u8;
        painter.circle_filled(
            center,
            side / 2.0 - 1.0,
            Color32::from_rgba_unmultiplied(22, 24, 31, bg_a),
        );
        painter.circle_stroke(
            center,
            side / 2.0 - 1.0,
            egui::Stroke::new(
                1.0_f32,
                Color32::from_rgba_unmultiplied(60, 68, 84, (op * 120.0) as u8),
            ),
        );

        let ActiveState {
            online,
            stale,
            last,
        } = state;

        // 1. Resolve quota data:
        // Scale (шкала / дуга) fills by used quota (расход).
        // Text (% в центре) displays remaining quota (остаток).
        let raw_limits = last.as_ref().map(|s| s.limits.as_slice()).unwrap_or(&[]);
        let (top_limit, claude_lim) = select_arcs(raw_limits);
        let top_used_pct = top_limit.map(|l| l.used_percent.clamp(0.0, 100.0)).unwrap_or(0.0);
        let top_rem_pct = (100.0 - top_used_pct).clamp(0.0, 100.0);

        let show_claude = settings.circle_show_claude_gpt;
        let (bot_used_pct, bot_rem_pct, bot_title) = if show_claude {
            let used = claude_lim.map(|l| l.used_percent.clamp(0.0, 100.0)).unwrap_or(0.0);
            let rem = (100.0 - used).clamp(0.0, 100.0);
            (used, rem, "CLAUDE / GPT".to_string())
        } else {
            let (used, rem) = if let Some(w) = top_limit.and_then(|l| l.weekly.as_ref()) {
                let r = w.remaining_percent.clamp(0.0, 100.0);
                ((100.0 - r).clamp(0.0, 100.0), r)
            } else if let Some(l2) = raw_limits.get(1) {
                let u = l2.used_percent.clamp(0.0, 100.0);
                (u, (100.0 - u).clamp(0.0, 100.0))
            } else {
                (0.0, 0.0)
            };
            (used, rem, lang.text("НА СЕМЬ ДНЕЙ", "FOR SEVEN DAYS").to_string())
        };

        // Reset text
        let now = Utc::now();
        let weekly_reset = get_weekly_reset(last.as_ref());
        let reset_5h_opt = top_limit.and_then(|l| l.window).map(|w| fmt_reset(w.resets_at, now));
        let s_weekly = format_weekly_reset_time(lang, weekly_reset, now);
        let s_5h = match &reset_5h_opt {
            Some(res) => format_reset_time(lang, res),
            None => {
                if !online && !stale {
                    lang.text("обновление…", "updating…").to_string()
                } else {
                    lang.text("Сброс неизвестен", "Reset unknown").to_string()
                }
            }
        };

        let reset_str = if settings.circle_show_weekly_reset {
            s_weekly
        } else {
            s_5h
        };

        // 2. Gauge Geometry — ideal circle (Rx = Ry = 80px) with middle pill overlay
        let pill_h = 20.0 * s;
        let pill_w = 160.0 * s;

        let rx_out = 80.0 * s;
        let ry_out = 80.0 * s;
        let thickness = 22.0 * s;
        let rx_in = rx_out - thickness;
        let ry_in = ry_out - thickness;
        let rx_mid = (rx_out + rx_in) / 2.0;
        let ry_mid = (ry_out + ry_in) / 2.0;
        let cap_r = thickness / 2.0;

        let track_color = Color32::from_rgba_unmultiplied(52, 60, 74, (op * 210.0) as u8);
        let top_color = get_circle_color(top_used_pct, settings.circle_palette, (op * 255.0) as u8);
        let bot_color = get_circle_color(bot_used_pct, settings.circle_palette, (op * 255.0) as u8);

        use std::f32::consts::PI;
        let slit_rad = 0.02_f32;
        let n_segs = settings.circle_segments;
        if n_segs == 0 {
            // Smooth continuous arc (Концепт Дуга)
            let a_cap = (cap_r / ry_mid).asin();
            let a_track_min = slit_rad + a_cap;
            let a_track_max = PI - a_track_min;
            // Segment count follows the drawn size, not a constant: the ring
            // stays a perfect circle at any widget scale.
            let steps_track = arc_steps(a_track_max - a_track_min, rx_mid);

            // Top track
            draw_annular_mesh(&painter, cx, cy, rx_in, rx_out, ry_in, ry_out, a_track_min, a_track_max, true, track_color, steps_track);
            painter.circle_filled(Pos2::new(cx + rx_mid * a_track_min.cos(), cy - ry_mid * a_track_min.sin()), cap_r, track_color);
            painter.circle_filled(Pos2::new(cx + rx_mid * a_track_max.cos(), cy - ry_mid * a_track_max.sin()), cap_r, track_color);

            // Top active — filled by used quota (расход)
            if top_used_pct > 0.0 {
                let frac = (top_used_pct / 100.0).clamp(0.0, 1.0);
                let span = a_track_max - a_track_min;
                let a_active_end = a_track_max;
                let a_active_start = a_track_max - frac * span;
                draw_annular_mesh(&painter, cx, cy, rx_in, rx_out, ry_in, ry_out, a_active_start, a_active_end, true, top_color, arc_steps(frac * span, rx_mid));
                painter.circle_filled(Pos2::new(cx + rx_mid * a_active_end.cos(), cy - ry_mid * a_active_end.sin()), cap_r, top_color);
                painter.circle_filled(Pos2::new(cx + rx_mid * a_active_start.cos(), cy - ry_mid * a_active_start.sin()), cap_r, top_color);
            }

            // Bottom track
            draw_annular_mesh(&painter, cx, cy, rx_in, rx_out, ry_in, ry_out, a_track_min, a_track_max, false, track_color, steps_track);
            painter.circle_filled(Pos2::new(cx + rx_mid * a_track_min.cos(), cy + ry_mid * a_track_min.sin()), cap_r, track_color);
            painter.circle_filled(Pos2::new(cx + rx_mid * a_track_max.cos(), cy + ry_mid * a_track_max.sin()), cap_r, track_color);

            // Bottom active — filled by used quota (расход)
            if bot_used_pct > 0.0 {
                let frac = (bot_used_pct / 100.0).clamp(0.0, 1.0);
                let span = a_track_max - a_track_min;
                let a_active_end = a_track_max;
                let a_active_start = a_track_max - frac * span;
                draw_annular_mesh(&painter, cx, cy, rx_in, rx_out, ry_in, ry_out, a_active_start, a_active_end, false, bot_color, arc_steps(frac * span, rx_mid));
                painter.circle_filled(Pos2::new(cx + rx_mid * a_active_end.cos(), cy + ry_mid * a_active_end.sin()), cap_r, bot_color);
                painter.circle_filled(Pos2::new(cx + rx_mid * a_active_start.cos(), cy + ry_mid * a_active_start.sin()), cap_r, bot_color);
            }
        } else {
            // Segmented mode (10, 12, 20)
            let n = n_segs;
            let a_min = slit_rad;
            let a_max = PI - slit_rad;
            let span = a_max - a_min;
            let gap = 0.035 * (12.0 / n as f32);
            // Each cell is smooth on its own: segment count follows the cell's
            // drawn size, so no stair-steps at any widget scale.
            let steps_cell = arc_steps(span / n as f32, rx_mid);

            for i in 0..n {
                let f1 = i as f32 / n as f32;
                let f2 = (i + 1) as f32 / n as f32;
                let seg_center_f = (i as f32 + 0.5) / n as f32;

                let a_start = a_max - f2 * span + gap / 2.0;
                let a_end = a_max - f1 * span - gap / 2.0;

                // Top segment — active by used quota (расход)
                let top_active = seg_center_f <= (top_used_pct / 100.0);
                let col_top = if top_active { top_color } else { track_color };
                draw_annular_mesh(&painter, cx, cy, rx_in, rx_out, ry_in, ry_out, a_start, a_end, true, col_top, steps_cell);

                // Bottom segment — active by used quota (расход)
                let bot_active = seg_center_f <= (bot_used_pct / 100.0);
                let col_bot = if bot_active { bot_color } else { track_color };
                draw_annular_mesh(&painter, cx, cy, rx_in, rx_out, ry_in, ry_out, a_start, a_end, false, col_bot, steps_cell);
            }
        }

        // 3. Middle Pill Overlay — centered at cx, cy, overlaid on top of the circle
        let pill_rect = Rect::from_center_size(
            Pos2::new(cx, cy),
            Vec2::new(pill_w, pill_h),
        );
        let pill_resp = ui.allocate_rect(pill_rect, Sense::hover());
        let pill_hovered = pill_resp.hovered();
        if pill_hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let pill_bg = if pill_hovered {
            Color32::from_rgba_unmultiplied(32, 38, 52, (op * 250.0) as u8)
        } else {
            Color32::from_rgba_unmultiplied(20, 23, 31, (op * 245.0) as u8)
        };
        let pill_stroke_col = if pill_hovered {
            Color32::from_rgba_unmultiplied(90, 140, 220, (op * 240.0) as u8)
        } else {
            Color32::from_rgba_unmultiplied(52, 60, 78, (op * 220.0) as u8)
        };
        painter.rect_filled(
            pill_rect,
            egui::Rounding::same(pill_h / 2.0),
            pill_bg,
        );
        painter.rect_stroke(
            pill_rect,
            egui::Rounding::same(pill_h / 2.0),
            egui::Stroke::new(0.9_f32, pill_stroke_col),
        );
        // The pill is a fixed 160·s wide, but the reset sentence is not: the
        // weekly one reaches "Сброс в понедельник через 168 часов". Measure
        // and shrink the font until it fits (down to 65%), instead of letting
        // long strings hang over the pill's ends.
        let text_color = Color32::from_rgba_unmultiplied(248, 250, 255, (op * 255.0) as u8);
        let font_size = fit_font_size(&painter, &reset_str, 9.5 * s, pill_w - 12.0 * s);
        painter.text(
            pill_rect.center(),
            Align2::CENTER_CENTER,
            reset_str,
            FontId::proportional(font_size),
            text_color,
        );

        // Active timer text highlight colors
        let show_weekly = settings.circle_show_weekly_reset;
        let active_text_color = Color32::from_rgba_unmultiplied(255, 255, 255, (op * 255.0) as u8);
        let muted_text_color = Color32::from_rgba_unmultiplied(135, 145, 165, (op * 220.0) as u8);

        let top_title_color = if !show_weekly { active_text_color } else { muted_text_color };
        let bot_title_color = if show_weekly { active_text_color } else { muted_text_color };

        // 4. Central texts inside Upper Arc — strictly symmetrical
        let top_label_text = format!("{:.0}%", top_rem_pct);
        painter.text(
            Pos2::new(cx, cy - 35.0 * s),
            Align2::CENTER_CENTER,
            top_label_text,
            FontId::proportional(25.0 * s),
            Color32::from_rgba_unmultiplied(255, 255, 255, (op * 255.0) as u8),
        );

        let top_sub_text = lang.text("НА ПЯТЬ ЧАСОВ", "FOR FIVE HOURS");
        painter.text(
            Pos2::new(cx, cy - 17.0 * s),
            Align2::CENTER_CENTER,
            top_sub_text,
            FontId::proportional(9.0 * s),
            top_title_color,
        );

        // 5. Central texts inside Lower Arc — strictly symmetrical
        painter.text(
            Pos2::new(cx, cy + 17.0 * s),
            Align2::CENTER_CENTER,
            bot_title,
            FontId::proportional(9.0 * s),
            bot_title_color,
        );

        let bot_label_text = format!("{:.0}%", bot_rem_pct);
        painter.text(
            Pos2::new(cx, cy + 35.0 * s),
            Align2::CENTER_CENTER,
            bot_label_text,
            FontId::proportional(25.0 * s),
            Color32::from_rgba_unmultiplied(255, 255, 255, (op * 255.0) as u8),
        );
    }
#[allow(clippy::excessive_precision)]
fn oklch_to_color32(percent: f32, a: u8) -> Color32 {
    let f = (percent / 100.0).clamp(0.0, 1.0);
    let (l, c, h_deg) = if f <= 0.40 {
        let t = f / 0.40;
        (
            0.72 * (1.0 - t) + 0.80 * t,
            0.17 * (1.0 - t) + 0.17 * t,
            145.0 * (1.0 - t) + 120.0 * t,
        )
    } else if f <= 0.70 {
        let t = (f - 0.40) / 0.30;
        (
            0.80 * (1.0 - t) + 0.78 * t,
            0.17 * (1.0 - t) + 0.18 * t,
            120.0 * (1.0 - t) + 65.0 * t,
        )
    } else {
        let t = (f - 0.70) / 0.30;
        (
            0.78 * (1.0 - t) + 0.65 * t,
            0.18 * (1.0 - t) + 0.22 * t,
            65.0 * (1.0 - t) + 25.0 * t,
        )
    };

    let h_rad = h_deg.to_radians();
    let a_lab = c * h_rad.cos();
    let b_lab = c * h_rad.sin();

    // Oklab to linear LMS
    let l_prime = l + 0.3963377774 * a_lab + 0.2158037573 * b_lab;
    let m_prime = l - 0.1055613458 * a_lab - 0.0638541728 * b_lab;
    let s_prime = l - 0.0894841775 * a_lab - 1.2914855480 * b_lab;

    let l_lin = l_prime * l_prime * l_prime;
    let m_lin = m_prime * m_prime * m_prime;
    let s_lin = s_prime * s_prime * s_prime;

    // Linear LMS to linear sRGB
    let r_lin = 4.0767416621 * l_lin - 3.3077115913 * m_lin + 0.2309699292 * s_lin;
    let g_lin = -1.2684380046 * l_lin + 2.6097574011 * m_lin - 0.3413193965 * s_lin;
    let b_lin = -0.0041960863 * l_lin - 0.7034186147 * m_lin + 1.7076147010 * s_lin;

    let to_u8 = |v: f32| -> u8 {
        let v_clamped = v.clamp(0.0, 1.0);
        let srgb = if v_clamped <= 0.0031308 {
            12.92 * v_clamped
        } else {
            1.055 * v_clamped.powf(1.0 / 2.4) - 0.055
        };
        (srgb * 255.0).round().clamp(0.0, 255.0) as u8
    };

    Color32::from_rgba_unmultiplied(to_u8(r_lin), to_u8(g_lin), to_u8(b_lin), a)
}

fn get_circle_color(percent: f32, palette: crate::config::CirclePalette, a: u8) -> Color32 {
    match palette {
        crate::config::CirclePalette::Monochrome => Color32::from_rgba_unmultiplied(241, 245, 249, a),
        crate::config::CirclePalette::Cyan => Color32::from_rgba_unmultiplied(45, 212, 191, a),
        crate::config::CirclePalette::Traffic => {
            // Thirds, traditional traffic-light colours.
            if percent <= 33.0 {
                Color32::from_rgba_unmultiplied(0, 168, 80, a) // green
            } else if percent <= 66.0 {
                Color32::from_rgba_unmultiplied(255, 196, 0, a) // yellow
            } else {
                Color32::from_rgba_unmultiplied(224, 36, 36, a) // red
            }
        }
        crate::config::CirclePalette::Gradient => oklch_to_color32(percent, a),
    }
}

/// How many segments an arc of `a_span` radians at radius `r_mid` needs so no
/// corner is visible: roughly one segment per 2 pixels of arc, whatever the
/// widget size. A fixed count made small percentages angular ("вмятины") and
/// left stair-steps ("лесенка") on the ring's edges when scaled up.
fn arc_steps(a_span: f32, r_mid: f32) -> usize {
    ((a_span.abs() * r_mid) / 2.0).ceil().clamp(8.0, 256.0) as usize
}

#[allow(clippy::too_many_arguments)]
fn draw_annular_mesh(
    painter: &egui::Painter,
    cx: f32,
    cy: f32,
    rx_in: f32,
    rx_out: f32,
    ry_in: f32,
    ry_out: f32,
    a1: f32,
    a2: f32,
    is_top: bool,
    color: Color32,
    steps: usize,
) {    if a1 >= a2 || steps == 0 || color.a() == 0 {
        return;
    }
    let mut mesh = egui::Mesh::default();
    let da = (a2 - a1) / steps as f32;
    let mut pts_outer = Vec::with_capacity(steps + 1);
    let mut pts_inner = Vec::with_capacity(steps + 1);

    for i in 0..=steps {
        let angle = a1 + i as f32 * da;
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        let (y_in, y_out) = if is_top {
            (cy - ry_in * sin_a, cy - ry_out * sin_a)
        } else {
            (cy + ry_in * sin_a, cy + ry_out * sin_a)
        };
        let p_in = Pos2::new(cx + rx_in * cos_a, y_in);
        let p_out = Pos2::new(cx + rx_out * cos_a, y_out);
        pts_inner.push(p_in);
        pts_outer.push(p_out);

        let v_in = mesh.vertices.len() as u32;
        mesh.vertices.push(egui::epaint::Vertex {
            pos: p_in,
            uv: Pos2::ZERO,
            color,
        });
        mesh.vertices.push(egui::epaint::Vertex {
            pos: p_out,
            uv: Pos2::ZERO,
            color,
        });

        if i > 0 {
            let prev_in = v_in - 2;
            let cur_in = v_in;
            let cur_out = v_in + 1;
            mesh.add_triangle(prev_in, prev_in + 1, cur_out);
            mesh.add_triangle(prev_in, cur_out, cur_in);
        }
    }
    painter.add(egui::Shape::Mesh(mesh));

    // Antialiased perimeter stroke: eliminates pixel stair-stepping ("лесенка") on mesh boundaries
    let mut perimeter = Vec::with_capacity(steps * 2 + 2);
    perimeter.extend(pts_outer);
    perimeter.extend(pts_inner.into_iter().rev());
    painter.add(egui::Shape::Path(egui::epaint::PathShape::closed_line(
        perimeter,
        Stroke::new(1.0_f32, color),
    )));
}

pub fn fmt_reset(reset: DateTime<Utc>, now: DateTime<Utc>) -> ResetParts {
    let local = reset.with_timezone(&Local);
    let rem = reset - now;
    let abs = if rem > Duration::hours(24) {
        local.format("%a %-H:%M").to_string()
    } else {
        local.format("%-H:%M").to_string()
    };
    let mins = rem.num_minutes().max(0);
    let expired = rem <= Duration::zero();
    let (rel, tiny) = if mins >= 1440 {
        (
            format!("{}d {}h", mins / 1440, (mins % 1440) / 60),
            format!("{}d", mins / 1440),
        )
    } else if mins >= 60 {
        (
            format!("{}h {}m", mins / 60, mins % 60),
            format!("{}h", mins / 60),
        )
    } else if !expired {
        // "0m" read as "already reset" while the quota was still counting.
        let s = if mins == 0 {
            "<1m".to_string()
        } else {
            format!("{mins}m")
        };
        (s.clone(), s)
    } else {
        // The clock ran out; the next poll brings the new window.
        (String::new(), "…".to_string())
    };
    ResetParts {
        abs,
        rel,
        tiny,
        mins,
        expired,
    }
}

#[cfg(test)]
mod tests {
    use super::{fmt_reset, format_reset_time, format_weekly_reset_time};
    use crate::i18n::Language;
    use chrono::{Duration, Utc};

    /// Exhaustive sweep of every reset sentence the pill can be asked to show:
    /// both formatters × both languages × the whole timer range (0 minutes to
    /// 8 days, stepping 1 minute), at a fixed `now` with an awkward local time
    /// (two-digit hour). The pill fits ~31 characters at the full 9.5·s font
    /// and shrinks to 65% (~48 chars) before overflowing; nothing may exceed
    /// the shrink capacity, and the sweep reports the longest string per
    /// combination so a future wording change trips this test.
    #[test]
    fn no_reset_string_outgrows_the_pill() {
        // A Wednesday at 23:47 local-ish: forces two-digit hours, the longest
        // weekday names and the "other day" weekly branch.
        let now = Utc.with_ymd_and_hms(2026, 9, 9, 20, 47, 0).unwrap();

        use chrono::TimeZone;
        let mut worst = [String::new(), String::new(), String::new(), String::new()];
        let mut worst_len = [0usize; 4];
        for min in 0..=(8 * 24 * 60) {
            let reset = now + Duration::minutes(min);
            let parts = fmt_reset(reset, now);
            for (lang, base) in [(Language::Russian, 0usize), (Language::English, 2)] {
                let s5 = format_reset_time(lang, &parts);
                let sw = format_weekly_reset_time(lang, Some(reset), now);
                for (i, s) in [(base, &s5), (base + 1, &sw)] {
                    let n = s.chars().count();
                    if n > worst_len[i] {
                        worst_len[i] = n;
                        worst[i] = s.clone();
                    }
                }
            }
        }

        // The pill at 65% of 9.5·s fits roughly 48 characters of Segoe UI.
        const CAP: usize = 48;
        for (i, label) in ["5h RU", "weekly RU", "5h EN", "weekly EN"].iter().enumerate() {
            assert!(
                worst_len[i] <= CAP,
                "{label}: longest string is {} chars (> {CAP}): {:?}",
                worst_len[i],
                worst[i]
            );
        }
        // Sanity: the sweep actually exercised the long branches, not empties.
        assert!(worst_len[1] >= 25, "weekly RU never got long: {:?}", worst[1]);
        assert!(worst_len[3] >= 25, "weekly EN never got long: {:?}", worst[3]);

        for (label, (s, n)) in ["5h RU", "weekly RU", "5h EN", "weekly EN"]
            .iter()
            .zip(worst.iter().zip(worst_len))
        {
            println!("{label:>10}: {n:>3} chars | {s:?}");
        }
    }

    /// The window is still counting until its reset actually passes: rounding
    /// the last seconds down to "0m" read as "already reset".
    #[test]
    fn the_last_minute_is_not_zero_minutes() {
        let now = Utc::now();
        let rel = |secs: i64| fmt_reset(now + Duration::seconds(secs), now).rel;

        assert_eq!(rel(40), "<1m", "under a minute still has time left");
        assert_eq!(rel(95), "1m");
        assert_eq!(rel(2 * 3600 + 5 * 60), "2h 5m");
        assert_eq!(rel(25 * 3600), "1d 1h");
        assert!(fmt_reset(now, now).expired, "the clock ran out, wait for a poll");
        assert!(fmt_reset(now - Duration::seconds(30), now).expired);
    }

    /// The small designs have room for the leading unit and nothing else, so
    /// the countdown must survive being cut down to it.
    #[test]
    fn the_tiny_countdown_keeps_the_leading_unit() {
        let now = Utc::now();
        let tiny = |secs: i64| fmt_reset(now + Duration::seconds(secs), now).tiny;

        assert_eq!(tiny(40), "<1m");
        assert_eq!(tiny(95), "1m");
        assert_eq!(tiny(2 * 3600 + 5 * 60), "2h");
        assert_eq!(tiny(25 * 3600), "1d");
        assert_eq!(tiny(-30), "…", "no room for a word on a nano row");
    }

    #[test]
    fn test_format_reset_time() {
        use super::ResetParts;
        use crate::i18n::Language;

        let parts = |abs: &str, rel: &str, tiny: &str, mins: i64, expired: bool| ResetParts {
            abs: abs.into(),
            rel: rel.into(),
            tiny: tiny.into(),
            mins,
            expired,
        };

        let res = parts("01:16", "2h 0m", "2h", 120, false);
        assert_eq!(format_reset_time(Language::Russian, &res), "Сброс в 1:16 через 120 минут");
        assert_eq!(format_reset_time(Language::English, &res), "Reset at 1:16 in 120 minutes");

        let res_1 = parts("01:16", "1m", "1m", 1, false);
        assert_eq!(format_reset_time(Language::Russian, &res_1), "Сброс в 1:16 через 1 минуту");
        assert_eq!(format_reset_time(Language::English, &res_1), "Reset at 1:16 in 1 minute");

        let res_2 = parts("01:16", "2m", "2m", 2, false);
        assert_eq!(format_reset_time(Language::Russian, &res_2), "Сброс в 1:16 через 2 минуты");

        let res_expired = parts("01:16", "", "…", 0, true);
        assert_eq!(format_reset_time(Language::Russian, &res_expired), "обновление…");
        assert_eq!(format_reset_time(Language::English, &res_expired), "updating…");
    }

    #[test]
    fn arcs_are_selected_by_kind_and_pool_not_by_title() {
        use super::select_arcs;
        use crate::providers::{Limit, LimitKind, LimitPool};

        let lim = |title: &str, kind: LimitKind, pool: LimitPool| Limit {
            title: title.into(),
            kind,
            pool,
            used_percent: 42.0,
            window: None,
            weekly: None,
        };

        // Claude: session first, weekly second — titles say nothing now.
        let claude = vec![
            lim("anything", LimitKind::Session, LimitPool::Own),
            lim("something else", LimitKind::Weekly, LimitPool::Own),
        ];
        let (top, bottom) = select_arcs(&claude);
        assert_eq!(top.unwrap().title, "anything");
        assert_eq!(bottom.unwrap().title, "something else");

        // Antigravity: top arc is Gemini, bottom is the Claude/GPT pool.
        let anti = vec![
            lim("Gemini", LimitKind::Session, LimitPool::Gemini),
            lim("Claude / GPT", LimitKind::Session, LimitPool::ClaudeGpt),
        ];
        let (top, bottom) = select_arcs(&anti);
        assert_eq!(top.unwrap().pool, LimitPool::Gemini);
        assert_eq!(bottom.unwrap().pool, LimitPool::ClaudeGpt);

        // Order does not matter: the Claude/GPT pool still fills the bottom.
        let reordered = vec![anti[1].clone(), anti[0].clone()];
        let (top, bottom) = select_arcs(&reordered);
        assert_eq!(top.unwrap().pool, LimitPool::Gemini);
        assert_eq!(bottom.unwrap().pool, LimitPool::ClaudeGpt);

        // Empty stays empty.
        let (top, bottom) = select_arcs(&[]);
        assert!(top.is_none() && bottom.is_none());
    }

    #[test]
    fn weekly_reset_comes_from_typed_limits() {
        use super::get_weekly_reset;
        use crate::providers::{Family, Limit, LimitKind, LimitPool, LimitWindow, Snapshot, WeeklyQuota};
        let now = Utc::now();
        let reset = now + Duration::hours(30);

        let lim = |kind: LimitKind, window: Option<LimitWindow>, weekly: Option<WeeklyQuota>| Limit {
            title: "no title sniffing".into(),
            kind,
            pool: LimitPool::Own,
            used_percent: 10.0,
            window,
            weekly,
        };

        // An explicitly weekly window wins.
        let snap = Snapshot {
            family: Family::Claude,
            limits: vec![
                lim(LimitKind::Session, Some(LimitWindow::ending_at(now + Duration::hours(2), Duration::hours(5), now)), None),
                lim(LimitKind::Weekly, Some(LimitWindow::ending_at(reset, Duration::days(7), now)), None),
            ],
        };
        assert_eq!(get_weekly_reset(Some(&snap)), Some(reset));

        // An Antigravity-style weekly badge is found too.
        let snap_badge = Snapshot {
            family: Family::Antigravity,
            limits: vec![lim(
                LimitKind::Session,
                None,
                Some(WeeklyQuota { remaining_percent: 12.0, resets_at: Some(reset) }),
            )],
        };
        assert_eq!(get_weekly_reset(Some(&snap_badge)), Some(reset));

        // A session-only snapshot has no weekly timer.
        let snap_none = Snapshot {
            family: Family::Codex,
            limits: vec![lim(LimitKind::Session, Some(LimitWindow::ending_at(reset, Duration::hours(5), now)), None)],
        };
        assert_eq!(get_weekly_reset(Some(&snap_none)), None);
        assert_eq!(get_weekly_reset(None), None);
    }

    #[test]
    fn test_format_weekly_reset_time() {
        use super::format_weekly_reset_time;
        use crate::i18n::Language;
        let now = chrono::Utc::now();
        // 1. None
        assert_eq!(format_weekly_reset_time(Language::Russian, None, now), "Сброс: нет таймера");
        assert_eq!(format_weekly_reset_time(Language::English, None, now), "Reset: no timer");

        // 2. 76 hours in future (> 24 hours -> day of week + hours)
        let future = now + chrono::Duration::hours(76);
        let ru = format_weekly_reset_time(Language::Russian, Some(future), now);
        assert!(ru.starts_with("Сброс "));
        assert!(ru.contains("через 76 часов"));
        assert!(!ru.contains('('));
        assert!(!ru.contains(')'));

        let en = format_weekly_reset_time(Language::English, Some(future), now);
        assert!(en.starts_with("Reset "));
        assert!(en.contains("in 76 hours"));
        assert!(!en.contains('('));
        assert!(!en.contains(')'));

        // 3. 25 minutes (< 300 minutes -> time + minutes)
        let soon = now + chrono::Duration::minutes(25);
        let ru_soon = format_weekly_reset_time(Language::Russian, Some(soon), now);
        assert!(ru_soon.starts_with("Сброс в "));
        assert!(ru_soon.contains("через 25 минут"));
        assert!(!ru_soon.contains('('));
    }
}

#[cfg(test)]
mod arc_smoothness_tests {
    use super::arc_steps;
    use std::f32::consts::PI;

    /// One segment per ~2 px of arc: a big widget needs many segments, a small
    /// one few — but never fewer than 8 (angular "dents") or more than 256
    /// (pointless geometry).
    #[test]
    fn segment_count_follows_the_drawn_size() {
        // Default widget: half-circle at r=69 px → ~108 segments.
        let steps = arc_steps(PI - 0.04, 69.0);
        assert!((100..=120).contains(&steps), "{steps}");

        // A tiny active arc (2%) on a big widget is still smooth.
        let steps = arc_steps(0.02 * PI, 180.0);
        assert!((8..=256).contains(&steps), "{steps}");

        // Clamps at both ends.
        assert_eq!(arc_steps(0.001, 1.0), 8);
        assert_eq!(arc_steps(PI, 10_000.0), 256);
    }
}

#[cfg(test)]
mod circle_palette_tests {
    use super::*;
    use crate::config::CirclePalette;

    #[test]
    fn test_traffic_palette_thresholds() {
        // Thirds with traditional traffic-light colours.
        let green = get_circle_color(20.0, CirclePalette::Traffic, 255);
        assert_eq!(green, Color32::from_rgba_unmultiplied(0, 168, 80, 255));

        let green33 = get_circle_color(33.0, CirclePalette::Traffic, 255);
        assert_eq!(green33, Color32::from_rgba_unmultiplied(0, 168, 80, 255));

        let yellow = get_circle_color(34.0, CirclePalette::Traffic, 255);
        assert_eq!(yellow, Color32::from_rgba_unmultiplied(255, 196, 0, 255));

        let yellow66 = get_circle_color(66.0, CirclePalette::Traffic, 255);
        assert_eq!(yellow66, Color32::from_rgba_unmultiplied(255, 196, 0, 255));

        let red = get_circle_color(67.0, CirclePalette::Traffic, 255);
        assert_eq!(red, Color32::from_rgba_unmultiplied(224, 36, 36, 255));

        let red100 = get_circle_color(100.0, CirclePalette::Traffic, 255);
        assert_eq!(red100, Color32::from_rgba_unmultiplied(224, 36, 36, 255));
    }

    /// The gradient must walk green → yellow → red over the 40/30/30 thirds,
    /// monotonically: hue drops with usage at every step, and each third's
    /// middle is dominated by the expected channel.
    #[test]
    fn test_gradient_oklch_endpoints() {
        let start = get_circle_color(0.0, CirclePalette::Gradient, 255);
        assert!(start.g() > start.r() && start.g() > start.b(), "green dominant at 0%: {:?}", start);

        let at35 = get_circle_color(35.0, CirclePalette::Gradient, 255);
        assert!(at35.g() > at35.r(), "green dominant at 35%: {:?}", at35);

        let mid = get_circle_color(55.0, CirclePalette::Gradient, 255);
        assert!(mid.r() > 180 && mid.g() > 170 && mid.b() < 130, "yellow/amber dominant at 55%: {:?}", mid);

        let end = get_circle_color(100.0, CirclePalette::Gradient, 255);
        assert!(end.r() > end.g() && end.r() > end.b(), "red dominant at 100%: {:?}", end);

        // Monotone: from green (high g−r) to red (high r−g) with no reversal.
        let warmth = |p: f32| {
            let c = get_circle_color(p, CirclePalette::Gradient, 255);
            c.r() as i32 - c.g() as i32
        };
        let mut prev = warmth(0.0);
        for p in (1..=100).step_by(1) {
            let w = warmth(p as f32);
            assert!(w >= prev - 2, "gradient reverses at {p}%: {prev} -> {w}");
            prev = w;
        }
    }

    #[test]
    fn test_cyan_and_monochrome_palettes() {
        let cyan = get_circle_color(50.0, CirclePalette::Cyan, 255);
        assert_eq!(cyan, Color32::from_rgba_unmultiplied(45, 212, 191, 255));

        let mono = get_circle_color(50.0, CirclePalette::Monochrome, 255);
        assert_eq!(mono, Color32::from_rgba_unmultiplied(241, 245, 249, 255));
    }
}


