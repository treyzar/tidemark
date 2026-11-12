# tidemark-mpris — plan

Real MPRIS adapter for the Tidemark Ubuntu port. Not a stub: every field/method maps to the
verified MPRIS2 spec (https://specifications.freedesktop.org/mpris-spec/latest/).

## Stack decision

- Transport: **zbus 5** (pure Rust D-Bus, maintained, z-galaxy/zbus; blocking API in default
  features). Rejected: `mpris` 2.1.0 crate — it wraps libdbus FFI (`dbus ^0.9.6`, per docs.rs),
  which cannot build/test on the Windows host and is not needed given zbus covers everything.
- Crate is a **standalone `[workspace]`** until the parent integrates it into the root workspace.
- Real transport gated to `cfg(target_os = "linux")`; pure model/error/sleep modules compile
  everywhere so `cargo test` on Windows is meaningful without libdbus.

## API and safety contracts

- `MprisPlayerId { bus_name, owner }` — pinned to the **unique D-Bus owner** (`:1.x`) at discovery
  time; every command re-verifies `GetNameOwner(bus_name) == owner` before acting
  (`Error::OwnerChanged` / `Error::PlayerGone`).
- **Unique-owner destination**: after verification the command is sent with the unique owner name
  (`:1.x`) as destination, never the well-known name — no TOCTOU window in which a migrated player
  could receive a message intended for the pinned one.
- **Reply-awaiting calls**: `PlayPause`/`Pause`/`Next`/`Previous`/`SetPosition` wait for the method
  reply; remote rejections surface as `Error::Dbus`. All calls are bounded by the connection's
  `method_timeout` (5 s), set at `Client::connect` via `blocking::connection::Builder`.
- `TrackSnapshot` — bus, owner, track id, title, artists, album, art_url (returned unchanged),
  playing, position (with `position_now()` extrapolation like the Windows SMTC card), duration,
  can_prev/next/seek, volume.
- `Volume` lives on **org.mpris.MediaPlayer2.Player** (per spec; not the root interface):
  `snapshot().volume` and `player_volume(id, v)` both use the Player interface. Finite 0.0..=1.0,
  else `Error::InvalidVolume`.
- `seek(id, track_id, position)`: re-reads fresh `mpris:trackid`, requires it to equal the caller's
  captured track id (`Error::TrackMismatch` otherwise) and `CanSeek` (`Error::CapabilityMissing`),
  then `SetPosition(trackid, µs)`.
- Explicit errors: BusConnect, Dbus, PlayerGone, OwnerChanged, TrackMismatch, CapabilityMissing,
  InvalidVolume. No shell-outs, no fake data anywhere.

## Sleep scheduling (model only)

`sleep.rs`: `SleepTimer::arm(player_id, after)` captures the exact player identity only — no track
capture. `take_due(now) -> Option<MprisPlayerId>` fires **once** and returns the captured identity
regardless of which track is playing or which player the UI currently shows; only explicit
`cancel()` stops it early. Live owner validation at execution time is `Client::pause_exact`'s job.
Worker orchestration is the parent's task.

## Tests

- Portable (`src/model.rs`, `src/sleep.rs` unit tests): identity pinning semantics, snapshot
  extrapolation, sleep arm/take-due-fire-once/explicit-cancel/no-track-capture/no-current-player
  coupling.
- Linux integration (`tests/integration.rs`, runs under parent CI `dbus-run-session` on the
  private bus): fake MPRIS player served with zbus object server implementing the canonical Player
  interface (including `Volume`); asserts discovery + owner pinning, snapshot roundtrip, seek
  track-guard, play_pause/pause effects, volume get/set, restarted-player `OwnerChanged`, and
  `PlayerGone` for unknown players. Commands are exercised against the unique-owner destination
  path used by the client. Real desktop players are never touched.
- Execution status: Windows host `cargo test` passes (9 model/sleep tests); the Linux integration
  binary compiles clean via `cargo check --target x86_64-unknown-linux-gnu --all-targets` on the
  Windows host; **actual integration runs are pending** the parent's Ubuntu CI:
  `dbus-run-session -- cargo test --manifest-path crates/tidemark-mpris/Cargo.toml --locked`.
- `Cargo.lock` is generated and kept in the deliverable for `--locked` CI.

## Root Raise follow-up

`Client::raise(&MprisPlayerId)` verifies the captured owner, reads `CanRaise`
from the canonical `org.mpris.MediaPlayer2` root interface, and awaits `Raise`
on that same unique-owner destination. False capability returns
`CapabilityMissing("Raise")`; property/method remote errors propagate as
`Dbus`. The existing connection-wide five-second timeout bounds each call.
`Volume` remains on the Player interface. No browser URL fallback or shell-out.
An acknowledged Raise request does not prove the desktop granted foreground focus.

The official root contract is documented at
https://specifications.freedesktop.org/mpris/latest/Media_Player.html .
A FakeRoot is served alongside FakePlayer on the same object path. Three isolated
Linux tests cover observable fake Raise effects, remote rejection propagation,
unsupported capability, and stale-owner rejection after restart. Tests were added
first and Linux-target checking initially failed because `Client::raise` was absent;
native behavior RED/GREEN remains pending parent Ubuntu `dbus-run-session` CI.
