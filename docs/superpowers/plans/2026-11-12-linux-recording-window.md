# Linux recording window

The standalone component is `apps/linux/src/ui/record.rs`; its std-only lifecycle model is `services/record_state.rs`. The lead owns module/manifest/Tools-button wiring and native GTK CI.

## Public facade

`record::build(app: &adw::Application, smoke: bool) -> adw::ApplicationWindow` creates the ordinary window titled `Tidemark — запись экрана`. Construction and presentation do not invoke a portal, record, write a file or change the clipboard. With `smoke=true`, all effect controls are disabled and their callbacks also reject invocation.

The user chooses a new absolute `.webm` filename, selects screen/window and frame rate/cursor preferences, then explicitly starts. The window states that this backend records video without audio. It has no pretend audio toggle. Existing files are rejected rather than replaced; the backend's 0600 `create_new` check remains authoritative against races.

`Recorder::start` and all encoder/portal work happen on a single worker. Only plain Rust options, paths, atomics and a bounded single progress/result snapshot cross that boundary. GTK objects stay on the main thread. The worker retains the `Recorder` and returns a saved-file result only from backend `Ok(path)`. UI completion checks the request identity before rendering it. A caught worker panic becomes `Error::Worker`, never success or an indefinitely pending UI.

Stop during recording requests graceful EOS/save and disables repeated commands. Stop during startup cancels a shared backend `StartCancellation` token immediately, before waiting for the UI worker. `Recorder::start_with_cancel` watches it during portal configuration and closes its owned pending request/session; it also checks cancellation after consent, before creating output and before launching the encoder. A late ready event cannot revive cancellation. Request/session closing remains bounded by D-Bus deadlines, so the UI does not promise synchronous disappearance of the system dialog.

Closing an active recording requests graceful stop/save on the worker; closing a pending start requests cancellation. All later UI results are invalidated. No success is displayed after the window closes. The worker can finish its bounded cleanup without holding strong GTK references. Dropping a window/controller provides the same stop/cancel fallback.

`build` also installs a single `GApplication::shutdown` handler when `smoke=false`, so `app.quit()` is covered even when GTK does not emit the recording window's close request. The handler calls `Recorder::shutdown_all(Duration::from_secs(2))`: it cancels a pending start immediately, requests EOS for an active child and escalates to abort after the grace period (at most one additional second). It does not wait for an open portal dialog. The backend shutdown gate prevents a pending portal worker from launching an encoder afterwards. The direct child additionally has a Linux parent-death signal tied to its persistent monitor thread; that is a last-resort termination guarantee, not a saved-file guarantee.

The lead may call `record::install_shutdown_hook(app)` once at startup for clarity; calling it again from `build` is harmless. No root quit gate is required for child termination. This hook is irreversible and belongs only to actual application shutdown, not a user-facing quit confirmation or ordinary recording-window close. Smoke construction deliberately installs no shutdown hook.

## Validation

Observed a RED compile for the missing state implementation, then ran:

```powershell
rustc --edition 2021 -D warnings --test apps/linux/src/services/record_state.rs -o target/record-state-tests.exe
target/record-state-tests.exe
```

All three lifecycle tests pass: single in-flight/stale completion protection, late-ready cancellation, and close-time stop/invalidation. Formatting checks pass for both new Rust files. The GTK component has **not been compiled or rendered on this Windows host**. Its native APIs, real portal selection, output finalization, close/cancel flow and inert smoke rendering require the parent's Linux pipeline.

Integration needs the Linux `tidemark-record` path dependency, `pub mod record` under `ui`, an explicit Tools button, and optionally a test-only `#[path="services/record_state.rs"]` module in main. Runtime GStreamer packages and backend native/synthetic-encoder tests are described in `2026-11-12-linux-recording.md`.
