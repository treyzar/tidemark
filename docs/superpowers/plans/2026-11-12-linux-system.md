# Ubuntu system controls backend

`crates/tidemark-system` is a standalone Rust workspace. No GTK or application files are part of this slice. Construction never changes host settings; mutations require an explicit target captured from a preceding read.

## Public API

- `volume::read() -> Result<Volume>` reads the current default PipeWire sink through fixed `/usr/bin/wpctl`. `Volume::identity()` exposes numeric node ID and object serial. `volume::{set_level, set_muted}` take the captured `Volume`, re-read identity before mutation, issue only literal argument arrays with the numeric target, and return confirmed observed state. A changed default or reused node ID fails rather than redirecting a queued operation. Levels set by Tidemark are limited to 0..=1; reads can report existing amplification. Each child process has a two-second deadline and 64 KiB output cap. Identity checks around separate CLI calls cannot make node reuse atomically impossible; confirmation fails if the later identity differs.
- `Client::connect()` opens the system bus with a bounded initial connection and one-second method deadlines. `wifi()` returns software/hardware enabled flags and Wi-Fi device count. `set_wifi(&WifiState, want)` targets the captured unique owner and exact device inventory, and confirms readback. Hardware radio disable is reported separately from the software toggle.
- `Client::bluetooth()` returns adapter states. `set_bluetooth_powered(&adapter, want)` verifies captured service owner, object path and hardware address, then confirms the result. It does not scan, pair or remove devices.
- `Client::battery()` reads UPower's display device. Missing battery/service, malformed data and transport failure are distinct errors; no fake zero is synthesized.
- `brightness::inventory()` reads bounded backlight attributes under `/sys/class/backlight`, whose canonical devices must remain under `/sys/devices`. The captured inode/device/max-brightness identity is rechecked by `Client::set_brightness`. Only logind's session `SetBrightness` mutates brightness; the library never writes privileged sysfs files directly.
- `metrics::cpu()` and `metrics::memory()` read bounded `/proc` text. Portable parsers calculate CPU utilization from two samples, exclude double-counted guest time, and require real available-memory data.

All blocking operations belong on a UI worker. Missing services/devices return `Unsupported`, policy rejection returns `Denied`, identity changes return `Changed`, and a reply without confirmed state returns `NotConfirmed`. System D-Bus operations share a twelve-second budget in addition to per-call bounds. No error text contains raw provider output or private device labels.

## Validation

Recovery found the production implementation present and compiling, but `src/fixtures.rs` empty. This slice fills it with canonical NetworkManager, BlueZ ObjectManager/Adapter, UPower and logind interfaces served on a private **session** bus. A single fixture test owns the canonical names to prevent parallel test interference. It covers reads, successful explicit writes, permission rejection, acknowledged-but-unapplied writes, changed Bluetooth identity, invalid battery percentage, invalid brightness input, and NetworkManager owner replacement. Brightness fixtures use only temporary fake sysfs directories. Process tests execute the Rust test binary itself with synthetic success/overflow/timeout modes, never host `wpctl`.

Actually run on Windows:

```text
cargo test --manifest-path crates/tidemark-system/Cargo.toml --locked
6 passed; Linux-only fixtures excluded
cargo check --manifest-path crates/tidemark-system/Cargo.toml --locked --target x86_64-unknown-linux-gnu --all-targets
passed
cargo clippy --manifest-path crates/tidemark-system/Cargo.toml --locked --target x86_64-unknown-linux-gnu --all-targets -- -D warnings
passed
cargo fmt --manifest-path crates/tidemark-system/Cargo.toml --check
passed
```

The native eleven-test suite has **not run on this Windows host**. Parent CI must execute:

```sh
dbus-run-session -- cargo test --manifest-path crates/tidemark-system/Cargo.toml --locked
```

No live volume, Wi-Fi, Bluetooth, brightness or battery operations were invoked during development. Physical device/session acceptance and GTK integration remain separate gates. Installing `wireplumber` supplies the fixed `wpctl` executable; it is a runtime packaging requirement when these controls are exposed.

## Primary contracts

- [NetworkManager manager methods and radio properties](https://networkmanager.dev/docs/api/latest/gdbus-org.freedesktop.NetworkManager.html)
- [BlueZ adapter API](https://bluez.readthedocs.io/en/latest/adapter-api/)
- [UPower display device](https://upower.freedesktop.org/docs/UPower.html)
- [logind session and brightness API](https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.login1.html)
- [WirePlumber wpctl CLI](https://pipewire.pages.freedesktop.org/wireplumber/tools/wpctl.html)
