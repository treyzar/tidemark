#![windows_subsystem = "windows"]

mod active_app;
mod agent;
mod anim;
mod audio;
mod autostart;
mod beat;
mod bb;
mod bb_endpoint;
mod brand;
mod capture;
mod clip_history;
mod clipboard;
mod config;
mod control;
mod dock;
mod fullscreen;
mod http;
mod island;
mod launcher;
mod layout;
mod lyrics;
mod legacy_players;
mod media;
mod notes;
mod overlay;
mod privacy;
mod recognize;
mod record;
mod render;
mod settings;
mod shelf;
mod signals;
mod stats;
mod text_tools;
mod textinput;
mod theme;
mod throne;
mod timer;
mod translate;
mod tray;
mod weather;
mod yandex_music;

use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::*;

fn main() {
    unsafe {
        // One island per user session.
        let _mutex = CreateMutexW(None, true, w!("Local\\WinIsland.Singleton"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // `win-island --settings` asks the running island to open its settings.
            if open_settings_requested() {
                if let Ok(running) = FindWindowW(w!("WinIslandWindow"), None) {
                    let _ = PostMessageW(Some(running), settings::WM_OPEN_SETTINGS, WPARAM(0), LPARAM(0));
                }
            }
            if std::env::args().any(|a| a == "--bb") {
                if let Ok(running) = FindWindowW(w!("WinIslandWindow"), None) {
                    let _ = PostMessageW(Some(running), bb::WM_OPEN, WPARAM(0), LPARAM(0));
                }
            }
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // Shell icons, the file picker, WIC and drag-and-drop need OLE
        // (an STA) on the UI thread.
        let _ = windows::Win32::System::Ole::OleInitialize(None);
    }

    autostart::refresh();
    let cfg = config::Config::load();
    let Ok(hwnd) = island::create(cfg) else { return };
    bb::start(hwnd);
    if std::env::args().any(|a| a == "--bb") { bb::open(hwnd); }
    if open_settings_requested() {
        settings::open(hwnd);
    }
    run_loop();
}

fn open_settings_requested() -> bool {
    std::env::args().any(|a| a == "--settings")
}

/// Sleeps in `GetMessageW` while the island is still; while it animates,
/// drains messages and draws one frame per vsync (Present blocks on it).
fn run_loop() {
    let mut msg = MSG::default();
    unsafe {
        loop {
            island::poll_completed_actions();
            if island::is_animating() {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        return;
                    }
                    if settings::is_dialog_message(&msg) {
                        continue;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                island::frame();
            } else {
                if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    return;
                }
                if settings::is_dialog_message(&msg) {
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
