// Tokpaek — a movable, translucent tray strip showing your Claude quota windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tokpaek::{app, config, windowing};

use eframe::egui;

#[cfg(windows)]
fn single_instance_guard() -> Option<windows::Win32::Foundation::HANDLE> {
    use windows::core::w;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, WPARAM};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, RegisterWindowMessageW};

    unsafe {
        let handle = CreateMutexW(None, true, w!("Local\\TokpaekSingleInstanceMutex")).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let msg = RegisterWindowMessageW(w!("Tokpaek_ActivateInstance"));
            if msg != 0 {
                let _ = PostMessageW(HWND(0xffff as *mut _), msg, WPARAM(0), LPARAM(0));
            }
            return None;
        }
        Some(handle)
    }
}

fn main() -> eframe::Result<()> {
    #[cfg(windows)]
    let _guard = match single_instance_guard() {
        Some(g) => g,
        None => {
            // Already running: do not spawn a second instance.
            return Ok(());
        }
    };

    let settings = config::Settings::load();

    let size = settings.circle_size as f32;
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([size, size])
        .with_min_inner_size([100.0, 100.0])
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(true)
        .with_taskbar(false);
    if settings.always_on_top {
        viewport = viewport.with_always_on_top();
    }
    if let Some((x, y)) = settings.pos {
        if windowing::visible_on_some_monitor(x, y) {
            viewport = viewport.with_position([x, y]);
        }
    }

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        multisampling: 4,
        ..Default::default()
    };

    eframe::run_native(
        "Tokpaek",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, settings)))),
    )
}
