# Yandex favorite worker integration

Only new Linux service files are introduced: `services/yandex.rs` and portable
`services/yandex_state.rs`. Parent owns Cargo dependencies, module registration,
and Home UI wiring. No background browser polling or startup accessibility calls.

## UI API

Create `Service::new()` for normal use or `Service::inactive()` for smoke mode.
Both constructors are inert: no thread, MPRIS connection or browser query starts.
Use an `Rc<RefCell<Service>>` on the GTK thread if multiple handlers share it.

On **every** music event, call `observe(snapshot.as_ref())`, including `None`,
before draining completions. This records player/track/metadata changes and ABA
transitions (A → B → A or A → None → A). Progress updates with identical identity
leave the request valid. A changed identity raises the worker cancellation flag.

On an explicit favorite action, call `start(&snapshot, desired_bool)`. Desired
state is never inferred from a local favorite list or silently inverted. `busy()`
is truthful until the sole request finishes and is consumed; another start returns
`StartError::Busy`, without queuing or starting another worker. `available()` is
false for smoke/inactive services or after worker creation/disconnection failure.
Start errors implement fixed Russian Display messages.

Poll `try_recv()` on the GTK main loop. `Outcome` contains the complete captured
`Request { id, epoch, key, want }`, `result: Result<bool, Failure>`, and `current`.
Only when `current` is true may the UI show the result/error for the visible track
or update its favorite icon. Discard stale outcomes while still releasing the
busy button. Do not mark a favorite optimistically; a service/provider failure
never proves liked or unliked state.

## Execution and safety

The captured key contains the exact unique MPRIS owner and well-known player name,
nonempty track ID, title and ordered artists. Missing/NoTrack IDs, absent metadata
and oversized fields are rejected. The worker connects to MPRIS and reads
`snapshot(captured_player)`; every identity/metadata field must equal the captured
key. It never selects a newly current player. After connecting to the accessibility
service, it repeats this pinned MPRIS check immediately before the favorite operation.
The artist argument is the original artist list joined with `, `.

`tidemark_yandex::Client::set_liked_with_guard(title, artist, want, guard)` performs
the existing trusted-origin/exact-track/unique-button checks and consults the
worker's cancellation flag immediately before its sole DoAction, after the final
evidence and owner checks. A false guard returns Changed without mutation; the
operation budget is checked after the guard. Existing `set_liked` delegates with
an always-true guard, preserving its API. A native fake-bus regression asserts a
false guard is evaluated after evidence reads, with zero actions and unchanged
favorite state. It was added before implementation and Linux-target compilation
initially failed for the missing method.

There is one request in flight and one bounded completion slot. No GTK value is
captured by the worker. Thread creation failure clears pending state and disables
the service; disconnected/panicked worker completion becomes WorkerStopped and
also disables it. Dropping the service raises cancellation and drops its receiver
without blocking the UI; the existing bounded provider operations finish outside
GTK. Calls already committed to the remote provider cannot be rolled back.
All human error messages omit track text, browser URLs and remote exception data.

## Validation

Seven portable identity tests pass on Windows with warnings denied:

```
rustc --edition 2021 -D warnings --test apps/linux/src/services/yandex_state.rs -o <temporary-test-executable>
<temporary-test-executable>
```

They cover explicit desired state, one in-flight request, old completion versus a
new request, owner and metadata changes, missing IDs, repeated identical updates,
and both ABA sequences. The initial stale-epoch implementation produced the
expected ABA failure, then was fixed and passed. Direct rustfmt checks pass.

A temporary check-only Cargo manifest, outside the repository, combines the real
MPRIS/Yandex crates with the new service as its library source. Linux-target
checking and clippy with warnings denied pass. This catches service typing without
changing the app manifest. Native backend fixtures remain parent Ubuntu CI's gate;
no live desktop/browser calls or favorite changes were made during development.
Real current-browser accessibility semantics remain unverified as recorded in the
backend plan. Parent must add the `tidemark-yandex` Linux dependency, register the
service module, include yandex_state in portable tests, and wire Home.
