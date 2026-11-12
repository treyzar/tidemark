# Tidemark for Ubuntu

Native Rust GTK4 application under development for Ubuntu 24.04/26.04 with GNOME. The stable release currently remains the Windows application; this package is a development checkpoint.

The current source connects shared config, history, notes, reminders, timers and text tools to native views, and uses a worker for MPRIS playback. GNOME-only controls remain unavailable until their client adapters are wired. The full parity checklist is in [the architecture](../../docs/ubuntu/architecture.md); capture/editor/OCR/QR/recording, actual Yandex Music likes, shelf/dock, search, weather/lyrics and system controls still need implementation and acceptance testing.

## Build

Use Ubuntu 24.04 amd64 with Rust stable:

```sh
sudo apt-get install libgtk-4-dev libadwaita-1-dev
cargo build --release --locked --manifest-path apps/linux/Cargo.toml
cargo run --manifest-path apps/linux/Cargo.toml
```

The binary is `apps/linux/target/release/tidemark`. `--settings` opens settings. The GNOME adapter is in `gnome-extension/tidemark@treyzar.github.io`; installing the package does not silently enable it or change desktop settings.

## Verification

```sh
cargo test --manifest-path apps/linux/Cargo.toml --locked
```

On Windows this checks only the portable UI model; the real GTK code is Linux-only. Native CI also compiles GTK, builds and installs the `.deb`, and runs `tidemark --smoke-test` with isolated XDG folders and no live clipboard or playback actions. Render artifacts are requested through `TIDEMARK_SMOKE_DIR`. Native checks are required before this checkpoint can be called a working Ubuntu build.
