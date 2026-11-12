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
    let Ok(connection) = zbus::blocking::connection::Builder::session().and_then(|builder| {
        builder
            .method_timeout(std::time::Duration::from_secs(5))
            .build()
    }) else {
        return LaunchDecision::NoAdapter;
    };
    let Ok(proxy) = zbus::blocking::Proxy::new(&connection, BUS_NAME, OBJECT_PATH, INTERFACE)
    else {
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

use futures_lite::{future, StreamExt};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::time::Duration;
use tidemark_core::shell::{FocusInfo, Rect, ShellError, ShortcutId, SHELL_CONTRACT_VERSION};
use zbus::zvariant::{OwnedValue, Structure};

#[path = "shell_state.rs"]
pub mod state;
pub use state::{Peer, ReadKey};
use tidemark_core::shell::{ClipboardOffer, WindowAction, WindowState};

const CALL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum Event {
    Connected(Vec<Capability>, Peer),
    ClipboardOffer {
        offer: ClipboardOffer,
        key: Option<ReadKey>,
    },
    ClipboardText(ReadKey, String),
    ClipboardWritten {
        request: u64,
        peer: Peer,
        result: Result<(), ShellError>,
    },
    WindowResult {
        request: u64,
        peer: Peer,
        id: u64,
        result: Result<WindowState, ShellError>,
    },
    Disconnected(String),
    NeedsHandover,
    HandoverAccepted,
    Focus(Option<FocusInfo>),
    Fullscreen(u32, bool),
    PrimaryMonitor(u32),
    Maximized(bool),
    Layout(String),
    CapsLock(bool),
    Shortcut(ShortcutId),
    Bindings(Vec<(String, bool)>),
    Error(ShellError),
}

#[derive(Debug)]
pub enum Command {
    Place(Rect),
    Handover,
    Shortcuts(bool),
    CaptureClipboard(bool),
    Window {
        request: u64,
        peer: Peer,
        id: u64,
        action: WindowAction,
    },
    SetClipboard {
        request: u64,
        peer: Peer,
        text: String,
    },
}

#[derive(Clone, Default)]
pub struct Service(Option<SyncSender<Command>>);
impl Service {
    pub fn send(&self, command: Command) -> bool {
        self.0
            .as_ref()
            .is_some_and(|tx| tx.try_send(command).is_ok())
    }
}

pub fn spawn(
    events: Sender<Event>,
    mut launcher_enabled: bool,
    mut clipboard_enabled: bool,
) -> Service {
    let (tx, rx) = mpsc::sync_channel(16);
    let worker_events = events.clone();
    let result = std::thread::Builder::new()
        .name("tidemark-shell".into())
        .spawn(move || {
            let events = worker_events;
            let mut generation = 0u64;
            async_io::block_on(async move {
                loop {
                    if let Err(error) = session(
                        &events,
                        &rx,
                        &mut launcher_enabled,
                        &mut clipboard_enabled,
                        &mut generation,
                    )
                    .await
                    {
                        if events.send(Event::Disconnected(error.to_string())).is_err() {
                            return;
                        }
                    } else {
                        return;
                    }
                    // Reconnect the session bus after loss, with a bounded backoff.
                    for _ in 0..10 {
                        async_io::Timer::after(Duration::from_millis(100)).await;
                    }
                }
            });
        });
    match result {
        Ok(_) => Service(Some(tx)),
        Err(error) => {
            let _ = events.send(Event::Disconnected(format!(
                "Не удалось запустить мост GNOME: {error}"
            )));
            Service::default()
        }
    }
}

async fn session(
    events: &Sender<Event>,
    commands: &Receiver<Command>,
    launcher_enabled: &mut bool,
    clipboard_enabled: &mut bool,
    generation: &mut u64,
) -> zbus::Result<()> {
    let connect = async {
        zbus::connection::Builder::session()?
            .method_timeout(CALL_TIMEOUT)
            .build()
            .await
    };
    let connection = future::or(connect, async {
        async_io::Timer::after(CALL_TIMEOUT).await;
        Err(zbus::Error::Failure(
            "session bus connection timed out".into(),
        ))
    })
    .await?;
    // Install both subscriptions BEFORE querying the owner or sending Hello.
    // The initial unicast snapshot is queued even while Hello is outstanding.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(INTERFACE)?
        .path(OBJECT_PATH)?
        .build();
    let mut signals = zbus::MessageStream::for_match_rule(rule, &connection, Some(128)).await?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")?
        .interface("org.freedesktop.DBus")?
        .member("NameOwnerChanged")?
        .add_arg(BUS_NAME)?
        .build();
    let mut owners = zbus::MessageStream::for_match_rule(rule, &connection, Some(16)).await?;
    let bus = zbus::Proxy::new(
        &connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await?;
    let mut owner: String = bus
        .call("GetNameOwner", &(BUS_NAME,))
        .await
        .unwrap_or_default();
    *generation = generation.wrapping_add(1);
    let mut authenticated = false;
    let mut clipboard = state::ClipboardGuard::default();
    clipboard.enable(*clipboard_enabled);
    let (read_tx, read_rx) = mpsc::sync_channel::<(ReadKey, Result<String, ShellError>)>(1);
    let mut reading = false;
    let mut attempted: Option<ReadKey> = None;
    let mut pending_owner = None;
    let mut latest_place = None;
    let mut handover_pending = false;
    let mut state_check = std::time::Instant::now();
    let mut previous_state = String::new();
    if !owner.is_empty() {
        authenticated =
            authenticate(&connection, &owner, events, *launcher_enabled, *generation).await;
    } else {
        let _ = events.send(Event::Disconnected("Адаптер GNOME не найден".into()));
    }

    loop {
        enum Wake {
            Signal(Option<zbus::Result<zbus::Message>>),
            Owner(Option<zbus::Result<zbus::Message>>),
            Tick,
        }
        let wake = if let Some(message) = pending_owner.take() {
            Wake::Owner(Some(Ok(message)))
        } else {
            future::or(
                future::or(async { Wake::Owner(owners.next().await) }, async {
                    Wake::Signal(signals.next().await)
                }),
                async {
                    async_io::Timer::after(Duration::from_millis(100)).await;
                    Wake::Tick
                },
            )
            .await
        };
        match wake {
            Wake::Owner(Some(Ok(message))) => {
                let (name, _old, next): (String, String, String) = message.body().deserialize()?;
                if name == BUS_NAME && next != owner {
                    owner = next;
                    *generation = generation.wrapping_add(1);
                    clipboard.invalidate();
                    attempted = None;
                    authenticated = false;
                    previous_state.clear();
                    let _ =
                        events.send(Event::Disconnected("Адаптер GNOME переподключается".into()));
                    if !owner.is_empty() {
                        authenticated = authenticate(
                            &connection,
                            &owner,
                            events,
                            *launcher_enabled,
                            *generation,
                        )
                        .await;
                        if authenticated {
                            if let Some(rect) = latest_place {
                                place(&connection, &owner, rect, events).await;
                            }
                        }
                    }
                }
            }
            Wake::Signal(Some(Ok(message))) => {
                dispatch_signal(
                    &message,
                    authenticated,
                    &owner,
                    *generation,
                    &connection,
                    events,
                    &mut clipboard,
                );
            }
            Wake::Signal(Some(Err(error))) | Wake::Owner(Some(Err(error))) => return Err(error),
            Wake::Signal(None) | Wake::Owner(None) => {
                return Err(zbus::Error::Failure(
                    "session bus signal stream ended".into(),
                ))
            }
            Wake::Tick => {}
        }
        let mut drained = 0;
        while let Some(message) = future::poll_once(signals.next()).await {
            match message {
                Some(Ok(message)) => dispatch_signal(
                    &message,
                    authenticated,
                    &owner,
                    *generation,
                    &connection,
                    events,
                    &mut clipboard,
                ),
                Some(Err(error)) => return Err(error),
                None => return Err(zbus::Error::Failure("GNOME signal stream ended".into())),
            }
            drained += 1;
            if drained >= 128 {
                break;
            }
        }
        if drained >= 128 {
            continue;
        }
        if let Some(message) = future::poll_once(owners.next()).await {
            match message {
                Some(Ok(message)) => {
                    pending_owner = Some(message);
                    continue;
                }
                Some(Err(error)) => return Err(error),
                None => return Ok(()),
            }
        }
        if let Ok((key, result)) = read_rx.try_recv() {
            reading = false;
            if clipboard.accepts(&key) && key.peer.matches(&owner, *generation) && authenticated {
                match result {
                    Ok(text) => {
                        let _ = events.send(Event::ClipboardText(key, text));
                    }
                    Err(error) => {
                        let _ = events.send(Event::Error(error));
                    }
                }
            }
        }
        if !reading && authenticated && !handover_pending {
            if let Some(key) = clipboard
                .current()
                .filter(|key| attempted.as_ref() != Some(key))
            {
                let tx = read_tx.clone();
                let conn = connection.clone();
                let task_key = key.clone();
                match std::thread::Builder::new()
                    .name("tidemark-clipboard-read".into())
                    .spawn(move || {
                        let result = async_io::block_on(read_clipboard(&conn, &task_key));
                        let _ = tx.try_send((task_key, result));
                    }) {
                    Ok(_) => {
                        reading = true;
                        attempted = Some(key);
                    }
                    Err(error) => {
                        attempted = Some(key);
                        let _ = events.send(Event::Error(ShellError::Failed(error.to_string())));
                    }
                }
            }
        }
        loop {
            // Prioritize bus ownership transitions over queued user mutations,
            // including release/reacquire by the SAME unique owner.
            if let Some(message) = future::poll_once(owners.next()).await {
                match message {
                    Some(Ok(message)) => {
                        pending_owner = Some(message);
                        break;
                    }
                    Some(Err(error)) => return Err(error),
                    None => return Ok(()),
                }
            }
            match commands.try_recv() {
                Ok(Command::CaptureClipboard(enabled)) => {
                    *clipboard_enabled = enabled;
                    clipboard.enable(enabled);
                }
                Ok(Command::Window {
                    request,
                    peer,
                    id,
                    action,
                }) => {
                    let result = if authenticated
                        && !handover_pending
                        && peer.matches(&owner, *generation)
                    {
                        mutate_window(&connection, &bus, &peer, id, action).await
                    } else {
                        Err(stale_peer())
                    };
                    let _ = events.send(Event::WindowResult {
                        request,
                        peer,
                        id,
                        result,
                    });
                }
                Ok(Command::SetClipboard {
                    request,
                    peer,
                    text,
                }) => {
                    let result = if authenticated
                        && !handover_pending
                        && peer.matches(&owner, *generation)
                    {
                        write_clipboard(&connection, &bus, &peer, &text).await
                    } else {
                        Err(stale_peer())
                    };
                    let _ = events.send(Event::ClipboardWritten {
                        request,
                        peer,
                        result,
                    });
                }
                Ok(Command::Shortcuts(enabled)) => {
                    if *launcher_enabled != enabled {
                        *launcher_enabled = enabled;
                        if authenticated && !handover_pending {
                            bind_shortcuts(&connection, &owner, events, enabled).await;
                        }
                    }
                }
                Ok(Command::Place(rect)) => {
                    latest_place = Some(rect);
                    if authenticated && !handover_pending {
                        place(&connection, &owner, rect, events).await;
                    }
                }
                Ok(Command::Handover) if !owner.is_empty() && !handover_pending => {
                    let proxy =
                        zbus::Proxy::new(&connection, owner.as_str(), OBJECT_PATH, INTERFACE)
                            .await?;
                    match proxy
                        .call::<_, _, bool>("RequestHandover", &(Vec::<String>::new(),))
                        .await
                    {
                        Ok(true) => {
                            // Keep this authenticated bus connection alive until
                            // GTK quits. The adapter waits for this unique name.
                            handover_pending = true;
                            let _ = events.send(Event::HandoverAccepted);
                        }
                        Ok(false) => {
                            let _ = events.send(Event::Error(ShellError::Failed(
                                "GNOME отклонил перезапуск".into(),
                            )));
                        }
                        Err(error) => {
                            let _ = events.send(Event::Error(map_error(error)));
                        }
                    }
                }
                Ok(Command::Handover) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        if !authenticated && !owner.is_empty() && state_check.elapsed() >= Duration::from_secs(1) {
            let proxy =
                zbus::Proxy::new(&connection, owner.as_str(), OBJECT_PATH, INTERFACE).await?;
            if let Ok(state) = proxy.get_property::<String>("ClientState").await {
                if state != previous_state && state == "blocked" {
                    let _ = events.send(Event::NeedsHandover);
                }
                previous_state = state;
            }
            state_check = std::time::Instant::now();
        }
    }
}

async fn authenticate(
    connection: &zbus::Connection,
    owner: &str,
    events: &Sender<Event>,
    launcher_enabled: bool,
    generation: u64,
) -> bool {
    if !launched_by_adapter() {
        return false;
    }
    let result = async {
        let proxy = zbus::Proxy::new(connection, owner, OBJECT_PATH, INTERFACE).await?;
        let (version, names): (u32, Vec<String>) = proxy.call("Hello", &()).await?;
        let caps = validate_hello(version, &names).map_err(zbus::Error::Failure)?;
        // Reject a Hello reply from an owner that disappeared during the call.
        let bus = zbus::Proxy::new(
            connection,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .await?;
        let live: String = bus.call("GetNameOwner", &(BUS_NAME,)).await?;
        if live != owner {
            return Err(zbus::Error::Failure(
                "GNOME owner changed during Hello".into(),
            ));
        }
        let _ = events.send(Event::Connected(
            caps.clone(),
            Peer {
                owner: owner.into(),
                generation,
            },
        ));
        // Additive v1 property: older adapters can still place and track state,
        // but cannot enable primary-only fullscreen hiding without this value.
        if let Ok(primary) = proxy.get_property::<u32>("PrimaryMonitor").await {
            let _ = events.send(Event::PrimaryMonitor(primary));
        }
        if caps.contains(&Capability::GlobalShortcuts) {
            bind_shortcuts(connection, owner, events, launcher_enabled).await;
        }
        Ok::<(), zbus::Error>(())
    }
    .await;
    match result {
        Ok(()) => true,
        Err(error) => {
            let denied = matches!(&error, zbus::Error::MethodError(name, _, _) if name.as_str() == "org.tidemark.Shell1.Error.AccessDenied");
            let _ = events.send(Event::Error(map_error(error)));
            if denied {
                let _ = events.send(Event::NeedsHandover);
            }
            false
        }
    }
}

fn stale_peer() -> ShellError {
    ShellError::Failed("GNOME изменился; действие отменено. Выберите окно заново.".into())
}
fn dispatch_signal(
    message: &zbus::Message,
    authenticated: bool,
    owner: &str,
    generation: u64,
    connection: &zbus::Connection,
    events: &Sender<Event>,
    clipboard: &mut state::ClipboardGuard,
) {
    let header = message.header();
    if !authenticated
        || header.sender().map(|v| v.as_str()) != Some(owner)
        || header.destination().map(|v| v.as_str()) != connection.unique_name().map(|v| v.as_str())
    {
        return;
    }
    match decode_signal(message) {
        Ok(Some(mut event)) => {
            if let Event::ClipboardOffer { offer, key } = &mut event {
                *key = clipboard.offer(
                    &Peer {
                        owner: owner.into(),
                        generation,
                    },
                    offer,
                );
            }
            let _ = events.send(event);
        }
        Ok(None) => {}
        Err(error) => {
            let _ = events.send(Event::Error(ShellError::Failed(error)));
        }
    }
}
async fn verify_owner(bus: &zbus::Proxy<'_>, peer: &Peer) -> Result<(), ShellError> {
    let live: String = bus
        .call("GetNameOwner", &(BUS_NAME,))
        .await
        .map_err(map_error)?;
    if live != peer.owner {
        return Err(stale_peer());
    }
    Ok(())
}
fn action_wire(action: WindowAction) -> Result<(&'static str, OwnedValue), ShellError> {
    Ok(match action {
        WindowAction::Activate => ("activate", 0u8.into()),
        WindowAction::Minimize => ("minimize", 0u8.into()),
        WindowAction::ToggleMaximize => ("toggle_maximize", 0u8.into()),
        WindowAction::TileLeft => ("tile_left", 0u8.into()),
        WindowAction::TileRight => ("tile_right", 0u8.into()),
        WindowAction::SetAbove(on) => ("set_above", on.into()),
        WindowAction::SetOpacity(value) if value >= 26 => ("set_opacity", value.into()),
        WindowAction::SetOpacity(_) => {
            return Err(ShellError::Failed("Недопустимая прозрачность окна".into()))
        }
    })
}
async fn mutate_window(
    connection: &zbus::Connection,
    bus: &zbus::Proxy<'_>,
    peer: &Peer,
    id: u64,
    action: WindowAction,
) -> Result<WindowState, ShellError> {
    let (action, arg) = action_wire(action)?;
    verify_owner(bus, peer).await?;
    let proxy = zbus::Proxy::new(connection, peer.owner.as_str(), OBJECT_PATH, INTERFACE)
        .await
        .map_err(map_error)?;
    let dict: HashMap<String, OwnedValue> = proxy
        .call("WindowAction", &(id, action, arg))
        .await
        .map_err(map_error)?;
    decode_window_state(dict).map_err(ShellError::Failed)
}
pub fn decode_window_state(mut dict: HashMap<String, OwnedValue>) -> Result<WindowState, String> {
    let state = WindowState {
        maximized: take(&mut dict, "maximized")?,
        above: take(&mut dict, "above")?,
        opacity: take(&mut dict, "opacity")?,
        minimized: take(&mut dict, "minimized")?,
    };
    if !dict.is_empty() {
        return Err("unexpected window state field".into());
    }
    Ok(state)
}
async fn read_clipboard(
    connection: &zbus::Connection,
    key: &ReadKey,
) -> Result<String, ShellError> {
    let proxy = zbus::Proxy::new(connection, key.peer.owner.as_str(), OBJECT_PATH, INTERFACE)
        .await
        .map_err(map_error)?;
    let message = proxy
        .call_method("GetClipboard", &(key.mime.as_str(),))
        .await
        .map_err(map_error)?;
    let body = message.body();
    let (bytes,): (&[u8],) = body
        .deserialize()
        .map_err(|error| ShellError::Failed(error.to_string()))?;
    state::text(bytes).map_err(ShellError::Failed)
}
async fn write_clipboard(
    connection: &zbus::Connection,
    bus: &zbus::Proxy<'_>,
    peer: &Peer,
    text: &str,
) -> Result<(), ShellError> {
    state::text(text.as_bytes()).map_err(ShellError::Failed)?;
    verify_owner(bus, peer).await?;
    let proxy = zbus::Proxy::new(connection, peer.owner.as_str(), OBJECT_PATH, INTERFACE)
        .await
        .map_err(map_error)?;
    proxy
        .call::<_, _, ()>(
            "SetClipboard",
            &("text/plain;charset=utf-8", text.as_bytes()),
        )
        .await
        .map_err(map_error)
}

fn shortcut_bindings(launcher_enabled: bool) -> Vec<(&'static str, &'static str)> {
    let mut bindings = vec![("toggle_theme", "<Super><Alt>t")];
    if launcher_enabled {
        bindings.push(("launcher", "<Super><Alt>space"));
    }
    bindings
}
async fn bind_shortcuts(
    connection: &zbus::Connection,
    owner: &str,
    events: &Sender<Event>,
    launcher_enabled: bool,
) {
    let result = async {
        let proxy = zbus::Proxy::new(connection, owner, OBJECT_PATH, INTERFACE).await?;
        proxy
            .call::<_, _, Vec<(String, bool)>>(
                "BindShortcuts",
                &(shortcut_bindings(launcher_enabled),),
            )
            .await
    }
    .await;
    match result {
        Ok(results) => {
            let _ = events.send(Event::Bindings(results));
        }
        Err(error) => {
            let _ = events.send(Event::Error(map_error(error)));
        }
    }
}

async fn place(connection: &zbus::Connection, owner: &str, rect: Rect, events: &Sender<Event>) {
    let result = async {
        let proxy = zbus::Proxy::new(connection, owner, OBJECT_PATH, INTERFACE).await?;
        // GDK owns the actual rounded input region; Shell1 input is ignored.
        proxy
            .call::<_, _, ()>(
                "Place",
                &(
                    "island",
                    (rect.x, rect.y, rect.w, rect.h),
                    Vec::<(i32, i32, i32, i32)>::new(),
                ),
            )
            .await
    }
    .await;
    if let Err(error) = result {
        let _ = events.send(Event::Error(map_error(error)));
    }
}

pub fn validate_hello(version: u32, names: &[String]) -> Result<Vec<Capability>, String> {
    if version != SHELL_CONTRACT_VERSION {
        return Err(format!("Несовместимый контракт GNOME: {version}"));
    }
    let mut caps = Vec::new();
    for name in names {
        let cap = capability_from_wire(name)
            .ok_or_else(|| format!("Неизвестная возможность GNOME: {name}"))?;
        if caps.contains(&cap) {
            return Err(format!("Повтор возможности GNOME: {name}"));
        }
        caps.push(cap);
    }
    Ok(caps)
}

fn map_error(error: zbus::Error) -> ShellError {
    if let zbus::Error::MethodError(name, detail, _) = &error {
        if name.as_str() == "org.tidemark.Shell1.Error.Unsupported" {
            if let Some(cap) = detail.as_deref().and_then(capability_from_wire) {
                return ShellError::Unsupported(cap);
            }
        }
    }
    ShellError::Failed(error.to_string())
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

fn take<T: TryFrom<OwnedValue, Error = zbus::zvariant::Error>>(
    dict: &mut HashMap<String, OwnedValue>,
    key: &str,
) -> Result<T, String> {
    T::try_from(
        dict.remove(key)
            .ok_or_else(|| format!("missing focus field {key}"))?,
    )
    .map_err(|_| format!("invalid focus field {key}"))
}

pub fn decode_focus(mut dict: HashMap<String, OwnedValue>) -> Result<Option<FocusInfo>, String> {
    if dict.is_empty() {
        return Ok(None);
    }
    let rect: Structure = take(&mut dict, "rect")?;
    let values = rect.fields();
    if values.len() != 4 {
        return Err("invalid focus rect".into());
    }
    let coordinates: Vec<i32> = values
        .iter()
        .map(|v| i32::try_from(v).map_err(|_| "invalid focus rect".to_owned()))
        .collect::<Result<_, _>>()?;
    if coordinates[2] < 0 || coordinates[3] < 0 {
        return Err("negative focus size".into());
    }
    let pid = if dict.contains_key("pid") {
        Some(take(&mut dict, "pid")?)
    } else {
        None
    };
    let icon = if dict.contains_key("icon") {
        Some(take(&mut dict, "icon")?)
    } else {
        None
    };
    let focus = FocusInfo {
        id: take(&mut dict, "id")?,
        app_id: take(&mut dict, "app_id")?,
        title: take(&mut dict, "title")?,
        pid,
        icon,
        maximized: take(&mut dict, "maximized")?,
        above: take(&mut dict, "above")?,
        opacity: take(&mut dict, "opacity")?,
        monitor: take(&mut dict, "monitor")?,
        rect: Rect {
            x: coordinates[0],
            y: coordinates[1],
            w: coordinates[2],
            h: coordinates[3],
        },
    };
    if !dict.is_empty() {
        return Err("unexpected focus field".into());
    }
    Ok(Some(focus))
}

fn decode_signal(message: &zbus::Message) -> Result<Option<Event>, String> {
    let header = message.header();
    let member = header.member().map(|v| v.as_str()).unwrap_or("");
    let body = message.body();
    let event = match member {
        "ClipboardChanged" => {
            let (mimes, sensitive) = body.deserialize().map_err(|e| e.to_string())?;
            Event::ClipboardOffer {
                offer: ClipboardOffer { mimes, sensitive },
                key: None,
            }
        }
        "FocusChanged" => {
            let (dict,): (HashMap<String, OwnedValue>,) =
                body.deserialize().map_err(|e| e.to_string())?;
            Event::Focus(decode_focus(dict)?)
        }
        "FullscreenChanged" => {
            let (monitor, on) = body.deserialize().map_err(|e| e.to_string())?;
            Event::Fullscreen(monitor, on)
        }
        "PrimaryMonitorChanged" => {
            let (monitor,) = body.deserialize().map_err(|e| e.to_string())?;
            Event::PrimaryMonitor(monitor)
        }
        "MaximizedTopChanged" => {
            let (on,) = body.deserialize().map_err(|e| e.to_string())?;
            Event::Maximized(on)
        }
        "LayoutChanged" => {
            let (layout,) = body.deserialize().map_err(|e| e.to_string())?;
            Event::Layout(layout)
        }
        "CapsLock" => {
            let (on,) = body.deserialize().map_err(|e| e.to_string())?;
            Event::CapsLock(on)
        }
        "ShortcutActivated" => {
            let (id,): (String,) = body.deserialize().map_err(|e| e.to_string())?;
            Event::Shortcut(match id.as_str() {
                "launcher" => ShortcutId::Launcher,
                "toggle_theme" => ShortcutId::ToggleTheme,
                "like_track" => ShortcutId::LikeTrack,
                _ => return Err("unknown shortcut".into()),
            })
        }
        _ => return Ok(None),
    };
    Ok(Some(event))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Str;
    fn focus_dict() -> HashMap<String, OwnedValue> {
        HashMap::from([
            ("id".into(), 42u64.into()),
            (
                "app_id".into(),
                OwnedValue::from(Str::from("org.gnome.Nautilus")),
            ),
            ("title".into(), OwnedValue::from(Str::from("Documents"))),
            ("maximized".into(), false.into()),
            ("above".into(), true.into()),
            ("opacity".into(), 255u8.into()),
            ("monitor".into(), 0u32.into()),
            (
                "rect".into(),
                OwnedValue::try_from(Structure::from((-10i32, 20i32, 800i32, 600i32))).unwrap(),
            ),
        ])
    }
    #[test]
    fn window_action_variants_and_confirmed_dict_keep_exact_types() {
        let (name, arg) = action_wire(WindowAction::SetAbove(true)).unwrap();
        assert_eq!(name, "set_above");
        assert_eq!(bool::try_from(arg).unwrap(), true);
        let (name, arg) = action_wire(WindowAction::SetOpacity(128)).unwrap();
        assert_eq!(name, "set_opacity");
        assert_eq!(u8::try_from(arg).unwrap(), 128);
        assert!(action_wire(WindowAction::SetOpacity(25)).is_err());
        let fields = || {
            HashMap::from([
                ("maximized".into(), false.into()),
                ("above".into(), true.into()),
                ("opacity".into(), 128u8.into()),
                ("minimized".into(), false.into()),
            ])
        };
        assert_eq!(decode_window_state(fields()).unwrap().opacity, 128);
        let mut wrong = fields();
        wrong.insert("opacity".into(), 128u32.into());
        assert!(decode_window_state(wrong).is_err());
        let mut missing = fields();
        missing.remove("above");
        assert!(decode_window_state(missing).is_err());
    }
    #[test]
    fn clipboard_bytes_are_borrowed_and_secret_offer_is_retained() {
        let call = zbus::Message::method_call(OBJECT_PATH, "GetClipboard")
            .unwrap()
            .build(&())
            .unwrap();
        let message = zbus::Message::method_return(&call.header())
            .unwrap()
            .build(&(b"safe fixture".as_slice(),))
            .unwrap();
        let body = message.body();
        let (bytes,): (&[u8],) = body.deserialize().unwrap();
        assert_eq!(state::text(bytes).unwrap(), "safe fixture");
        let message = zbus::Message::signal(OBJECT_PATH, INTERFACE, "ClipboardChanged")
            .unwrap()
            .build(&(vec!["text/plain"], true))
            .unwrap();
        assert!(matches!(
            decode_signal(&message).unwrap(),
            Some(Event::ClipboardOffer {
                offer: ClipboardOffer {
                    sensitive: true,
                    ..
                },
                key: None
            })
        ));
    }
    #[test]
    fn disabled_launcher_releases_search_binding_and_preserves_theme() {
        assert_eq!(
            shortcut_bindings(false),
            vec![("toggle_theme", "<Super><Alt>t")]
        );
        assert!(shortcut_bindings(true).contains(&("launcher", "<Super><Alt>space")));
    }
    #[test]
    fn primary_fullscreen_policy_ignores_secondary_and_unknown_mapping() {
        let screens = std::collections::HashSet::from([1u32]);
        assert!(!hide_for_fullscreen(true, Some(0), &screens));
        assert!(!hide_for_fullscreen(true, None, &screens));
        assert!(!hide_for_fullscreen(false, Some(1), &screens));
        assert!(hide_for_fullscreen(true, Some(1), &screens));
    }
    #[test]
    fn command_queue_reports_backpressure_instead_of_claiming_placement() {
        let (tx, _rx) = mpsc::sync_channel(1);
        let service = Service(Some(tx));
        assert!(service.send(Command::Place(Rect::default())));
        assert!(!service.send(Command::Place(Rect {
            w: 560,
            ..Rect::default()
        })));
        assert!(!Service::default().send(Command::Handover));
    }
    #[test]
    fn empty_focus_is_none_and_complete_focus_preserves_types() {
        assert_eq!(decode_focus(HashMap::new()).unwrap(), None);
        let focus = decode_focus(focus_dict()).unwrap().unwrap();
        assert_eq!(focus.id, 42);
        assert_eq!(focus.rect.x, -10);
        assert_eq!(focus.pid, None);
        assert_eq!(focus.icon, None);
        assert_eq!(focus.opacity, 255);
    }
    #[test]
    fn malformed_focus_never_becomes_no_focus() {
        let mut fields = focus_dict();
        fields.remove("above");
        assert!(decode_focus(fields).is_err());
        let mut fields = focus_dict();
        fields.insert("opacity".into(), 255u32.into());
        assert!(decode_focus(fields).is_err());
        let mut fields = focus_dict();
        fields.insert("pid".into(), OwnedValue::from(Str::from("123")));
        assert!(decode_focus(fields).is_err());
        let mut fields = focus_dict();
        fields.insert("future_field".into(), true.into());
        assert!(decode_focus(fields).is_err());
    }
    #[test]
    fn optional_focus_fields_are_decoded_without_coercion() {
        let mut fields = focus_dict();
        fields.insert("pid".into(), 123u32.into());
        fields.insert("icon".into(), OwnedValue::from(Str::from("folder")));
        let focus = decode_focus(fields).unwrap().unwrap();
        assert_eq!(focus.pid, Some(123));
        assert_eq!(focus.icon.as_deref(), Some("folder"));
    }
    #[test]
    fn hello_rejects_wrong_version_unknown_or_duplicate_capabilities() {
        assert_eq!(
            validate_hello(1, &["place_windows".into()]).unwrap(),
            vec![Capability::PlaceWindows]
        );
        assert!(validate_hello(2, &[]).is_err());
        assert!(validate_hello(1, &["made_up".into()]).is_err());
        assert!(validate_hello(1, &["place_windows".into(), "place_windows".into()]).is_err());
    }
    #[test]
    fn unsupported_errors_require_exact_error_name_and_capability() {
        let call = zbus::Message::method_call(OBJECT_PATH, "Place")
            .unwrap()
            .build(&())
            .unwrap();
        let error = |name, detail: &str| {
            zbus::Error::from(
                zbus::Message::error(&call.header(), name)
                    .unwrap()
                    .build(&(detail,))
                    .unwrap(),
            )
        };
        assert!(matches!(
            map_error(error(
                "org.tidemark.Shell1.Error.Unsupported",
                "place_windows"
            )),
            ShellError::Unsupported(Capability::PlaceWindows)
        ));
        assert!(matches!(
            map_error(error(
                "org.tidemark.Shell1.Error.Unsupported",
                "place_windows "
            )),
            ShellError::Failed(_)
        ));
        assert!(matches!(
            map_error(error("org.tidemark.Shell1.Error.Failed", "place_windows")),
            ShellError::Failed(_)
        ));
    }
    #[test]
    fn signal_body_dictionary_and_shortcut_use_exact_wire_shapes() {
        let message = zbus::Message::signal(OBJECT_PATH, INTERFACE, "FocusChanged")
            .unwrap()
            .build(&(focus_dict(),))
            .unwrap();
        assert!(matches!(
            decode_signal(&message).unwrap(),
            Some(Event::Focus(Some(FocusInfo { id: 42, .. })))
        ));
        let message = zbus::Message::signal(OBJECT_PATH, INTERFACE, "FocusChanged")
            .unwrap()
            .build(&(HashMap::<String, OwnedValue>::new(),))
            .unwrap();
        assert!(matches!(
            decode_signal(&message).unwrap(),
            Some(Event::Focus(None))
        ));
        let message = zbus::Message::signal(OBJECT_PATH, INTERFACE, "ShortcutActivated")
            .unwrap()
            .build(&("toggle_theme",))
            .unwrap();
        assert!(matches!(
            decode_signal(&message).unwrap(),
            Some(Event::Shortcut(ShortcutId::ToggleTheme))
        ));
    }
}

/// The island is placed on the primary monitor. Unknown mapping disables
/// automatic hiding rather than reacting to a secondary monitor's fullscreen.
pub fn hide_for_fullscreen(
    enabled: bool,
    primary: Option<u32>,
    fullscreen: &std::collections::HashSet<u32>,
) -> bool {
    enabled && primary.is_some_and(|monitor| fullscreen.contains(&monitor))
}
