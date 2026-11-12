# Tidemark GNOME Shell adapter

`tidemark@treyzar.github.io/` is a GNOME Shell extension for GNOME 46–50 (Ubuntu 24.04 and 26.04).

It does only the things Mutter does not let a Wayland client do:
- keep the island above other windows, below the top bar;
- report the active window, fullscreen and maximized state;
- act on other windows;
- grab global shortcuts;
- read the clipboard while unfocused;
- report keyboard layout and Caps Lock.

It exposes these as the D-Bus interface `org.tidemark.Shell1` (bus name `org.tidemark.Shell`, path `/org/tidemark/Shell`) to the Tidemark process it launched itself. Product logic and UI live in the Rust app (`apps/linux`, GTK application id `io.github.treyzar.Tidemark`, binary `/usr/bin/tidemark`).

**Status: development version with real headless GNOME coverage.** The isolated integration harness has passed on Ubuntu 24.04 (Mutter 46) and 26.04 (Mutter 50), including authenticated launch, placement, clipboard, window actions, shortcuts and crash recovery. Physical desktop acceptance below is still required; headless checks do not prove multi-monitor input routing or the real browser integration.

## Files

| File | Role |
|---|---|
| `org.tidemark.Shell1.xml` | the D-Bus contract; names mirror `crates/tidemark-core/src/shell.rs` |
| `contract.js` | constants and pure helpers (also loaded by the Node tests) |
| `adapter.js` | all decisions: client lifecycle, authentication, roles, placement, tracking, actions, shortcuts, clipboard privacy; no `gi://` imports |
| `gnomePlatform.js` | Mutter/Shell calls and signal wiring, including the GNOME 46–48 / 49+ differences |
| `dbusService.js` | export, peer-PID authentication, Variant conversion, unicast signals |
| `extension.js` | `enable()` / `disable()` |

## Contract version 1

- **Enum strings and keys:** roles, window actions, capabilities and shortcut ids use the serde `snake_case` names. `a{sv}` keys use the serde field names. `Rect` is `(iiii)` = `(x, y, w, h)` in logical pixels.
- **Advertised capabilities (only these):** `place_windows`, `focus_tracking`, `window_actions`, `fullscreen_tracking`, `maximized_tracking`, `clipboard_read`, `clipboard_write`, `global_shortcuts`, `keyboard_layout`, `caps_lock`.
- **Not advertised:**
  - `input_region`: the client sets its own GTK input region.
  - `screenshot`, `capture_hiding`, `brightness`: not implemented. `Screenshot` and `SetCaptureHidden` answer `org.tidemark.Shell1.Error.Unsupported` with the capability name as the message, and `Brightness` is never emitted.
- **Window roles:** the client's GTK window **title** says what each window is.
  - `tidemark-island`: the island. It is made sticky and kept above other windows, hidden from Alt+Tab and the window list, and centred at the top of the primary monitor just below the top bar.
    - For `Place("island", rect, input)`, `rect.x` is ignored and `rect.y` is the offset below the top bar (`top_offset`).
    - `rect.w`/`h` are the requested size, but the window's real frame width is used for centring. The client sizes its own window.
    - The island is re-centred when monitors or the panel change. If mapping the island takes focus within 1 s, focus goes back to the app the user was in.
  - `tidemark-overlay`: the screenshot overlay. It gets the same treatment, and `Place` moves and resizes it to `rect`.
  - Any other window of the client is `settings`: `Place` moves it to `rect.x`/`y` and activates it.
- **`a{sv}` and `None`:** an **empty** dict means Rust `None` (for example no focused app). A non-empty dict has every non-`Option` field, and `None` options (`pid`, `icon`) are left out. The Rust decoder must follow this rule explicitly: it is not plain serde.
- **`Unsupported` errors:** the message is exactly the capability wire name, never prose, so the Rust client maps it with serde into `ShellError::Unsupported`.

  | Method | Capability |
  |---|---|
  | `Place` | `place_windows` |
  | `GetFocus` | `focus_tracking` |
  | `WindowAction` | `window_actions` |
  | `GetClipboard` | `clipboard_read` |
  | `SetClipboard` | `clipboard_write` |
  | `Screenshot` | `screenshot` |
  | `SetCaptureHidden` | `capture_hiding` |
  | `BindShortcuts` | `global_shortcuts` |
- **Focus:**
  - `GetFocus` and `FocusChanged` describe the last focused application window: never Tidemark's own windows, never Shell surfaces.
  - An empty `a{sv}` means none.
  - `pid` and `icon` are left out when unknown.
  - Moving a window does not re-send `FocusChanged`; a title or state change does.
- **Primary monitor:** the open `PrimaryMonitor` property and unicast `PrimaryMonitorChanged(monitor)` identify the monitor used for island placement. The snapshot sends this index before per-monitor fullscreen state. Monitor changes update the property and signal even when fullscreen flags do not change. Clients compare `FullscreenChanged` with this index, so a fullscreen app on another display does not hide the island. These are additive version-1 members; clients of older adapters must leave automatic fullscreen hiding unavailable if the property is absent.
- **`WindowAction(id, action, arg)`:**
  - `arg` is `<b>` for `set_above` and `<y>` for `set_opacity`, which takes 26–255 so a window can't be made invisible. Other actions ignore `arg`.
  - The method returns the state Mutter reports afterwards.
  - Actions on Tidemark's own windows or Shell windows fail.
  - `tile_left`/`tile_right` give half of the work area.
  - Opacity the adapter set is put back after Mutter animations. Each window gets back the opacity it had before Tidemark changed it when the client exits or crashes and on disable, and windows already destroyed are skipped.
- **`BindShortcuts(a(ss))`:**
  - It uses `MetaDisplay.grab_accelerator`. A combination that GNOME or another app already owns returns `false`; existing bindings are never overridden, and no gsettings are written.
  - Linux defaults: launcher `<Super><Alt>space`, theme `<Super><Alt>t`, like `<Super><Alt>l`. These avoid Ubuntu's Alt+Space (window menu) and Ctrl+Alt+T (terminal).
  - Each call replaces the previous grabs. Shortcuts work in normal and Overview modes, not on the lock screen.
- **Clipboard:**
  - `ClipboardChanged(mimes, sensitive)` carries MIME types only.
  - Content marked with `x-kde-passwordManagerHint` is announced as `([], true)` and `GetClipboard` refuses to read it.
  - Content is returned as `ay` (at most 64 MiB). File descriptors are not used in version 1.
- **Bounded reads:** reads go through `Meta.Selection.transfer_async` with `size` = 64 MiB + 1, which is a hard byte limit in Mutter 46's `core/meta-selection.c`, and are cancelled after 5 s. Larger content is refused, so unbounded provider data is never buffered in gnome-shell.
- **No stale reads:** every owner change bumps a clipboard generation, and a read during which the owner changed (even A → B → A) is discarded.
- **Keyboard:** `LayoutChanged("RU")` and `CapsLock(b)` are sent on change only.
- **Signals** go **only** to the client that called `Hello`, as unicast D-Bus signals. Right after `Hello` the adapter sends a snapshot: `FocusChanged`, the `FullscreenChanged` monitors that are on, `MaximizedTopChanged`, `LayoutChanged` and `CapsLock`.

## Who may call what

- **Open to any caller:** `Version`, `Capabilities`, `ClientState`, `RequestLaunch`, `RequestHandover`. These only ever start the fixed binary, with only `--settings` passed through.
- **Everything else** requires the caller's D-Bus peer PID (`GetConnectionUnixProcessID`) to be the process the adapter launched itself. The check runs again after the asynchronous lookup against the current *client generation*, so a caller checked for a client that has since exited, restarted or been re-adopted is refused. Otherwise the error is `AccessDenied`.
- **What gets launched:**
  - The client is always started as `/usr/bin/tidemark`, never looked up in `PATH`.
  - On Wayland it is started as a trusted client through `Meta.WaylandClient`; window ownership comes from `owns_window`, or the PID on X11.
  - It gets `TIDEMARK_SHELL_ADAPTER=1` in its environment.
  - For development, gnome-shell's own environment may set `TIDEMARK_ADAPTER_CLIENT=/abs/path/to/tidemark`. It must be an absolute path to an executable and is logged when used.

## Startup, single instance and handover

`ClientState` is one of `none`, `starting`, `running`, `blocked`, `failed`. It is an open property with `PropertiesChanged`.

1. **Normal:** on `enable()` the adapter launches `/usr/bin/tidemark`. The state is `starting`, then `running` once the client calls `Hello`.
2. **Tidemark started by hand (app menu, terminal, autostart) without `TIDEMARK_SHELL_ADAPTER`.** The app must do this **before** registering its GApplication:
   - If `org.tidemark.Shell` exists, call `RequestLaunch(args)`.
     - `true`: exit; the trusted instance starts.
     - `false` with `ClientState` `running`/`starting`: register normally; GApplication forwards the activation (for example `--settings`) to the trusted primary.
     - `false` otherwise (`failed`, or a cooldown): run unintegrated (`NullBackend`) and handle the state as in case 3.
   - If the bus name is missing: run unintegrated and watch for the name to appear.
3. **An untrusted Tidemark is already running when the adapter starts** (for example the extension was enabled later). The adapter's child finds the existing GApplication primary, forwards to it and exits with status 0 within 5 s without `Hello`. The adapter sets `blocked` and **does not restart**, so it doesn't loop into the same wall. The running app sees `blocked` (or gets `AccessDenied` on `Hello`) and then:
   - offers the user a restart for GNOME integration. It must **not** restart silently.
   - on consent: saves all state (notes, reminders, running timers and the sleep timer, clipboard history, shelf), calls `RequestHandover(args)` and exits.
   - The adapter launches the official client only after the caller's bus connection is gone (it watches the caller's unique name), so the new process becomes the primary and restores the saved state.
   - If the old instance doesn't exit within 30 s, the handover is cancelled.
4. **Crashes** (non-zero exit or signal) are restarted after 1, 2, 5, 10 and 30 s; a run of 60 s or more resets the count. Then the state is `failed` until `RequestLaunch`. A clean exit after `Hello` is a user quit (`none`) and is not restarted.
5. **Screen lock:** GNOME disables extensions on lock. The client keeps running, and on unlock the adapter re-adopts the same process (state `starting`). The client must call `Hello` again when `org.tidemark.Shell` reappears.

Requirement for packaging: the `.desktop` file must not use `DBusActivatable=true`. Activation through dbus-daemon would start an instance the adapter didn't launch.

## Tests (no GNOME needed)

```sh
npm test --prefix gnome-extension      # = node --test (Node 18+), Windows and Linux
node --check gnome-extension/tidemark@treyzar.github.io/*.js   # syntax of every module
```

- `tests/contract.test.js` checks the XML, `contract.js` and `shell.rs` against each other:
  - capability, role, action and shortcut wire names;
  - `FocusInfo`, `WindowState` and `Rect` field names and D-Bus types;
  - `ShellSignal` variants and field names against the signal arguments and signatures;
  - that every `ShellBackend` method has a D-Bus method;
  - error kinds, `metadata.json`, the fixed client path and the default shortcuts.
- `tests/adapter.test.js` drives `adapter.js` with a fake platform:
  - launch by fixed path and the developer override;
  - PID authentication and generations;
  - crash backoff, give-up and recovery;
  - single-instance `blocked`, handover and handover timeout;
  - re-adoption after lock;
  - island and overlay roles, placement below the top bar, re-centring, the focus-steal guard;
  - focus, maximized and fullscreen signals;
  - window actions and their limits;
  - shortcut conflicts;
  - clipboard secrecy;
  - keyboard signals;
  - `Unsupported` answers.

These tests cannot prove the GNOME calls themselves: `gnomePlatform.js` and `dbusService.js` only run inside gnome-shell.

## Headless real-GNOME harness (Linux CI)

`integration/run-headless.sh` runs the real extension inside a **private headless gnome-shell 46**. It uses Mutter's native backend: `--headless --wayland --no-x11 --virtual-monitor 1280x800 --wayland-display tidemark-test-0 --unsafe-mode`, flags checked in Mutter 46.0's `meta-context-main.c`.

**Isolation:**
- The shell runs on a private `dbus-run-session` bus, in its own process group, which is stopped as a group.
- All writes go to a fresh harness root (`mktemp -d`, mode 700): `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME` and `XDG_RUNTIME_DIR` point inside it. `HOME` is not changed (on CI it is the runner's own ephemeral home), and nothing is written there.
- gsettings (`enabled-extensions`, input sources `us`+`ru`) are written only to the root's dconf.
- The outer part refuses to run unless `CI=true` or `TIDEMARK_HARNESS_ISOLATED_OK=1`.
- The inner part (`--inner`) refuses unless it was started by the outer part, before any gsettings write. It checks:
  - a one-time token from the root, consumed on use;
  - that the root and XDG paths are validated, owned by the user and mode 700;
  - that it runs on a session bus different from the caller's, on which `org.gnome.Shell` is not present.
- The extension state is polled (up to 30 s) until it is ACTIVE with no error. GNOME 46 and 50 temporarily expose ERROR (`3`) with an empty error during asynchronous loading; that sentinel is allowed to settle, while a reported error or OUT_OF_DATE fails immediately. A persistent sentinel fails the ACTIVE deadline. After the fixture, ACTIVE and an empty `GetExtensionErrors` history are required again.

**How it drives the adapter:**
- The adapter launches `integration/fixture-client.js` through the developer override `TIDEMARK_ADAPTER_CLIENT` (absolute path, executable).
- The fixture calls `org.tidemark.Shell1` from that launched process, so it is authenticated by its real PID. Production authentication is unchanged.
- `integration/victim.js` is a separate GTK 4 app window the adapter does not own.
- Results are checked against Mutter itself through `org.gnome.Shell.Eval`, which only this private `--unsafe-mode` shell exposes. The extension state comes from `org.gnome.Shell.Extensions.GetExtensionInfo` (`state` 1 = ACTIVE).

```sh
sudo apt-get install -y --no-install-recommends gnome-shell gjs gir1.2-gtk-4.0 dbus dbus-daemon   dconf-service dconf-gsettings-backend dconf-cli libglib2.0-bin libgl1-mesa-dri mesa-vulkan-drivers
CI=true TIDEMARK_HARNESS_ARTIFACT_DIR="$PWD/gnome-harness-artifacts"   bash gnome-extension/integration/run-headless.sh    # 0 = required checks passed, 1 = a check failed, 2 = refused
# The artifact dir receives results-*.txt, phase1.json and gnome-shell.log (harness files only).
```

| Area | Check |
|---|---|
| Harness | extension ACTIVE; `org.tidemark.Shell` owned; `Version` open; foreign `gdbus` caller gets `AccessDenied`; no extension JS errors in the shell log |
| Phase 1 (launched PID) | open properties before `Hello` (`ClientState` `starting`); `Hello` → `running` with snapshot `FocusChanged({})` = None, `CapsLock`, `MaximizedTopChanged` |
| Island | Mutter frame is top-centred below the panel; above, sticky, skip-taskbar |
| Place | unknown role → `InvalidArgs` |
| Focus | the victim is activated through the oracle, as a user click would (a new window does not take focus from the island without input); `FocusChanged` for the victim with serde keys, `(iiii)` rect and the victim's real PID; `GetFocus` agrees |
| Window actions | `set_above`, `set_opacity` (and the floor), `tile_left` (Mutter frame = left half of the work area, and `MaximizedTopChanged(true)` because it covers the strip), `toggle_maximize` both ways in Mutter, minimize → `MaximizedTopChanged(false)` and activate → `(true)`; actions on the island refused |
| Clipboard | the victim (another app, focused) offers content: text + password hint → `([], true)` and unreadable; 64 MiB + 1 → refused by the bounded read; 1 MiB → read intact; the adapter's own `SetClipboard` round trip through `Meta.Selection`; Screenshot → `Unsupported`/`screenshot` |
| Shortcuts | defaults grabbed, `<Alt>space` reported taken |
| Crash | the victim is dimmed to 179, then the fixture exits with status 3 |
| Phase 2 (restarted PID) | the new PID is authenticated after `Hello` (`starting` → `running`); the victim's opacity is back to 255 |
| `[optional]` (reported, fatal only with `TIDEMARK_HARNESS_STRICT=1`) | Super+Alt+Space, Caps Lock and the input-source switch via a Clutter virtual keyboard |

**What the harness cannot show:**
- **Status:** the private headless harness has passed all 25 required and 3 optional checks on GNOME 46.2 and 50.1 in CI. The startup-state polling fix and final error-history check still require a native CI rerun; local Bash regression tests stub D-Bus and never start GNOME.
- **`SetClipboard` over 64 MiB** is not sent in the harness: the bus closes the connection of a sender that exceeds its message limit (seen in CI), so the adapter's own bound is covered by the mock tests.
- **Not covered at all:**
  - the Xorg session;
  - real input focus behaviour when clicking the island;
  - multi-monitor and scale changes;
  - lock-screen re-adoption;
  - the single-instance `blocked`/handover flow with a real GApplication.

  These stay in the manual acceptance list below.

## Real GNOME acceptance (to be run; not done yet)

On fresh Ubuntu 24.04 (GNOME 46, Wayland **and** Xorg) and 26.04 (GNOME 50, Wayland) VMs, with `/usr/bin/tidemark` installed:

1. Install the folder to `~/.local/share/gnome-shell/extensions/tidemark@treyzar.github.io` (or with the `.deb`). Log out and back in, then run `gnome-extensions enable tidemark@treyzar.github.io`. Watch `journalctl --user -f -o cat /usr/bin/gnome-shell | grep tidemark`: there must be no errors.
2. Check the open surface:
   - `gdbus introspect --session --dest org.tidemark.Shell --object-path /org/tidemark/Shell` shows the interface.
   - `gdbus call ... --method org.freedesktop.DBus.Properties.Get org.tidemark.Shell1 ClientState` returns `running` once the app has said `Hello`.
3. From a terminal (not the client), `gdbus call --session --dest org.tidemark.Shell --object-path /org/tidemark/Shell --method org.tidemark.Shell1.GetFocus` must fail with `org.tidemark.Shell1.Error.AccessDenied`.
4. Island:
   - It is centred below the top bar and stays above a normal maximized window.
   - It is absent from Alt+Tab, the dash's window list and the overview window picker.
   - It is visible on every workspace.
   - Starting the app does not take focus from the window you were typing in.
   - Clicking the island's text field gives it keyboard input.
5. Plug in or move a second monitor and change scale: the island re-centres on the primary monitor.
6. Focus and window actions:
   - Switching apps updates the Window tile: title, icon, app id.
   - Pin on top, opacity 85/70/100, minimize, maximize/restore, and left/right half all work on GTK, Qt and XWayland apps, and the returned state matches.
7. Fullscreen and maximized: fullscreen video sends `FullscreenChanged(primary, true)`, and maximizing the focused window sends `MaximizedTopChanged(true)`.
8. Shortcuts:
   - Super+Alt+Space, Super+Alt+T and Super+Alt+L reach the app.
   - Binding `<Alt>space` reports `false`, and the window menu still opens on Alt+Space.
   - Nothing fires on the lock screen.
9. Clipboard:
   - Copying in another app sends `ClipboardChanged` while the island is unfocused, and text and image reads match.
   - A KeePassXC password copy sends `([], true)` and cannot be read.
   - `SetClipboard` text pastes in another app.
10. Keyboard: switching input source sends `LayoutChanged`, and Caps Lock sends `CapsLock`.
11. Lifecycle:
    - `kill -9` the client: it restarts within about 1 s.
    - Quit it from the tray: no restart.
    - Lock and unlock: the same PID is re-adopted.
12. Single instance:
    - Start `tidemark` by hand before enabling the extension, then enable it: `ClientState` goes to `blocked` with no restart loop. The app offers the restart; after consent, state is kept and `ClientState` goes to `running`.
    - Start `tidemark --settings` by hand while integrated: it forwards to the trusted instance.
13. Disable the extension: shortcuts are released, opacities are reset, and the client keeps running without integration.

Record results per VM in `docs/ubuntu/testing.md` (owned by QA/parent).

## Known limits of version 1

- The island sits below the GNOME top bar, not over the clock.
- There is no screenshot, capture hiding or brightness yet; they answer `Unsupported`.
- `x-kde-passwordManagerHint` is treated as secret whatever its value.
- GNOME 47, 48 and 49 are declared in `metadata.json` but only 46 and 50 are in the acceptance matrix.
- Clicking into the island depends on Mutter giving focus to a keep-above normal window (acceptance step 4).
