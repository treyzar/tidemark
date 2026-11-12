# Tidemark for Ubuntu

Native Rust GTK4 application under development for Ubuntu 24.04/26.04 with GNOME. The stable release currently remains the Windows application; this package is a development checkpoint.

The current source connects shared config, history, notes, reminders, timers and text tools to native views, and uses a worker for MPRIS playback. The GNOME worker now retains its authenticated connection for placement and desktop state; window actions, background clipboard and capture controls remain unavailable. The full parity checklist is in [the architecture](../../docs/ubuntu/architecture.md); capture/editor/OCR/QR/recording, actual Yandex Music likes, shelf/dock, search, weather/lyrics and system controls still need implementation and acceptance testing.

## Build

Use Ubuntu 24.04 amd64 with Rust stable:

```sh
sudo apt-get install libgtk-4-dev libadwaita-1-dev
cargo build --release --locked --manifest-path apps/linux/Cargo.toml
cargo run --manifest-path apps/linux/Cargo.toml
```

The binary is `apps/linux/target/release/tidemark`. `--settings` opens settings. The GNOME adapter is in `gnome-extension/tidemark@treyzar.github.io`; installing the package does not silently enable it or change desktop settings.

Artwork uses the shared `tidemark-artwork` crate: bounded HTTP/file/data reads
and image decoding run on the media worker, producing 160×160 straight RGBA.
The GTK thread creates `R8g8b8a8` memory textures and keeps at most eight by
content hash. Player owner, track metadata and URI key the shared bounded retry
schedule, including late artwork and replacement bytes at the same URI.

## GNOME bridge slice

The worker subscribes to Shell1 and bus owner changes before `Hello`, retains
its connection, checks version/capability names and reconnects to each new
unique owner after extension restarts. Individual calls and initial connection
use five-second limits. State signals are accepted only from that owner and
only when addressed to the client's unique bus name. Focus dictionaries retain
exact D-Bus types; an empty dictionary means no focus, while malformed data is
reported as an error.

Placement follows compact/expanded size and the configured top offset. The
additive `PrimaryMonitor` property and `PrimaryMonitorChanged` signal select
which fullscreen events hide the island; older adapters without this mapping
leave fullscreen hiding unavailable. Restoring uses visibility rather than
window activation. `Super+Alt+Space` toggles the island, and `Super+Alt+T`
changes the theme; search and Yandex likes remain unavailable. Occupied shortcut
bindings are reported and never replaced.

An untrusted running instance offers an explicit restart confirmation. It saves
notes, reminders, config and clipboard history through fallible atomic writes
before requesting handover, then exits only after the adapter accepts. Running
or paused timers, an armed music sleep timer and unsaved text prevent handover
because transferring those states is not implemented. Ordinary smoke runs never
start this worker. The protocol is covered by portable tests; native GTK and
real GNOME lifecycle/placement acceptance are still required for this slice.

## Verification

```sh
cargo test --manifest-path apps/linux/Cargo.toml --locked
```

On Windows this checks only the portable UI model; the real GTK code is Linux-only. Native CI also compiles GTK, builds and installs the `.deb`, and runs `tidemark --smoke-test` with isolated XDG folders and no live clipboard or playback actions. Render artifacts are requested through `TIDEMARK_SMOKE_DIR`. Native checks are required before this checkpoint can be called a working Ubuntu build.
