# Native system controls component

Scope: only `apps/linux/src/ui/system.rs`, `apps/linux/src/services/system_state.rs` and this plan. Parent owns mounting, manifests, packaging and native CI.

## Integration

- Add Linux dependency `tidemark-system = { path = "../../crates/tidemark-system" }`.
- Declare `pub mod system;` in `ui/mod.rs`.
- Build `SystemView::build(shared.smoke)`, append its public `root`, and **retain the SystemView handle** as a ControlView field. Dropping the handle stops its weak timer/controller and invalidates queued UI results.
- The component polls its own weak GLib timer; the optional public `tick()` is not required from ControlView.
- For portable CI, include `#[cfg(test)] #[path = "services/system_state.rs"] mod system_state_tests;` in main.rs.

The constructor in normal mode starts a read-only refresh on a worker. Smoke mode starts no thread and performs no system, device, clipboard or process access. All real mutations require explicit user buttons.

## Behavior

Sound and brightness have a draft slider followed by explicit Apply. The labels show only the last confirmed backend value; choosing a draft does not claim the host changed. Drafts capture the backend target object when edited. Apply, mute, Wi-Fi and per-adapter Bluetooth operations retain those exact identities and rely on backend verification before writing. Unsupported devices have no invented zero measurement and no enabled action. A rejected confirmation clears the affected state rather than retaining a potentially stale on/off value.

The first available backlight is displayed; multi-backlight selection is a follow-up. Bluetooth adapters are shown individually. Battery, CPU and RAM are read-only. CPU waits for a second valid sample instead of displaying fabricated zero utilization. Provider errors are mapped to short Russian states.

At most one worker and one bounded result slot exist per component. No GTK object enters a worker. Pending controls are disabled, periodic refresh waits for the current job, and request tokens reject stale or closed results. Dropping the view flags cancellation; a backend call already in progress remains bounded by its existing deadline and may finish off-thread. The component cannot undo an explicit mutation already sent to the host.

## Verification

Actually run on Windows:

```text
rustc --edition 2021 --test apps/linux/src/services/system_state.rs -o target/system-state-tests.exe
target/system-state-tests.exe
2 passed
rustfmt --edition 2021 --check apps/linux/src/ui/system.rs apps/linux/src/services/system_state.rs
passed
```

Tests cover single pending request, stale completion not unlocking a newer operation, and close invalidation. No test changes host volume, radio or brightness. GTK type checking and native smoke rendering require parent Ubuntu CI; this component has not been claimed built or run under GTK here.
