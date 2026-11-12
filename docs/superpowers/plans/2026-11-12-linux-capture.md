# Linux capture utilities checkpoint — 2026-11-12

`crates/tidemark-capture` is a standalone private Rust workspace with its own Cargo.lock. It supplies a real screenshot portal client, offline OCR/QR and raster operations. It does not supply a GTK selection/editor window, clipboard integration, island exclusion policy, drawing annotations/undo or screen recording. This checkpoint is a port component, not a completed capture feature.

## API for the GTK worker

```rust
// Linux only. Explicit user action; never call from GTK's main thread.
let portal = tidemark_capture::portal::Client::connect()?;
let captured = portal.screenshot(&tidemark_capture::portal::ScreenshotOptions {
    parent_window: exported_parent_handle, // wayland:..., x11:..., or empty
    interactive: true,
    timeout: std::time::Duration::from_secs(60),
})?;
let image = captured.load()?;
let selection = image.crop(tidemark_capture::Rect { x: 0, y: 0, width: 120, height: 60 })?;
let text = tidemark_capture::ocr::recognize(&selection, std::time::Duration::from_secs(15))?;
let qr_strings = selection.qr()?;
```

`Raster` is portable: `decode(&[u8])`, `load(&Path)`, `from_rgba(width,height,Vec<u8>)`, `width()`, `height()`, `pixels()`, `crop(Rect)`, `pixelate(Rect,block_size)`, `png()`, and `qr()`. Rectangles are in physical decoded image pixels. `pixelate` mutates only the selection using mean-color blocks (2–128 pixels); it is a visual effect, with no information-erasure guarantee. `qr()` returns unique UTF-8 decoded strings, an empty vector when no grids are found, and a distinct error if all detected grids fail decoding. It never opens QR URLs or executes their contents.

`ScreenshotOptions` defaults to interactive=true and 60 seconds. The portal may display its own permission/selection UI. No silent-capture fallback is attempted. `Screenshot { path: PathBuf }` represents the portal-confirmed local URI; image opening is explicit through `load()`. The library does not remove portal-owned files. The caller owns user-visible screenshots and the editing/clipboard workflow. There is no request in constructors except session-bus connection establishment; no screenshots run unless screenshot() is explicitly called.

## Portal lifecycle and identity

The client uses the session bus service `org.freedesktop.portal.Desktop`, starting its registered D-Bus service on an explicit screenshot request if necessary. It captures the unique owner, derives the request path from its unique sender name and a per-call 128-bit getrandom token, and registers a sender/path/interface/member-specific Response subscription **before** Screenshot. The one-slot response queue retains an immediate signal emitted before the method reply. Method invocation targets the pinned unique owner. The returned request path must exactly equal the predicted path; older portal implementations using different paths fail explicitly rather than weakening matching.

Owner mapping is checked before the call, after its reply and after the matching signal; message sender/path are checked too. A replaced owner, unexpected request path, wrong signal issuer, missing/remote/ambiguous URI or malformed response cannot yield success. Response 0 requires a local file URI, response 1 is Error::Canceled, and other codes are Error::Portal. Parent handle and interactive option are forwarded without selecting a browser or touching the clipboard. The client does not require the newer version-3 target option, preserving Ubuntu 24.04 portal compatibility.

Connection establishment has a two-second deadline. Individual method waits are two seconds; the entire screenshot transaction has a caller timeout capped at 120 seconds, using an async timeout race over all waits. On an unsuccessful operation after request initiation, Close targets the exact expected request on the pinned owner, with an additional maximum 500 ms cleanup budget. No automatic retry captures another screenshot. Caller must bound simultaneous UI workers; this library has no background scheduling.

## Image and OCR bounds

Only PNG, JPEG and WebP are enabled. Input file/byte length is capped at 32 MiB, width and height at 8192, total pixels at 16,777,216 (64 MiB RGBA), and decoder allocations at 128 MiB. Dimensions are checked before full decoding. Caller RGBA length must exactly match dimensions. PNG output uses a capped writer. Raster loading opens once, validates the opened file, reads at most the byte limit plus one sentinel, and on Linux rejects a final symlink and non-regular files using no-follow/nonblocking opens. Crop arithmetic is checked. QR detection is limited to 64 grids, and operates on local decoded pixels only. Image decode/QR buffers can coexist; the decoder allocation limit is not a total process-memory claim.

OCR uses the installed `tesseract` executable directly via Command, with fixed rus+eng, LSTM engine, page segmentation 3 and OMP_THREAD_LIMIT=2. There is no shell and no network. A private temporary PNG is automatically removed. Stdin/stderr are null; stdout is drained nonblocking into at most 1 MiB of memory. A monotonic process deadline (caller value capped at 60 seconds) terminates and reaps the direct child. The process deadline begins at process launch; PNG preparation is bounded by image limits and precedes it. Errors include unavailable executable, failure/missing language data, output limit and timeout; stderr/private image paths are never incorporated into error messages. Install tesseract-ocr, tesseract-ocr-rus and tesseract-ocr-eng on Ubuntu. The child runner is Linux-only; raster/URI/QR operations test on Windows.

## Validation

Observed RED: the three initial portable image/URI/QR tests failed against unimplemented APIs. After implementation, four Windows tests pass, covering strict local URI handling, synthetic QR generation/PNG round trip, malformed/oversized dimensions, exact RGBA length, crop overflow/empty bounds and selection-only pixelation. `cargo test --locked`, formatting and Windows clippy with warnings denied pass.

`cargo check --locked --target x86_64-unknown-linux-gnu --all-targets` and Linux-target clippy with warnings denied pass on the Windows host. This compiles, but does not execute, native portal/OCR tests. Native fixtures use a private D-Bus service and generated data; they never invoke the real desktop portal. They cover success with Response emitted before the Screenshot method return, user cancellation, portal failure, remote URI rejection, timeout/Close, wrong returned path, owner replacement, spoofed signal owner and request-path syntax. OCR subprocess fixtures use the test executable, synthetic output and a timed sleep to test success, overflow and killing/reaping on timeout. A separate ignored test invokes installed Tesseract against a generated blank image only.

Required native gate:

```text
dbus-run-session -- cargo test --locked --manifest-path crates/tidemark-capture/Cargo.toml
# After installing Tesseract and rus+eng, offline synthetic OCR smoke:
cargo test --locked --manifest-path crates/tidemark-capture/Cargo.toml installed_tesseract -- --ignored
```

Native RED/GREEN execution remains pending Ubuntu CI. Actual desktop portal consent, parent-window integration, screenshot scaling/coordinates and the GTK editor/clipboard flow still need native application smoke checks. No development call captured a screen, inspected a live browser, read a user clipboard or uploaded text/images.

## Primary references checked

- [Screenshot portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Screenshot.html): method options and URI result. Interactive customization may present portal UI; target selection is a newer optional interface feature.
- [Request portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Request.html): handle-token path convention, subscribe-before-call race avoidance, response codes and Close semantics.
- [Tesseract command-line usage](https://tesseract-ocr.github.io/tessdoc/Command-Line-Usage.html): combined language selection, LSTM engine and text/stdout invocation.
