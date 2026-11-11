#![windows_subsystem = "windows"]

mod active_app;
mod agent;
mod anim;
mod audio;
mod autostart;
mod beat;
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
mod render;
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

use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::*;

fn main() {
    unsafe {
        // One island per user session.
        let _mutex = CreateMutexW(None, true, w!("Local\\WinIsland.Singleton"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // Shell icons, the file picker, WIC and drag-and-drop need OLE
        // (an STA) on the UI thread.
        let _ = windows::Win32::System::Ole::OleInitialize(None);
    }

    let cfg = config::Config::load();
    if island::create(cfg).is_err() {
        return;
    }
    run_loop();
}

/// Sleeps in `GetMessageW` while the island is still; while it animates,
/// drains messages and draws one frame per vsync (Present blocks on it).
fn run_loop() {
    let mut msg = MSG::default();
    unsafe {
        loop {
            if island::is_animating() {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        return;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                island::frame();
            } else {
                if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
