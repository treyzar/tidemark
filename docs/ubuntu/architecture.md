# Tidemark for Ubuntu — architecture

Status: **architecture and acceptance plan; implementation in progress.** Reference product: Tidemark for Windows **v1.3.0** (tag `v1.3.0`). Current implemented capabilities and preview limitations are listed in the [README](../../README.md#linux); the matrices below remain acceptance targets, not a report that every feature works.
Nothing in this document claims parity. A feature is at parity only after it passes the release gates in §9 on real Ubuntu systems.

## 1. Targets

| | Baseline | Compatibility |
|---|---|---|
| OS | Ubuntu 24.04 LTS, amd64 | Ubuntu 26.04 LTS, amd64 |
| Desktop | GNOME Shell / Mutter 46 | GNOME Shell / Mutter 50 |
| Sessions | Wayland (default) **and** Xorg (optional login session) | Wayland only (GNOME 50 removed the X11 session; X11 apps run through XWayland) |
| Toolkit floor | GTK 4.14, libadwaita 1.5 (Ubuntu 24.04 archive) | GTK/libadwaita from 26.04 |
| Audio | PipeWire 1.0 + pipewire-pulse | PipeWire from 26.04 |

Other desktops (KDE, Sway, Hyprland) are not release targets. Code paths must fail gracefully there, but this release doesn't test them.

Windows remains a first-class product: the same behaviour, binary names (`win-island.exe`, `island-hook.exe`), data path and installer.

## 2. Platform constraints that shape the design

1. **On Wayland, ordinary clients cannot**:
   - position their own toplevel window;
   - stay always-on-top, stay out of the taskbar/Alt+Tab, or avoid taking focus when shown;
   - read the clipboard without keyboard focus;
   - read global key state (Caps Lock, layout) or grab global hotkeys outside a portal;
   - inspect, activate or modify other apps' windows;
   - exclude themselves from screen capture.

   All of these are things Tidemark does on Windows.
2. **GNOME/Mutter does not provide `wlr-layer-shell` or a data-control protocol** that we can rely on. Treat both as absent; at most, probe the Wayland registry at runtime.
3. **The GNOME top bar is Shell UI drawn above all client windows.** A client window cannot cover the clock area the way the Windows island sits at the screen's top edge. Decision: the island sits **directly below the top bar** (`top_offset` defaults to the panel height). Hiding or replacing the clock is out of scope.
4. **The GlobalShortcuts portal is implemented by xdg-desktop-portal-gnome only since GNOME 48.** It is unavailable on the 24.04 baseline, so hotkeys go through the compositor adapter.
5. **The accent-color setting from the Settings portal is only available from GNOME 47.** On 46, the accent from `config.toml` is used.
6. **`Meta.WaylandClient` was redesigned in GNOME 49.**
   - 46–48: launch the client with the pre-redesign `Meta.WaylandClient` subprocess API, then call `hide_from_window_list(window)` on the client object.
   - 49 and later: `Meta.WaylandClient.new_subprocess(...)`, then `Meta.Window.hide_from_window_list()` / `set_type()`.
7. **Default shortcuts on Ubuntu clash with Tidemark's defaults.**
   - Alt+Space opens the window menu (`activate-window-menu`).
   - Ctrl+Alt+T opens a terminal.
   - Ctrl+Alt+L: check for a conflict at runtime.
8. **GNOME has no system tray.** Ubuntu ships the AppIndicator/KStatusNotifierItem extension enabled by default.
9. **Shell extensions must be JavaScript (GJS, ES modules since GNOME 45).** Rust cannot be loaded into the compositor.

Consequence: **a Rust GTK4 application plus a minimal GJS compositor adapter.** The adapter only does what Mutter forbids clients to do (§4.3). It contains no product logic and no product UI. On the Xorg session, an `x11rb` backend implements the same Rust trait, so the adapter is optional there.

## 3. Repository layout

The parent simplified this on 2026-11-12: there is no mass move of the Windows code.

```
Cargo.toml                    root package `win-island` (Windows app, src/, build.rs) + [workspace]
                              members = [".", "crates/tidemark-core"], default-members = ["."]
                              exclude = ["crates/tidemark-mpris"] (standalone until reviewed)
src/                          Windows app; portable modules re-exported from tidemark-core
crates/
  tidemark-core/              DONE (milestone 1): pure Rust, no OS APIs, builds/tests on Windows and Linux
  tidemark-net/               later: trait Http + WinHttp (cfg windows) / ureq+rustls (cfg linux)
  tidemark-mpris/             integrations agent, own temporary [workspace]; joins after review
apps/
  linux/                      later: binary `tidemark` (GTK4 + libadwaita): ui/, shell/, services/
gnome-extension/tidemark@tidemark.app/   later: GJS adapter (shell-version 46..50)
packaging/debian/             later (parent): cargo-deb metadata, .desktop, autostart, icons
```

Rules:
- The `windows*` crates are `[target.'cfg(windows)'.dependencies]` of the root package.
- `cargo build --release` at the root still produces `target/release/win-island.exe` and `island-hook.exe`. `scripts/package.ps1` retains the Windows installer and ZIP; the release workflow also gates Ubuntu packaging on the Linux test matrix.
- Each portable module has **one source** in `tidemark-core`. The Windows crate keeps the old paths with `use tidemark_core::{…}` in `src/main.rs` (and `use tidemark_core::media_artwork as artwork` / `settings_motion as motion` where modules were included with `#[path]`).

### 3.1 What `tidemark-core` contains (milestone 1)

| Module | Origin | Notes |
|---|---|---|
| `anim`, `settings_motion`, `layout`, `timer`, `media_artwork` | moved unchanged | |
| `textinput`, `text_tools` | moved; layout swap now from `keyboard` | |
| `keyboard` | `EN`/`RU` tables + `swap_layout` from `launcher.rs` | `launcher` re-exports `swap_layout` |
| `config` | moved | `Config::dir()` = config dir, `Config::data_dir()` = data dir; `load_from`/`save_to` with explicit paths; missing file → defaults written; broken file → defaults, file left untouched; writes via `paths::write_atomic` (unique `create_new` sibling temp, rename, temp removed on error; Unix files 0600, newly created folders 0700) |
| `notes`, `clip_history` | moved | stored under the **data** dir; `load_from`/`save_to` |
| `paths` | new | Windows: `%APPDATA%\win-island` for both; Unix: `$XDG_CONFIG_HOME/tidemark` and `$XDG_DATA_HOME/tidemark` (relative values ignored), defaults `~/.config`, `~/.local/share`; `resolve(platform, env_fn)` makes it testable without touching real folders |
| `shell` | new | `ShellBackend` trait, `NullBackend`, `Capability`, `WindowRole`, `Rect`, `FocusInfo`, `WindowAction`, `WindowState`, `ShortcutId`, `ShortcutBinding`, `BindResult`, `ClipboardOffer`, `ClipboardData`, `ShellSignal`, `ShellError`, `SHELL_CONTRACT_VERSION = 1`; JSON wire names pinned by tests |
| `event` | new | `Event { shell, osd, service_error }`, `Osd`, `ServiceId` |
| `service` | new | `EventSink` (impl for `mpsc::Sender`/`SyncSender`), `Service { type Command; fn send }` |
| `agent_wire` | `Message` from `agent.rs` | + `Reply { choice }`; Windows `agent.rs` re-exports `Message` |

Not moved yet (still Windows-coupled): `theme` (registry), `lyrics`/`weather`/`translate` (WinHTTP + `PostMessageW`; parsers move with `tidemark-net`), `launcher` ranking/calculator (index thread posts `WM_APP`).

## 4. Module contracts

### 4.1 `tidemark-core` (owner: platform lead)

These modules are extracted from v1.3.0 and are pure today: `layout`, `anim`, `settings_motion`, `textinput`, `text_tools`, `timer`, `notes`, `clip_history` (model), `media_artwork`, `config` (schema). The core also takes:
- the parsers from `lyrics`, `weather` and `translate`;
- launcher ranking and the calculator;
- the agent wire protocol.

New shared types:

```rust
// Implemented in crates/tidemark-core/src/{event,shell}.rs (milestone 1):
pub enum Event { Shell { signal: ShellSignal }, Osd { osd: Osd }, ServiceError { service: ServiceId, message: String } }
pub enum ShortcutId { Launcher, ToggleTheme, LikeTrack }
pub enum Osd { Volume { level, muted }, Brightness { level }, Layout { code }, CapsLock { on } }
// Later milestones add variants for media, weather, lyrics, control, capture, agent, etc.
// Each one is a contract change owned by the platform lead, with a JSON test.
```

- `Config` is one superset schema. Keys that only make sense on one OS are ignored on the other.
- Linux paths: config in `$XDG_CONFIG_HOME/tidemark/config.toml`; data (`clipboard/`, `lyrics/`, `shelf.json`, notes) in `$XDG_DATA_HOME/tidemark/`.
- The wire protocol stays byte-compatible. It is one JSON line each way: `{"kind","source","title","text","options"}` → `{"choice":n|null}`.

### 4.2 Service contract (all Linux services)

```rust
pub trait Service { type Cmd; fn send(&self, cmd: Self::Cmd); }   // never blocks the UI
// spawn(tx: async_channel::Sender<Event>, cfg: &Config) -> impl Service
```

- One `std::thread` per service, sleeping on events (D-Bus signals, PipeWire loop, fds). No tokio and no polling when an event source exists, as on Windows.
- The UI drains the channel on the GLib main context. There is no shared mutable state across threads beyond `Arc<Mutex<_>>` snapshots that the service owns.
- Every external call has a timeout. A missing D-Bus service produces an `Event::Error`, never a panic.

### 4.3 `ShellBackend` trait ↔ D-Bus `org.tidemark.Shell1` (owner: lead specifies; adapter owner implements)

The adapter exposes it on the session bus at `/org/tidemark/Shell`. It answers **only the unique bus name of the client it launched** (or the caller whose process ID is the launched client's).

| Member | Kind | Purpose |
|---|---|---|
| `Version` (u), `Capabilities` (as) | property | feature negotiation; the Rust side degrades by capability |
| `Place(s role, (iiii) rect, a(iiii) input)` | method | position the `island`, `overlay` or `settings` window; keep-above, sticky, skip-taskbar, no focus-on-map; input region for click-through |
| `GetFocus() → a{sv}` | method | `id, app_id, title, pid, icon, maximized, above, opacity, rect, monitor` |
| `WindowAction(t id, s action, v arg)` | method | `activate, minimize, toggle_maximize, set_above(b), set_opacity(y), tile_left, tile_right` → returns the real resulting state |
| `GetClipboard(s mime) → h` / `SetClipboard(s mime, h fd)` | method | content passed only through file descriptors, never in signals |
| `Screenshot(b include_cursor, (iiii) area?) → h` | method | silent PNG via `Shell.Screenshot`; the island's window actor is hidden during capture |
| `SetCaptureHidden(b)` | method | hide the island's actor from screencasts while recording (best effort) |
| `BindShortcuts(a(ss) name_accel) → a(sb)` | method | `Main.wm.addKeybinding`; reports conflicts instead of overriding GNOME bindings |
| `FocusChanged`, `FullscreenChanged(u,b)`, `MaximizedTopChanged(b)` | signal | active window tile, hide in fullscreen, shrink under windows |
| `ClipboardChanged(as mimes, b sensitive)` | signal | `sensitive` = `x-kde-passwordManagerHint: secret` present |
| `ShortcutActivated(s)`, `LayoutChanged(s)`, `CapsLock(b)`, `Brightness(d)` | signal | hotkeys and on-screen notices |

- The adapter starts `tidemark` as a trusted Wayland client (§2.6 version shim) and restarts it on crash, with backoff.
- The Rust app also runs standalone. Without the adapter it uses `NullBackend` (Wayland) or `X11Backend` (Xorg) and lists the missing capabilities in Settings → System.
- The adapter must not implement UI, networking, persistence or product decisions.

### 4.4 Ownership (stable; one owner per path)

| Area | Paths | Owner role | Depends on |
|---|---|---|---|
| Workspace, core, net, contracts | root `Cargo.toml`/`Cargo.lock`, `crates/tidemark-core`, `crates/tidemark-net`, `docs/ubuntu/architecture.md` | Platform lead | — |
| CI workflow, packaging docs | `.github/workflows/ubuntu.yml`, packaging docs | Parent | — |
| MPRIS crate | `crates/tidemark-mpris/**` (own `TrackSnapshot` types; adapter converts later) | Integrations (GLM) | — |
| Windows app | `src/`, `build.rs`, `installer/`, `scripts/package.ps1` (repository root) | Platform lead only; no behaviour change while porting | core |
| Linux UI | `apps/linux/src/ui`, `apps/linux/src/main.rs` | Linux UI | core, ShellBackend |
| Compositor adapter | `gnome-extension/`, `apps/linux/src/shell/gnome.rs` | GNOME adapter | `Shell1` spec |
| X11 backend | `apps/linux/src/shell/x11.rs` | GNOME adapter | ShellBackend |
| Media, audio, system | `services/{mpris,audio,beat,privacy,netman,bluez,power,proxy,ytmusic_atspi}` | Integrations / services | core |
| Launcher, dock, clipboard, tray, agent socket | `services/{launcher,dock,clipboard,tray,agent_socket}`, `crates/island-hook` | Desktop services | core, ShellBackend |
| Capture, OCR, QR, recording | `services/{capture,ocr,record}`, `ui/overlay` | Capture | ShellBackend, portals |
| Packaging, QA matrix | `packaging/`, `docs/ubuntu/testing.md` | Packaging/QA | all |

Rules:
- A contract change (Event/Command enums, Config keys, `Shell1`) is a PR by the platform lead first; implementers then adapt.
- No implementer edits another owner's paths.
- Shared logic goes to `tidemark-core` with tests that run on both OSes.

## 5. Linux technology choices

| Concern | Choice | Why |
|---|---|---|
| GUI | `gtk4` (feature `v4_14`), `libadwaita` (`v1_5`) | native Wayland/X11, drag-and-drop, input methods, accessibility, HiDPI, portals; GNOME look for Settings |
| Island drawing | custom `GtkWidget`, `GtkSnapshot`/GSK paths + Pango, frame clock only while animating | replaces D2D/DirectComposition; idle = 0 CPU |
| D-Bus | `zbus` (blocking API on service threads) | MPRIS, NetworkManager, BlueZ, logind, Shell1, FileManager1 |
| Portals | `ashpd` | Screenshot, ScreenCast, FileChooser, Settings, OpenURI |
| Audio | `pipewire` crate (+ `libpulse-binding` only if needed for per-stream volume) | per-app volume, monitor capture, device-in-use |
| Recording | GStreamer (`gstreamer-rs`): `pipewiresrc` → `vah264enc`/`x264enc` + `pulsesrc`(monitor) → AAC → `mp4mux` | H.264 + AAC MP4 in `~/Videos`, like Windows |
| OCR | Tesseract 5 (`tesseract-ocr`, `-rus`, `-eng`) via the `leptess`/`tesseract` crate | offline, Russian + English |
| QR | `rqrr` (unchanged) | |
| HTTP | `ureq` + `rustls` | no OpenSSL runtime dependency |
| Accessibility (Yandex Music) | `atspi` crate | replaces Windows UI Automation |
| Tray | `ksni` (StatusNotifierItem) | works with Ubuntu's AppIndicator extension |
| X11 fallback | `x11rb` (EWMH, XShape, XFixes, XKB, XGrabKey) | Xorg session on 24.04 |

Excluded: WebView/Electron/Tauri, tokio, Flatpak/Snap packaging. The adapter, accessibility access and NetworkManager access need host integration, so packaging is `.deb`.

## 6. Feature parity matrix (v1.3.0, complete)

Classes:
- **N**: native Rust / D-Bus.
- **P**: xdg-desktop-portal.
- **A**: compositor adapter (Wayland).
- **X**: Xorg backend.
- **⚠**: known deviation or risk, listed in §8.

Every row must reach a verified ✅ in the test matrix before release; nothing ships as a silent stub.

### 6.1 Island shell and compact states

| v1.3.0 feature | Windows mechanism | Ubuntu plan | Class |
|---|---|---|---|
| Island window: transparent, rounded, click-through outside the shape, topmost, no taskbar, never steals focus | layered popup, `SetWindowRgn`, `WS_EX_NOACTIVATE`, DirectComposition | GTK window + input region; `Place` (A) / EWMH DOCK + `_NET_WM_STATE_ABOVE` + XShape (X) | A / X |
| Position at top centre of the primary monitor; follows monitor/DPI changes | `MonitorFromPoint`, per-monitor DPI v2 | `Place` with monitor geometry; GDK scale factor; below the top bar (§2.3) | A / X ⚠ |
| Spring animations, liquid/beat edge, hover expand with `hover_delay_ms`, click to expand | D2D path geometry | GSK path + frame clock; same `anim` code | N |
| Clock + weather in the pill | DirectWrite, Open-Meteo → wttr.in fallback | Pango; same parsers through `tidemark-net` | N |
| Collapsed media pill (cover, accent edge) | SMTC | MPRIS | N |
| Toasts: "Скопировано", app switch, agent cards | own rendering | same | N |
| Volume OSD | `IAudioEndpointVolume` callback | PipeWire default-sink volume/mute events | N |
| Brightness OSD | `GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS` | adapter `Brightness` signal; X: backlight via `org.gnome.SettingsDaemon.Power` | A / X |
| Keyboard layout + Caps Lock OSD | raw input + `GetKeyboardLayout` | adapter `LayoutChanged` / `CapsLock`; X: XKB events | A / X |
| Camera/microphone in-use dot + notice | ConsentStore registry | PipeWire registry: running `Video/Source`/`Audio/Source` nodes and the client `application.name` | N |
| Active-app switch notice with icon | `SetWinEventHook` foreground | adapter `FocusChanged` + icon theme; X: `_NET_ACTIVE_WINDOW` | A / X |
| Hide in fullscreen (`hide_in_fullscreen`) + reveal at top edge | appbar `ABN_FULLSCREENAPP`, cursor poll | adapter `FullscreenChanged`; reveal via a 1-px input strip; X: `_NET_WM_STATE_FULLSCREEN` | A / X |
| Shrink to a strip under maximized windows (`shrink_under_windows`) | foreground rect check | adapter `MaximizedTopChanged`; X: EWMH geometry | A / X |
| Timer "bristle" edge, countdown in the pill | own | same | N |
| Island excluded from screenshots/recordings | `WDA_EXCLUDEFROMCAPTURE` | screenshots: the adapter hides the actor; recording: `SetCaptureHidden` best effort, otherwise the island hides while recording | A ⚠ |
| Animation stats (`WIN_ISLAND_STATS`) | `GetProcessTimes` | `getrusage`, log in `$XDG_STATE_HOME` | N |

### 6.2 Home tab tiles (`home_tiles`: media, clipboard, window, tools, weather, apps)

| Tile / control | Ubuntu plan | Class |
|---|---|---|
| Media: cover, title/artist, progress | MPRIS `Metadata` (`mpris:artUrl` file/http, bounded retry like `media_artwork`) | N |
| Media: prev / play-pause / next | MPRIS `Previous/PlayPause/Next` | N |
| Media: click progress bar to seek | MPRIS `SetPosition` (only when `CanSeek`) | N |
| Media: click text to copy "artist — title" | GTK clipboard (island has focus at click) | N |
| Media: per-app volume slider, speaker = mute/restore | PipeWire stream volume matched by MPRIS PID / `application.process.binary` | N ⚠ (browser streams share one PID) |
| Media: heart = Yandex Music like, `Ctrl+Alt+L` | AT-SPI2 in Chrome/Chromium: find the "Мне нравится" button, verify title/artist, invoke; restore the previous tab | N ⚠ |
| Media: click cover = open the Yandex Music tab | AT-SPI tab selection + adapter `WindowAction activate` | N + A ⚠ |
| Media context menu: open Yandex Music, copy, sleep timer 15/30/60 min, cancel | core timer + MPRIS `Pause` for the original bus name | N |
| Media from Winamp/AIMP | Not applicable on Linux; equivalent Linux players (Audacious, Strawberry, etc.) publish MPRIS | N (equivalent) |
| Clipboard tile: recent entries, click = paste back | clipboard service | A / X |
| Window tile: title (click = activate), keep on top, opacity 100→85→70, minimize | adapter `WindowAction`; actual state read back | A / X |
| Window menu: open, maximize/restore, left/right half, pin to dock, copy title, show executable | adapter (first four); dock + clipboard (N); `/proc/<pid>/exe` → `org.freedesktop.FileManager1.ShowItems` | A + N |
| Tools tile: 8 buttons | see §6.4 | |
| Weather tile | net + core | N |
| Apps (dock): pinned apps with icons, launch, "+" picker, remove from dock | `.desktop` entries / executables; `GDesktopAppInfo` launch; icon theme; picker = app list + FileChooser portal | N / P |

### 6.3 Tabs

| Tab | Features | Ubuntu plan | Class |
|---|---|---|---|
| Главная (Home) | skyline tile layout, tiles toggled in settings | core `layout` | N |
| Буфер (Clipboard) | history of text/image/files, de-dup, cap, pinned entries survive clearing, password-manager exclusion, menu: pin/unpin, remove, clear unpinned, show file in folder | `ClipboardChanged` → `GetClipboard` (A) / XFixes (X); `sensitive` = do not record; model `clip_history`; images as PNG; reveal via FileManager1 | A / X + N |
| Заметки (Notes) | scratch note, Russian-phrase reminders, timer presets/toggle/reset, reminder removal | core `notes`/`timer`; GLib timeout at the deadline | N |
| Полка (Shelf) | drop files on the island, drag out to any app, remove/clear | GTK4 `DropTarget`/`DragSource` (`text/uri-list`, `GdkFileList`) | N |
| Текст (Text) | 12 transforms (layout fix, case ×4, translit, join lines, spaces, JSON pretty/compact, Base64 to/from), translate ru↔en, copy | core `text_tools` + net translate | N |
| Управление (Control) | Wi-Fi list/connect/password/disconnect; Bluetooth paired connect/disconnect; VPN connect/disconnect; Throne status, system proxy toggle, open Throne; refresh; open system settings; power: lock, sleep, restart, shut down | NetworkManager D-Bus (Wi-Fi, VPN/WireGuard); BlueZ D-Bus; Throne: process + TUN via netlink + `org.gnome.system.proxy` gsettings; `gnome-control-center`; lock `org.gnome.ScreenSaver.Lock`, sleep/restart/off via `org.gnome.SessionManager` / logind (polkit) | N |
| Поиск (Search, Alt+Space) | apps, recent files, files and folders in Desktop/Documents/Downloads, calculator, web search (`search_url`), keyboard nav, Ctrl+Enter variant | `.desktop` apps, `recently-used.xbel`, XDG user dirs; index on a thread, refreshed ≤ 10 min; hotkey via `BindShortcuts` (A) / XGrabKey (X) | N + A / X ⚠ |

### 6.4 Tools (Tools tile, `layout::TOOLS`)

| Tool | Ubuntu plan | Class |
|---|---|---|
| Область (area screenshot + editor: arrows, boxes, text, pixelate, undo, copy) | adapter `Screenshot` (silent, no island) → fullscreen GTK overlay over the frozen image on each monitor → PNG to clipboard; fallback Screenshot portal (interactive) | A / P |
| Весь экран (full screen) | same, no selection | A / P |
| Текст (OCR) | overlay selection → Tesseract rus+eng → text to the Text tab / clipboard | A + N ⚠ |
| QR-код | overlay selection → `rqrr` | A + N |
| Пипетка (eyedropper with magnifier) | from the frozen screenshot; fallback portal `PickColor` | A / P |
| Без формата (plain text) | read clipboard text → write it back as `text/plain` only | A / X |
| Команда (opens Search) | same as Alt+Space | N |
| Запись (record screen + system sound, MP4) | ScreenCast portal (monitor, cursor embedded, `persist_mode` restore token) + PipeWire monitor audio → GStreamer MP4 in `~/Videos` | P ⚠ (first-run consent dialog) |

### 6.5 App-level, settings, integrations

| Feature | Ubuntu plan | Class |
|---|---|---|
| Single instance; `--settings` opens Settings in the running instance | `GApplication` unique name `app.tidemark.Tidemark` + action | N |
| Tray: Settings…, open config.toml, reload config.toml, start with system (check), exit | `ksni` StatusNotifierItem | N |
| Settings, 4 sections: theme (system/dark/light) with preview, accent, background colour, opacity, hover delay, home tiles, behaviour toggles (active app, OSD, privacy, launcher, beat edge, clipboard history, GPU), weather city, search URL, autostart, open config / reset, Apply without restart | libadwaita `PreferencesDialog`-style window (4 pages), live apply; "GPU rendering" maps to GSK renderer choice; adds a System page listing missing capabilities | N |
| System theme following, accent | Settings portal `color-scheme`; `accent-color` on GNOME ≥ 47, config accent on 46 | P |
| Hotkeys: Alt+Space search, Ctrl+Alt+T theme, Ctrl+Alt+L like | `BindShortcuts`; on conflict, show it in Settings and suggest an alternative; Tidemark never silently overrides GNOME bindings | A / X ⚠ |
| Autostart | `~/.config/autostart/tidemark.desktop` | N |
| Config migration (Yandex → Google search URL etc.) | core `config` (shared) | N |
| Claude Code hooks (PermissionRequest, AskUserQuestion, Notification, Stop) | `island-hook` Linux build; Unix socket `$XDG_RUNTIME_DIR/tidemark/agent.sock`, mode 0600, `SO_PEERCRED` same-UID check; `integrations/claude-code/hooks.template.json` with Linux path | N |
| opencode plugin | same plugin; `TIDEMARK_HOOK` / `WIN_ISLAND_HOOK` env var; default path per OS | N |
| Wallpapers and brand assets | shipped in `/usr/share/tidemark` | N |
| Install / uninstall | `.deb` (cargo-deb): binaries, `.desktop`, icons, adapter in `/usr/share/gnome-shell/extensions/`; user enables it after re-login (Wayland) | N |

## 7. Data flow

```
GNOME Shell ── adapter (GJS) ──Shell1 D-Bus──┐
Xorg ── x11rb ───────────────────────────────┤ ShellBackend
MPRIS / NM / BlueZ / logind / PipeWire / portals / AT-SPI ── service threads
                                             └─► async_channel<Event> ─► GLib main loop ─► Island state (core) ─► GSK draw
UI commands ─► Service::send / ShellBackend calls (never blocking the main loop)
```

## 8. Genuine blockers and deviations (must be resolved or explicitly accepted)

1. **GNOME's top bar covers the top edge.** The island sits below the bar. This is a visible deviation from Windows that the user has to accept.
2. **The compositor adapter is mandatory for full parity on Wayland.** Without it, these are unavailable: clipboard history, plain text, active-window tile and actions, fullscreen/maximized behaviour, layout and Caps Lock notices, global hotkeys on GNOME 46/47, and silent screenshots. The `.deb` installs it, but GNOME requires a re-login and the user's enablement. Extension API breaks per GNOME release require a maintained shim.
3. **Yandex Music like via AT-SPI is unverified.** Chromium exposes its accessibility tree only when assistive technology is active. That needs `toolkit-accessibility`, or launching the browser with `--force-renderer-accessibility`. The button labels must match the current web UI. Until this is verified on 24.04 and 26.04 with Chrome, the feature is **not** at parity.
4. **Self-exclusion from recordings has no Wayland API.** The fallback is hiding the island while recording.
5. **Screen recording consent.** The ScreenCast portal shows a dialog on first use; the restore token suppresses it afterwards. Windows has no dialog.
6. **OCR engine differs.** Tesseract replaces Windows OCR, so accuracy differs. It is accepted only if the gate in §9 passes.
7. **Shortcut conflicts.** Alt+Space and Ctrl+Alt+T are taken on Ubuntu. Product decision needed: keep the defaults with a conflict prompt, or use Linux-specific defaults.
8. **Per-app volume for browsers.** All tabs share one audio stream owner, so the speaker mutes the browser's audio stream rather than one tab (Windows has the same per-process limitation).
9. **Winamp/AIMP IPC has no Linux equivalent.** Covered by MPRIS players. This is a documented replacement, not a gap.
10. **Power actions** depend on polkit policy. On default Ubuntu desktops the active local user can suspend, reboot and power off without a password.

## 9. Release acceptance gates

1. **Windows unchanged.**
   - `cargo build --release` and `cargo test --release` pass on `windows-latest`.
   - The v1.3.0 manual smoke list passes: island, media, clipboard, tools, recording, control, search, settings, hook.
   - The installer and ZIP build.
2. **Linux CI.** On `ubuntu-24.04`, these pass: `cargo build --release`, `cargo test --workspace`, `cargo clippy -D warnings`, and an extension lint (`eslint` + GJS import check).
3. **Contract tests.**
   - A `gdbus`-driven test of every `Shell1` method and signal on GNOME 46 and 50 (nested `gnome-shell --devkit`/`--nested` session).
   - Agent protocol round-trip tests shared with Windows.
4. **Manual matrix.** Every row of §6 is marked ✅ / ⚠ (accepted deviation) / ❌ in `docs/ubuntu/testing.md` for:
   - 24.04 Wayland with the adapter;
   - 24.04 Xorg;
   - 24.04 Wayland without the adapter (graceful degradation: no crash, missing features listed);
   - 26.04 Wayland with the adapter.

   Release requires **no ❌** on 24.04 Wayland + adapter and 26.04 Wayland + adapter.
5. **Budget** (same spirit as `PLAN.md`), with the island collapsed:
   - idle CPU ≈ 0 (no wakeups over 10 s);
   - idle private memory < 60 MB including GTK;
   - startup < 500 ms;
   - no frame-clock activity when nothing animates.
6. **Security.**
   - Agent socket 0600 + UID check.
   - `Shell1` answers only the launched client.
   - Clipboard content never travels in a broadcast signal.
   - Password-manager-marked entries are never stored.
7. **Packaging.**
   - `.deb` installs and removes cleanly on fresh 24.04 and 26.04 VMs.
   - Autostart works.
   - The tray appears.
   - The adapter shows in Extensions after re-login.
8. **Honesty gate.** The README/CHANGELOG for Linux lists every ⚠ deviation. No feature is advertised that hasn't passed the matrix.

## 10. Implementation order

0. Workspace split (lead). Gate 1 must stay green.
1. `tidemark-core` extraction and the contracts: Event/Command, Config, `Shell1` XML, ShellBackend trait (lead).
2. In parallel: adapter skeleton + `Place`/focus/fullscreen (adapter); GTK island shell with collapse/expand and Home layout (UI); MPRIS, weather, lyrics (services).
3. Clipboard, launcher, dock, shelf, tray, settings, autostart, agent socket.
4. Control tab: NetworkManager, BlueZ, logind, proxy/Throne.
5. Capture overlay, OCR, QR, eyedropper, recording.
6. Notices and privacy, Yandex Music via AT-SPI, X11 backend.
7. Packaging, the full matrix, gates 1–8.

## 11. Primary sources

- GNOME Shell extension porting guides: [GNOME 49](https://gjs.guide/extensions/upgrading/gnome-shell-49.html), [GNOME 48](https://gjs.guide/extensions/upgrading/gnome-shell-48.html)
- Mutter API, `Meta.WaylandClient`: <https://mutter.gnome.org/meta/class.WaylandClient.html>
- GlobalShortcuts portal: <https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html>; GNOME implementation: [This Week in GNOME #189](https://thisweek.gnome.org/posts/2025/02/twig-189)
- xdg-desktop-portal documentation (Screenshot, ScreenCast, Settings, FileChooser): <https://flatpak.github.io/xdg-desktop-portal/docs/>
- MPRIS D-Bus specification: <https://specifications.freedesktop.org/mpris-spec/latest/>
- NetworkManager D-Bus API: <https://networkmanager.dev/docs/api/latest/>
- BlueZ D-Bus API: <https://git.kernel.org/pub/scm/bluetooth/bluez.git/tree/doc>
- systemd-logind D-Bus API: <https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.login1.html>
- StatusNotifierItem: <https://www.freedesktop.org/wiki/Specifications/StatusNotifierItem/>
- Desktop Entry / autostart / icon theme specs: <https://specifications.freedesktop.org/>
- GNOME 50 removes the X11 session: [The Register](https://www.theregister.com/2026/03/19/gnome_50/); Ubuntu 26.04 release: [OMG! Ubuntu](https://www.omgubuntu.co.uk/2026/04/ubuntu-26-04-lts-released?noamp=mobile)
- gtk4-rs: <https://gtk-rs.org/gtk4-rs/stable/latest/docs/gtk4/>; libadwaita-rs: <https://world.pages.gitlab.gnome.org/Rust/libadwaita-rs/>

Facts that change between GNOME versions (§2.4–2.6) must be checked again against these sources when the adapter is implemented and on every new GNOME release.
