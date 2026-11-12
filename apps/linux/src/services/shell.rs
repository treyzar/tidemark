//! Startup handshake with the GNOME Shell adapter.
//!
//! Before GTK registers its GApplication name, a normally launched process
//! asks `org.tidemark.Shell1.RequestLaunch`. If the extension accepts, it
//! spawns an authenticated client (with `TIDEMARK_SHELL_ADAPTER=1`) and this
//! process exits, so the long-lived instance is the trusted one. When the
//! adapter or the extension is absent, the call fails quietly and the app
//! starts exactly as before.

pub const BUS_NAME: &str = "org.tidemark.Shell";
pub const OBJECT_PATH: &str = "/org/tidemark/Shell";
pub const INTERFACE: &str = "org.tidemark.Shell1";
pub const ENV_MARKER: &str = "TIDEMARK_SHELL_ADAPTER";

use tidemark_core::shell::Capability;

pub fn launched_by_adapter() -> bool {
    std::env::var(ENV_MARKER).ok().as_deref() == Some("1")
}

/// What the pre-GApplication launch probe decided. Only `Launched` makes the
/// caller exit; every other variant registers its own GApplication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchDecision {
    /// The adapter spawned the trusted client; this process exits 0.
    Launched,
    /// A trusted client is running; GApplication forwards to it.
    Forward,
    /// The adapter exists but cannot take over (blocked/failed/cooldown).
    Unintegrated,
    /// No adapter on the bus: normal, unintegrated start.
    NoAdapter,
}

/// `--settings` is the only argument the adapter accepts; everything else is
/// dropped here as well as on the extension side.
pub fn preflight(args: &[String]) -> LaunchDecision {
    let allowed: Vec<String> = args
        .iter()
        .filter(|arg| arg.as_str() == "--settings")
        .cloned()
        .collect();
    let Ok(connection) = zbus::blocking::Connection::session() else {
        return LaunchDecision::NoAdapter;
    };
    let Ok(proxy) = zbus::blocking::Proxy::new(
        &connection,
        BUS_NAME,
        OBJECT_PATH,
        INTERFACE,
    ) else {
        return LaunchDecision::NoAdapter;
    };
    match proxy.call::<_, _, bool>("RequestLaunch", &(allowed,)) {
        Ok(true) => LaunchDecision::Launched,
        Ok(false) => match client_state(&proxy) {
            Some(state) if state == "running" || state == "starting" => LaunchDecision::Forward,
            Some(_) => LaunchDecision::Unintegrated,
            None => LaunchDecision::NoAdapter,
        },
        Err(_) => {
            if proxy.get_property::<u32>("Version").is_ok() {
                LaunchDecision::Unintegrated
            } else {
                LaunchDecision::NoAdapter
            }
        }
    }
}

fn client_state(proxy: &zbus::blocking::Proxy<'_>) -> Option<String> {
    proxy.get_property::<String>("ClientState").ok()
}

/// Requests a handover from an untrusted primary. Only call after saving all
/// state and obtaining the user's explicit consent.
#[allow(dead_code)]
pub fn request_handover(args: &[String]) -> bool {
    let allowed: Vec<String> = args
        .iter()
        .filter(|arg| arg.as_str() == "--settings")
        .cloned()
        .collect();
    let Ok(connection) = zbus::blocking::Connection::session() else {
        return false;
    };
    let Ok(proxy) = zbus::blocking::Proxy::new(
        &connection,
        BUS_NAME,
        OBJECT_PATH,
        INTERFACE,
    ) else {
        return false;
    };
    proxy
        .call::<_, _, bool>("RequestHandover", &(allowed,))
        .unwrap_or(false)
}

#[allow(dead_code)]
pub fn observe_client_state() -> Option<String> {
    let connection = zbus::blocking::Connection::session().ok()?;
    let proxy = zbus::blocking::Proxy::new(&connection, BUS_NAME, OBJECT_PATH, INTERFACE).ok()?;
    client_state(&proxy)
}

/// `Hello` marks this process as the authenticated client and returns the
/// adapter's contract version and capabilities. Kept for the bridge task; the
/// current UI does not call GNOME-only methods yet.
#[allow(dead_code)]
pub fn hello() -> Result<(u32, Vec<Capability>), String> {
    let connection = zbus::blocking::Connection::session()
        .map_err(|error| format!("session bus unavailable: {error}"))?;
    let proxy = zbus::blocking::Proxy::new(&connection, BUS_NAME, OBJECT_PATH, INTERFACE)
        .map_err(|error| format!("proxy: {error}"))?;
    let (version, names): (u32, Vec<String>) = proxy
        .call("Hello", &())
        .map_err(|error| format!("Hello: {error}"))?;
    let capabilities = names
        .iter()
        .filter_map(|name| capability_from_wire(name))
        .collect();
    Ok((version, capabilities))
}

fn capability_from_wire(name: &str) -> Option<Capability> {
    Some(match name {
        "place_windows" => Capability::PlaceWindows,
        "input_region" => Capability::InputRegion,
        "focus_tracking" => Capability::FocusTracking,
        "window_actions" => Capability::WindowActions,
        "fullscreen_tracking" => Capability::FullscreenTracking,
        "maximized_tracking" => Capability::MaximizedTracking,
        "clipboard_read" => Capability::ClipboardRead,
        "clipboard_write" => Capability::ClipboardWrite,
        "screenshot" => Capability::Screenshot,
        "capture_hiding" => Capability::CaptureHiding,
        "global_shortcuts" => Capability::GlobalShortcuts,
        "keyboard_layout" => Capability::KeyboardLayout,
        "caps_lock" => Capability::CapsLock,
        "brightness" => Capability::Brightness,
        _ => return None,
    })
}
