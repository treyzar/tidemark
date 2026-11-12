# Ubuntu video recording backend

`crates/tidemark-record` is a standalone Rust workspace. It implements video-only recording through the ScreenCast portal and an owned GStreamer encoder process. It does not capture during construction or tests, enable persistent portal permissions, or edit desktop preferences. GTK controls, packaging dependencies and full GNOME recording acceptance are owned by the lead.

## Integration API

- `Recorder::start(Options, absolute_webm_path) -> Result<Recorder>` is blocking and belongs on a worker. It validates options, acquires the single-recording permit, requests portal consent, creates a private new output, and waits until the encoder has begun writing a WebM cluster. Existing output files are never overwritten.
- `Recorder::start_with_cancel(options, path, StartCancellation)` preserves that API with a shared cancellation guard available before blocking. UI startup Stop/close cancels this token immediately. Portal configuration polls cancellation every 20 ms and closes its owned request/session, and post-consent guards prevent a late successful response from creating output or launching an encoder. The original `start` uses an uncancelled token for existing callers.
- `status()` and `poll()` are nonblocking. `poll()` returns `None` until completion; only `Some(Ok(path))` proves a saved file.
- `request_stop()` requests graceful EOS without blocking the GTK thread. Continue polling for the final result.
- `stop(self) -> Result<PathBuf>` waits on a worker for graceful stop and cleanup. A timeout/error is not a saved-file result.
- Dropping a recorder requests asynchronous abort. The owned monitor interrupts/reaps the encoder, closes the portal session and removes its uncommitted output. A replacement file at the output path is preserved. Do not immediately advertise a completed file after dropping the handle.
- `Recorder::shutdown_all(grace) -> bool` is an irreversible process-shutdown gate: future starts are rejected, pending consent is cancelled without waiting for the dialog, and an active encoder receives graceful EOS. It waits at most five seconds of grace plus one second for forced abort. The boolean reports child cleanup only; it never proves a saved file. The GTK shutdown hook uses two seconds of grace.

`Options::audio = true` returns `AudioUnsupported` before any portal request. No microphone or implicit default audio source is selected. Sources are monitor, window or either; one source is requested. Cursor embedding is checked against the portal's advertised modes.

## Protocol and process guarantees

The portal flow is `CreateSession`, `SelectSources`, `Start`, `OpenPipeWireRemote`. Request response subscriptions are installed before each method call. Unique owner, canonical request/session paths, the session-close signal, node/serial values and the received stream socket are checked. The historical `session_handle` response is a string. `pipewire-serial` is preferred when present, with the numeric node path used for older portals. Calls and the complete consent/configuration sequence have deadlines; cleanup closes only the request/session belonging to this connection and the captured owner.

The encoder is fixed at `/usr/bin/gst-launch-1.0`. It receives only fixed VP8/WebM pipeline elements and validated numeric properties. The user-selected filename is never inserted into GStreamer's parser: a new 0600 file is passed as stdout to `fdsink fd=1`, with quiet output enabled. The approved remote is duplicated to an occupied high descriptor and made inheritable only in the child. The parent's close-on-exec state is preserved.

The encoder owns its process group. Stop sends SIGINT so `-e` can finalize EOS; failure to stop by the deadline kills/reaps that owned group. Stderr is drained without retaining private diagnostics and capped at 64 KiB. The output size has a process-level file-size cap and monitor checks; maximum recording duration is bounded. A successful encoder exit, WebM header/cluster checks, output inode identity and `sync_all` are required before returning a saved path.

The persistent monitor performs portal setup, creates the encoder and owns it through reaping. In the child, `PR_SET_PDEATHSIG(SIGKILL)` plus a parent-PID recheck covers abrupt creator/process death. Linux ties that signal to the creator thread, so spawning on a short-lived caller and then transferring the child would be incorrect; this backend deliberately starts the child in its monitor. Forced shutdown can interrupt the EOS wait. Abrupt termination can leave an incomplete file and is never reported as a successful save.

Required Ubuntu runtime packages are `gstreamer1.0-tools`, `gstreamer1.0-pipewire`, `gstreamer1.0-plugins-base` and `gstreamer1.0-plugins-good`. Their presence and the actual PipeWire source properties must be checked by the native CI/package owner.

The GStreamer launcher documentation recommends the library API for applications and describes the launcher as a debugging tool. This slice deliberately recovers the existing process-based design requested for this backend; native CLI/plugin compatibility is therefore an acceptance gate, and migration to the GStreamer Rust bindings remains a possible follow-up. The child-process checks do not replace that compatibility test.

## Verification actually performed on Windows

- `cargo test --manifest-path crates/tidemark-record/Cargo.toml --locked`: five portable option/pipeline/shutdown-policy/cancellation-token tests pass.
- `cargo fmt --manifest-path crates/tidemark-record/Cargo.toml --check`: passes after recovering two formatting-only differences.
- `cargo check --manifest-path crates/tidemark-record/Cargo.toml --target x86_64-unknown-linux-gnu --all-targets --locked`: passes.
- Linux-target and Windows-target clippy with `--all-targets -- -D warnings`: pass.

The native portal and process fixtures compile but have **not executed on this Windows host**. Run them on an isolated session bus:

```sh
dbus-run-session -- cargo test --manifest-path crates/tidemark-record/Cargo.toml --locked
```

Fixtures cover response-before-return, canonical lifecycle and Unix FD transfer, cancellation/failure, timeout cleanup, wrong sender/path/owner, revoked sessions, invalid FDs, multiple streams, parent/child FD flags, graceful SIGINT output, forced kill, stderr overflow, encoder rejection and output/replacement preservation. They do not contact a desktop or a real PipeWire server.

Additional native fixtures cover consent-time shutdown returning promptly, EOS requests without downgrading an existing abort, forced interruption of an encoder ignoring EOS, parent-thread death killing an intentionally unreaped child, and an outer caller exiting while the persistent monitor legitimately continues. These compile but still require the Linux run above.

Cancellation regressions use the real private fake portal and production post-consent preparation path: cancelling while Start consent is pending closes its request/session without creating output or requesting PipeWire; a successful response racing with cancellation is rejected before output creation or encoder launch. They compile here and require the same native test run.

After installing the GStreamer packages, also run the explicit synthetic encoder test:

```sh
cargo test --manifest-path crates/tidemark-record/Cargo.toml --locked installed_gstreamer_encodes_only_synthetic_video -- --ignored
```

That test records only `videotestsrc`. It is ignored by default because it requires installed native plugins. A successful synthetic pipeline does not prove the portal's real GNOME/PipeWire source, permission dialog, cursor behavior or a complete screen recording. Those remain acceptance gates; video-only support must be labeled accordingly.

## Primary references

- [ScreenCast portal lifecycle and wire types](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)
- [Portal request handles and early response subscription](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Request.html)
- [Portal session lifecycle](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Session.html)
- [GStreamer command-line launcher and EOS option](https://gstreamer.freedesktop.org/documentation/tools/gst-launch.html)
- [Linux parent-death signal and creator-thread lifetime](https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html)
- [WebM muxer](https://gstreamer.freedesktop.org/documentation/matroska/webmmux.html)
