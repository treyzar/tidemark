//! Contract between the Linux app and whatever controls the desktop for it:
//! the GNOME Shell adapter (`org.tidemark.Shell1` over D-Bus), an Xorg
//! backend, or nothing (`NullBackend`). See `docs/ubuntu/architecture.md` §4.3.
//!
//! The JSON forms below are the wire names. The adapter is written in GJS and
//! matches them by string, so renaming a variant is a breaking change: the
//! `wire_names_are_stable` test pins every one. Binary payloads (clipboard,
//! screenshots) travel as file descriptors in the D-Bus transport. In this
//! trait they are plain bytes.

use serde::{Deserialize, Serialize};

/// Contract version. Bump on any incompatible change to the types below.
pub const SHELL_CONTRACT_VERSION: u32 = 1;

/// What a backend can do. The app hides or explains features whose
/// capability is missing instead of failing silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Keep-above, sticky, hidden from task lists, no focus on map, placement.
    PlaceWindows,
    /// Click-through outside the island shape.
    InputRegion,
    FocusTracking,
    WindowActions,
    FullscreenTracking,
    MaximizedTracking,
    ClipboardRead,
    ClipboardWrite,
    Screenshot,
    CaptureHiding,
    GlobalShortcuts,
    KeyboardLayout,
    CapsLock,
    Brightness,
}

impl Capability {
    pub const ALL: [Capability; 14] = [
        Capability::PlaceWindows,
        Capability::InputRegion,
        Capability::FocusTracking,
        Capability::WindowActions,
        Capability::FullscreenTracking,
        Capability::MaximizedTracking,
        Capability::ClipboardRead,
        Capability::ClipboardWrite,
        Capability::Screenshot,
        Capability::CaptureHiding,
        Capability::GlobalShortcuts,
        Capability::KeyboardLayout,
        Capability::CapsLock,
        Capability::Brightness,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowRole {
    Island,
    Overlay,
    Settings,
}

/// Logical (layout) pixels in the global compositor space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// The window the user is working in (the Window tile, app-switch notices).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FocusInfo {
    /// Backend-specific stable id for `window_action`.
    pub id: u64,
    /// Desktop app id ("org.gnome.Nautilus"), empty if unknown.
    pub app_id: String,
    pub title: String,
    pub pid: Option<u32>,
    /// Icon name from the icon theme, or an absolute file path.
    pub icon: Option<String>,
    pub maximized: bool,
    pub above: bool,
    /// 0..=255.
    pub opacity: u8,
    pub rect: Rect,
    pub monitor: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", content = "arg", rename_all = "snake_case")]
pub enum WindowAction {
    Activate,
    Minimize,
    ToggleMaximize,
    SetAbove(bool),
    SetOpacity(u8),
    TileLeft,
    TileRight,
}

/// The state the compositor reports after an action (never assumed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowState {
    pub maximized: bool,
    pub above: bool,
    pub opacity: u8,
    pub minimized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutId {
    /// Search (Alt+Space on Windows).
    Launcher,
    /// Light/dark theme (Ctrl+Alt+T on Windows).
    ToggleTheme,
    /// Yandex Music like (Ctrl+Alt+L).
    LikeTrack,
}

/// `accel` uses GTK accelerator syntax, e.g. "<Alt>space", "<Control><Alt>l".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutBinding {
    pub id: ShortcutId,
    pub accel: String,
}

/// `bound == false` means the accelerator is taken (by GNOME or another
/// app); the backend never steals an existing binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindResult {
    pub id: ShortcutId,
    pub bound: bool,
}

/// What was just copied. Content is fetched separately with `get_clipboard`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardOffer {
    pub mimes: Vec<String>,
    /// Marked secret by a password manager (`x-kde-passwordManagerHint`):
    /// must not be recorded.
    pub sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardData {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Pushed by the backend; the D-Bus signal names are the PascalCase forms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "signal", rename_all = "snake_case")]
pub enum ShellSignal {
    FocusChanged { focus: Option<FocusInfo> },
    FullscreenChanged { monitor: u32, fullscreen: bool },
    MaximizedTopChanged { covered: bool },
    ShortcutActivated { id: ShortcutId },
    ClipboardChanged { offer: ClipboardOffer },
    /// Short layout code for the OSD, e.g. "RU", "EN".
    LayoutChanged { layout: String },
    CapsLock { on: bool },
    /// 0.0..=1.0.
    Brightness { level: f32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "error", content = "detail", rename_all = "snake_case")]
pub enum ShellError {
    Unsupported(Capability),
    /// The backend tried and the desktop refused or failed; text for the UI.
    Failed(String),
}

impl std::fmt::Display for ShellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellError::Unsupported(c) => write!(f, "not supported by this desktop: {c:?}"),
            ShellError::Failed(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for ShellError {}

pub type ShellResult<T> = Result<T, ShellError>;

/// Implemented by the GNOME adapter client, the Xorg backend and
/// `NullBackend`. Calls may block briefly (D-Bus round trip); signals are
/// delivered separately as `Event::Shell`.
pub trait ShellBackend: Send {
    fn capabilities(&self) -> Vec<Capability>;
    fn place(&self, role: WindowRole, rect: Rect, input: &[Rect]) -> ShellResult<()>;
    fn focus(&self) -> ShellResult<Option<FocusInfo>>;
    fn window_action(&self, id: u64, action: WindowAction) -> ShellResult<WindowState>;
    fn get_clipboard(&self, mime: &str) -> ShellResult<ClipboardData>;
    fn set_clipboard(&self, data: &ClipboardData) -> ShellResult<()>;
    /// PNG of `area` (or every monitor), with the island hidden.
    fn screenshot(&self, area: Option<Rect>, include_cursor: bool) -> ShellResult<Vec<u8>>;
    fn set_capture_hidden(&self, hidden: bool) -> ShellResult<()>;
    fn bind_shortcuts(&self, bindings: &[ShortcutBinding]) -> ShellResult<Vec<BindResult>>;

    fn supports(&self, capability: Capability) -> bool {
        self.capabilities().contains(&capability)
    }
}

/// No desktop integration: every call reports what is missing.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullBackend;

impl ShellBackend for NullBackend {
    fn capabilities(&self) -> Vec<Capability> {
        Vec::new()
    }
    fn place(&self, _: WindowRole, _: Rect, _: &[Rect]) -> ShellResult<()> {
        Err(ShellError::Unsupported(Capability::PlaceWindows))
    }
    fn focus(&self) -> ShellResult<Option<FocusInfo>> {
        Err(ShellError::Unsupported(Capability::FocusTracking))
    }
    fn window_action(&self, _: u64, _: WindowAction) -> ShellResult<WindowState> {
        Err(ShellError::Unsupported(Capability::WindowActions))
    }
    fn get_clipboard(&self, _: &str) -> ShellResult<ClipboardData> {
        Err(ShellError::Unsupported(Capability::ClipboardRead))
    }
    fn set_clipboard(&self, _: &ClipboardData) -> ShellResult<()> {
        Err(ShellError::Unsupported(Capability::ClipboardWrite))
    }
    fn screenshot(&self, _: Option<Rect>, _: bool) -> ShellResult<Vec<u8>> {
        Err(ShellError::Unsupported(Capability::Screenshot))
    }
    fn set_capture_hidden(&self, _: bool) -> ShellResult<()> {
        Err(ShellError::Unsupported(Capability::CaptureHiding))
    }
    fn bind_shortcuts(&self, _: &[ShortcutBinding]) -> ShellResult<Vec<BindResult>> {
        Err(ShellError::Unsupported(Capability::GlobalShortcuts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn wire_names_are_stable() {
        let caps: Vec<_> = Capability::ALL.iter().map(|c| to_value(c).unwrap()).collect();
        assert_eq!(
            serde_json::Value::Array(caps),
            json!([
                "place_windows", "input_region", "focus_tracking", "window_actions",
                "fullscreen_tracking", "maximized_tracking", "clipboard_read", "clipboard_write",
                "screenshot", "capture_hiding", "global_shortcuts", "keyboard_layout", "caps_lock", "brightness"
            ])
        );
        assert_eq!(to_value(WindowRole::Island).unwrap(), json!("island"));
        assert_eq!(to_value(WindowAction::Activate).unwrap(), json!({"action": "activate"}));
        assert_eq!(to_value(WindowAction::SetOpacity(217)).unwrap(), json!({"action": "set_opacity", "arg": 217}));
        assert_eq!(to_value(WindowAction::SetAbove(true)).unwrap(), json!({"action": "set_above", "arg": true}));
        assert_eq!(to_value(WindowAction::TileRight).unwrap(), json!({"action": "tile_right"}));
        assert_eq!(to_value(ShortcutId::ToggleTheme).unwrap(), json!("toggle_theme"));
        assert_eq!(
            to_value(ShellSignal::FullscreenChanged { monitor: 1, fullscreen: true }).unwrap(),
            json!({"signal": "fullscreen_changed", "monitor": 1, "fullscreen": true})
        );
        assert_eq!(
            to_value(ShellSignal::ShortcutActivated { id: ShortcutId::Launcher }).unwrap(),
            json!({"signal": "shortcut_activated", "id": "launcher"})
        );
        assert_eq!(
            to_value(ShellError::Unsupported(Capability::Screenshot)).unwrap(),
            json!({"error": "unsupported", "detail": "screenshot"})
        );
        assert_eq!(SHELL_CONTRACT_VERSION, 1);
    }

    #[test]
    fn every_signal_round_trips() {
        let focus = FocusInfo {
            id: 42,
            app_id: "org.gnome.Nautilus".into(),
            title: "Документы".into(),
            pid: Some(1234),
            icon: Some("org.gnome.Nautilus".into()),
            maximized: false,
            above: true,
            opacity: 255,
            rect: Rect { x: -10, y: 0, w: 800, h: 600 },
            monitor: 0,
        };
        let all = vec![
            ShellSignal::FocusChanged { focus: Some(focus) },
            ShellSignal::FocusChanged { focus: None },
            ShellSignal::FullscreenChanged { monitor: 0, fullscreen: false },
            ShellSignal::MaximizedTopChanged { covered: true },
            ShellSignal::ShortcutActivated { id: ShortcutId::LikeTrack },
            ShellSignal::ClipboardChanged {
                offer: ClipboardOffer { mimes: vec!["text/plain;charset=utf-8".into()], sensitive: true },
            },
            ShellSignal::LayoutChanged { layout: "RU".into() },
            ShellSignal::CapsLock { on: true },
            ShellSignal::Brightness { level: 0.5 },
        ];
        for s in all {
            let text = serde_json::to_string(&s).unwrap();
            assert_eq!(serde_json::from_str::<ShellSignal>(&text).unwrap(), s, "{text}");
        }
        for a in [
            WindowAction::Activate,
            WindowAction::Minimize,
            WindowAction::ToggleMaximize,
            WindowAction::SetAbove(false),
            WindowAction::SetOpacity(0),
            WindowAction::TileLeft,
            WindowAction::TileRight,
        ] {
            let text = serde_json::to_string(&a).unwrap();
            assert_eq!(serde_json::from_str::<WindowAction>(&text).unwrap(), a);
        }
    }

    #[test]
    fn unknown_wire_names_are_rejected() {
        assert!(serde_json::from_str::<Capability>("\"teleport\"").is_err());
        assert!(serde_json::from_value::<WindowAction>(json!({"action": "close"})).is_err());
    }

    #[test]
    fn null_backend_reports_unsupported() {
        let b = NullBackend;
        assert!(b.capabilities().is_empty());
        assert!(!b.supports(Capability::ClipboardRead));
        assert_eq!(b.focus(), Err(ShellError::Unsupported(Capability::FocusTracking)));
        assert_eq!(
            b.place(WindowRole::Island, Rect::default(), &[]),
            Err(ShellError::Unsupported(Capability::PlaceWindows))
        );
        assert_eq!(
            b.window_action(1, WindowAction::Minimize),
            Err(ShellError::Unsupported(Capability::WindowActions))
        );
        assert_eq!(b.get_clipboard("text/plain"), Err(ShellError::Unsupported(Capability::ClipboardRead)));
        assert_eq!(b.screenshot(None, false), Err(ShellError::Unsupported(Capability::Screenshot)));
        assert_eq!(b.bind_shortcuts(&[]), Err(ShellError::Unsupported(Capability::GlobalShortcuts)));
        // Usable as a trait object.
        let boxed: Box<dyn ShellBackend> = Box::new(NullBackend);
        assert!(boxed.set_capture_hidden(true).is_err());
    }
}
