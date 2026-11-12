# Ubuntu 24.04 GNOME — integration contracts (read-only research, v1.3.0 → Linux port)

Baseline: Ubuntu 24.04 LTS (GNOME 46) amd64; compatibility target 26.04 LTS (Resolute Raccoon,
released 23 Apr 2026): **GNOME 50, which removed the Xorg/X11 session** (source: Ubuntu release
history, https://en.wikipedia.org/wiki/Ubuntu_version_history#Ubuntu_26.04_LTS_(Resolute_Raccoon) —
verified; upstream GNOME release notes should be double-checked at implementation time).

**Parity statuses below are targets from design analysis, not implemented or proven behavior.**

**Session impact of GNOME 50**: the X11 fallback backend (§0) exists only as a 24.04-era option;
on 26.04 only the Wayland + GNOME Shell extension path exists. Design the bridge as the primary
contract from day one.
Windows sources referenced by file; every contract below states what the Windows build does, the Linux
replacement, and parity. Verified = confirmed against the source file listed (or primary doc linked);
UNVERIFIED = assumption to check during implementation.

## 0. Session model (the core fork)

The island on Windows is one Win32 process that is: a top-edge always-on-top overlay
(`WS_EX_TOPMOST|WS_EX_NOACTIVATE|WS_EX_NOREDIRECTIONBITMAP`, island.rs:361), a window manager
assistant for other apps (active_app.rs), and a system-event hub (signals.rs).

On Wayland a plain client can be none of those: no global coordinates, no z-order, no other
windows' metadata, no global hotkeys without portal. Contract: split into

- **Core daemon** — pure Rust, session-agnostic: MPRIS, PipeWire/PulseAudio, D-Bus portals,
  config (config.toml port), notes/timers/weather/launcher logic. No compositor calls.
- **Compositor bridge (GNOME Shell extension, GJS)** — required on Wayland for:
  overlay placement/always-on-top at top edge (Mutter has **no `zwlr_layer_shell`** — UNVERIFIED for
  current GNOME versions, but historically true for Mutter), window actions on other apps,
  clipboard-history capture, fullscreen detection, Caps Lock/keyboard-layout display, tray support.
- **X11 fallback backend** — only relevant on 24.04 (GNOME 46 offers an X11 session): full parity
  possible via above-window + `_NET_WM_STATE_ABOVE`, opacity `_NET_WM_WINDOW_OPACITY`, `XGrabKey`
  hotkeys, `XGetImage`/`XShm` screenshots, clipboard-manager style client, EWMH events for foreground
  tracking. **GNOME 50 on 26.04 removed the X11 session** — no X11 fallback there (verified).

Parity decision point: implement the bridge first on Wayland; X11 backend only as a 24.04-era
option if the extension path falls short (26.04/GNOME 50 removed the X11 session, so no X11 path
there — verified). GNOME Shell extension API broke at GNOME 45 (ESM); pin to 46 API and test on 49/50
(UNVERIFIED exact breaking changes in 47–50).

## 1. Music "now playing" + artwork — MPRIS2

Windows (src/media.rs): SMTC push events → `MediaState{title,artist,playing,position,duration,
can_prev,next,seek,cover,app_id}`; cover decoded once via WIC to 160×160 BGRA (`COVER_PX`), late-cover
retries at 250/500/1000/2000 ms (src/media_artwork.rs, `ArtworkSchedule` — portable logic + tests).

Contract — subscribe D-Bus (session bus) to all names matching `org.mpris.MediaPlayer2.*`:

| SMTC field | MPRIS equivalent |
|---|---|
| title/artist/album | `Metadata` map: `xesam:title`, `xesam:artist` (array), `xesam:album` |
| cover | `Metadata["mpris:artUrl"]` (file:// or data: URI; may be absent/late) |
| playing | `PlaybackStatus` ∈ {Playing, Paused, Stopped} |
| position/duration | `Position` (µs) + `Seeked` signal; duration from `Metadata["mpris:length"]` |
| can_prev/next/seek | `CanGoPrevious`, `CanGoNext`, `CanSeek` properties |
| play/pause/next/prev | methods `PlayPause`, `Next`, `Previous` |
| seek | `SetPosition(trackid, position)` |
| session identity | unique bus name (pins the sleep timer to one player, media.rs:196-245) |

Parity: full. Choose the "playing" bus with the newest change; keep `ArtworkSchedule` unchanged.
Docs: https://specifications.freedesktop.org/mpris-spec/latest/ (verified spec).

Gaps/risks:
- `mpris:artUrl` is commonly an **HTTP(S)** URL (streaming players), and may also be `file://` or
  `data:`; it may be absent/late. Fetch contract (bounded, no shell):
  - allowed schemes only: `https`, `http`, `file`, `data` — reject everything else;
  - HTTP(S): plain GET, no cookies, no stored credentials, no proxy credentials; do not follow
    redirects to a different scheme; abort on non-2xx;
  - hard byte limit **8 MiB** (same cap as Windows `read_cover`, media.rs:392) and a fetch timeout
    (suggested ≤ 5 s, consistent with `ArtworkSchedule` retry cadence);
  - content must be decoded by an image decoder (image/codec crate — Windows parity of WIC) sized
    and clipped exactly like `decode_cover` (centered square, 160×160 BGRA);
  - `file://` only inside XDG data/cache dirs if artifact provenance matters; simplest contract:
    accept any local path the player already published;
  - no arbitrary shell command execution, no URL rewriting, never forward user credentials;
  - failures flow into the existing `ArtworkSchedule` retry logic.
- `Position` does not auto-advance; extrapolate from `sampled_at` exactly like `position_now()` (media.rs:64).
- Player crash leaves a stale bus name — handle `NameOwnerChanged` removal.- Winamp/AIMP legacy polling (src/legacy_players.rs) — **dropped**, no Linux analogue.

## 2. Per-app volume + master volume/brightness — PipeWire/PulseAudio

Windows: `audio::app_volume()/set_app_volume()` per audio session (media.rs:331, 214); master volume
callback `IAudioEndpointVolumeCallback` (src/signals.rs); brightness via power notification.

Contract (session-agnostic, works Wayland and X11):
- Per-stream: PulseAudio (pactl/PulseAudio Rust crate) or PipeWire API; Ubuntu 24.04 runs PipeWire
  with the PulseAudio compat layer. Enumerate stream nodes, read/set node volume, subscribe to changes.
- Master volume/mute: `org.PulseAudio.Core1` (D-Bus) or Pulse API server callback — parity full.
- Brightness: logind D-Bus `SetBrightness` + `org.freedesktop.UPower`/backlight monitor — parity full
  (signals.rs brightness notification).
- Keyboard-layout indicator (signals.rs layout): `org.gnome.desktop.input-sources` gsettings +
  `input-sources` changes via D-Bus — parity full. Caps Lock: **needs compositor bridge on Wayland**
  (no global keyboard state for clients); on X11 use XKB events.

Docs: https://docs.pipewire.org/ (verified); GNOME gsettings key UNVERIFIED semantics for layout name strings.

## 3. Real Yandex Music likes — AT-SPI2

Windows (src/yandex_music.rs): UI Automation walks browser/standalone windows; verifies service URL
(https only, exact host list music.yandex.{ru,com,by,kz}, trusted_music_url yandex_music.rs:186-210);
finds "Плеер" group matching exact title+artist; single «Нравится» toggle button (`TogglePattern`);
double read-before-write; restores original tab and minimized state (RAII guards).

Contract — AT-SPI2 (`org.a11y.Bus`, client via `atspi` Rust crate or libatspi):
- Find window/tab: same algorithm on `Accessible` tree (application → frame → tab).
- Tab activation: `SelectionItem.Select()` on tab item — same contract as Windows, no focus theft needed.
- URL verification: Windows reads Document `Value`. UNVERIFIED that Chromium-on-Linux exposes the URL
  on the document accessible; fallback contract: verify a hyperlink/landmark or accept the same
  branded-tab-title + track match without URL check (would be a security downgrade — must decide).
- Toggle button: `State` (ACTIVE) + `Action` interface (`DoAction`), verify via state re-read.
- Timeouts: mirror `SetConnectionTimeout(700)`/`SetTransactionTimeout(1000)` as explicit async deadlines.

Risks:
1. **Browsers disable accessibility by default on Linux.** Chromium enables AT-SPI only when an
   assistive client is present; may need `GNOME_ACCESSIBILITY=1`/`ACCESSIBILITY=1` in the user env or
   instructing the user (UNVERIFIED current Chromium behavior). Firefox exposes a11y via GTK.
2. Under Wayland, list of toplevels is not directly available to a plain client — enumerate the a11y
   desktop tree instead (works: AT-SPI desktop is compositor-side). UNVERIFIED that Chromium publishes
   tab items under GNOME Wayland identically to X11.
3. No `SetForegroundWindow` analogue on Wayland — use `xdg-activation-v1` portal for the "open player"
   action; verify per-compositor support (UNVERIFIED for Mutter 46).

## 4. Clipboard history — files/images + GNOME bridge

Windows (src/clipboard.rs, clip_history.rs): `AddClipboardFormatListener` → CF_UNICODETEXT / CF_DIB
(→ PNG via WIC) / CF_HDROP; respects password-manager exclusion markers
(`ExcludeClipboardContentFromMonitorProcessing`, `Clipboard Viewer Ignore`, `CanIncludeInClipboardHistory`,
clipboard.rs:65-78); write-back of text/files/DIB.

Contract:
- **Under Wayland GNOME, a background client cannot observe clipboard changes** (focus-gated paste
  protocol only). Two options, pick one:
  a) **GNOME Shell extension bridge** owns the seat clipboard (`St.Clipboard`/`Gdk` access inside Shell
     or Meta.later polling of `gtk clipboard`) and pushes entries to the daemon over D-Bus —
     UNVERIFIED exact Shell API surface for clipboard access inside extensions; check GPaste and
     "Clipboard Indicator" extensions for the working pattern.
  b) **X11 session**: daemon registers as clipboard manager (`x11 clipboard-manager` protocol) — full parity.
- Formats: text and images map 1:1 (image → PNG in history dir, clip_history.rs naming kept).
  Files: no CF_HDROP analogue — carry `text/uri-list` (GIO URIs) as the Files entry; write-back of
  files → set `text/uri-list` (parity partial: apps that only accept text/x-moz-uri style lists are
  out of scope). UNVERIFIED: GIO file copy support in Nautilus via uri-list — standard, likely fine.
- **Secret-exclusion markers have no Linux convention** — known privacy regression; mitigation:
  ignore passwords pasted from org.freedesktop.secrets-backed managers is not detectable → document as gap.
- GNOME 46+ removed legacy tray/X11-only paths; GPaste (extension) is the closest proven implementation
  (UNVERIFIED GNOME 46 compatibility).

## 5. Screenshots / OCR / QR / recording — portals

Windows: virtual-screen `BitBlt` grab with crop/pixelate (src/capture.rs); OCR via Windows.Media.Ocr
(user-profile languages, background thread, src/recognize.rs); QR via `rqrr` (portable — keep as-is);
recording via Windows.Graphics.Capture + WASAPI loopback + Media Foundation H.264/AAC mp4 (src/record.rs).

Contracts:
- **Still screenshot**: XDG Desktop Portal `org.freedesktop.portal.Screenshot` (GNOME implements).
  Parity: full for the "grab whole screen/monitor" flow; the interactive crop happens in our own UI
  window over the saved image, as on Windows. On X11: `XGetImage`/`XShmGetImage` of the virtual screen.
  Portal docs: https://flatpak.github.io/xdg-desktop-portal/doc/org.freedesktop.portal.Screenshot.html (verified).
  Risk: portal may show a consent/notification per shot (GNOME shows a notification; silent grabs are
  possible when pre-authorized — UNVERIFIED which GNOME versions allow non-interactive screenshots).
- **Screen recording**: `org.freedesktop.portal.ScreenCast` → PipeWire video stream; encode with
  GStreamer (x264enc + audio) to MP4 in `~/Videos` (record.rs new_path parity). System sound:
  PipeWire loopback of the default sink (pw-record target semantics). Portal requires a **user consent
  dialog per session/monitor** — the one-tap flow becomes tap+confirm (behavioral gap; unavoidable on
  Wayland). Cursor capture: portal `cursor_mode` option (parity with Windows cursor-inclusive, record.rs:2).
- **OCR**: no OS-bundled OCR on Linux. Contract: **tesseract** (libtesseract) with language data from
  `tesseract-ocr-rus`/`-eng` packages; keep the upscale-before-OCR heuristic (recognize.rs:30).
  Gap: needs package installation at runtime or a bundled data dir; output quality differs from
  Windows OCR (UNVERIFIED parity for Cyrillic UI screenshots).
- **QR**: `rqrr` unchanged (recognize.rs:55-60, tests portable).

## 6. Network / shell controls (Wi‑Fi, Bluetooth, VPN, volume tile)

Contract (session-agnostic):
- Wi‑Fi: NetworkManager D-Bus (`org.freedesktop.NetworkManager`: devices, wireless enable, AP list,
  ActivateConnection). Docs: https://networkmanager.dev/docs/api/latest/ (verified).
- Bluetooth: BlueZ D-Bus `org.bluez` (devices, connect/disconnect). Docs: https://git.kernel.org/pub/scm/bluetooth/bluez.git/tree/doc (verified).
- VPN: NetworkManager VPN active connections (same API). UNVERIFIED: third-party VPN CLIs (as on
  Windows some VPNs are shell-driven).
- Master volume tile: PulseAudio/PipeWire (§2).
- Power/reboot/shutdown: logind D-Bus (`org.freedesktop.login1.Manager.PowerOff/Reboot`) — replaces
  Win32 shutdown calls; parity full.

## 7. Window controls for other apps (active window card)

Windows (src/active_app.rs + island.rs): topmost toggle (`SetWindowPos HWND_TOPMOST`,
active_app.rs:173-184), opacity cycle 100→85→70→100 (`WS_EX_LAYERED` + `SetLayeredWindowAttributes`,
active_app.rs:203-236), minimize, place left/right (monitor-aware `SetWindowPos`, active_app.rs:330),
pin to dock, copy title, foreground tracking via `SetWinEventHook`, fullscreen via appbar
`ABN_FULLSCREENAPP` (src/fullscreen.rs), exe/friendly-name via version resource (active_app.rs:59-104).

Contract:
- **Wayland GNOME: all of these require the GNOME Shell extension.** The extension (or Mutters
  private `org.gnome.Shell.Extensions` eval — not allowed) exposes over D-Bus:
  `windowList()` → {id, title, wm_class, pid, is_fullscreen}, `setAlwaysOnTop(id, bool)`,
  `setOpacity(id, 0-100)` (Meta.WindowActor.set_opacity), `minimize(id)`, `place(id, 'left'|'right')`,
  `activate(id)`, `activeWindow()` change notifications, `fullscreenChanged` events.
  Each method above has a direct Meta API inside Shell (verified pattern used by window-management
  extensions; UNVERIFIED in-repo — needs a probe).
- **X11**: EWMH client messages for all of the above + `_NET_ACTIVE_WINDOW` tracking +
  `_NET_WM_STATE_ABOVE`, `_NET_WM_WINDOW_OPACITY` — full parity without an extension.
- App name/icon: `.desktop` lookup from `wm_class` via GIO `AppInfo` (replaces the version-resource
  FileDescription); icon extraction from icon theme. Parity full.

## 8. Island surface, tray, hotkeys, autostart

- **Overlay window**: Wayland — anchored at top edge requires the Shell extension to host/position it
  (no layer-shell in Mutter) or be drawn as a Shell actor (extension UI in GJS/Clutter — a bigger
  rewrite). Contract choice: extension creates a normal GTK window, keeps it above and
  non-focusable, and reports monitor work-area; daemon draws via existing Rust renderer is NOT
  possible in GJS — so the recommended shape is: extension positions/keeps-above a daemon-owned
  window using its privileged API. UNVERIFIED whether a non-activatable window can stay reliably
  above on Wayland GNOME — highest architectural risk of the port.
  X11: override-redirect/always-above window at top edge — direct port of island.rs:574/637.
- **Tray**: Windows tray icon → `StatusNotifierItem` (SNI/DBMenu). GNOME needs the
  `AppIndicator` extension (Ubuntu ships `gnome-shell-extension-appindicator`; UNVERIFIED whether
  preinstalled/enabled on the default 24.04 image — historically Ubuntu enables it).
- **Global hotkeys**: `RegisterHotKey` (island.rs:513-516: Ctrl+Alt+T, Ctrl+Alt+L, Alt+Space) →
  XDG GlobalShortcuts portal (`org.freedesktop.portal.GlobalShortcuts`, GNOME 45+). Parity full,
  but shortcuts are registered with a one-time user confirmation dialog per shortcut
  (UNVERIFIED GNOME 46 portal UI). X11 fallback: `XGrabKey`.
- **Autostart**: HKCU Run key → XDG autostart `.desktop` in `~/.config/autostart` (parity full,
  src/autostart.rs is trivial to port).

## 9. Parity summary

All statuses are **design targets**, not implemented or proven behavior (see header note).

| Feature | Wayland/GNOME 24.04 | X11 (24.04 only; removed in GNOME 50) |
|---|---|---|
| Media card, artwork, controls | MPRIS — full | full |
| Per-app/master volume, brightness | full | full |
| Yandex likes (AT-SPI) | full if browser a11y enabled; URL check UNVERIFIED | full (a11y same) |
| Clipboard history + write-back | requires Shell bridge; no secret markers | full; no secret markers |
| Screenshot + crop editor | portal (consent may appear) | full, silent |
| Recording + system sound | ScreenCast portal + PipeWire (consent dialog) | full via X11 capture |
| OCR | tesseract (external dep) | same |
| QR | rqrr — full | full |
| Window card: pin/opacity/place/restore | requires Shell extension | full (EWMH) |
| Fullscreen hide | requires Shell extension | full |
| Island overlay at top edge | requires Shell extension (risk: no layer-shell) | full |
| Tray icon | SNI + AppIndicator extension | SNI + extension |
| Global hotkeys | GlobalShortcuts portal (dialog) | XGrabKey |
| Wi-Fi/BT/VPN, power, layout, autostart | full (D-Bus) | full |

## 10. Unverified items to probe first

1. Chromium/Firefox AT-SPI exposure defaults and Document URL accessibility on Wayland (§3).
2. GNOME Shell clipboard API usable by an extension for history capture (§4).
3. Non-interactive portal screenshot/screen-cast authorization persistence (§5).
4. Mutter: absence of layer-shell and reliability of an extension-kept-above, non-focusable window (§0/§8).
5. AppIndicator extension presence by default on Ubuntu 24.04 (§8).
6. GlobalShortcuts portal behavior on GNOME 46 (§8).
7. Tesseract quality/packaging for the OCR editor flow (§5).
8. GNOME 50 (26.04) Shell extension API changes vs 46 — header's GNOME 50/X11-removal fact is
   verified via Wikipedia release history; exact extension-API breakage between 46 and 50 is not.
