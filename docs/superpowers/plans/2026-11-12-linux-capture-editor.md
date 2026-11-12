# Modular GTK capture editor checkpoint — 2026-11-12

Files owned by this checkpoint: `apps/linux/src/ui/capture.rs` and `apps/linux/src/services/capture_state.rs`. Existing app/module/manifest files are unchanged. The editor depends on the ready `tidemark-capture` crate and GTK4/libadwaita already selected for Linux.

## Host integration API

Declare `pub mod capture` in the Linux UI module and ensure the Linux target dependencies include `tidemark-capture`. A Tools action can then call:

```rust
let window = crate::ui::capture::CaptureWindow::build(app, smoke);
window.present();
```

`build(&adw::Application, smoke: bool) -> adw::ApplicationWindow` constructs an ordinary, non-fullscreen editor titled `Tidemark — снимок экрана`. It does not request a screenshot, connect to portals, run OCR/QR, write files or access the clipboard. GTK callbacks retain editor state through weak widget references; callers can present the returned window without keeping a separate Rust controller alive. Host integration should reuse one editor window rather than create unrestricted concurrent editors.

`CaptureWindow::preview(app, synthetic_raster)` constructs a deterministic image preview with every effect disabled. The root smoke harness can generate a small RGBA raster and inspect/render that window without a live capture. The helper has no GTK/dependency requirements; root can add `#[cfg(test)] #[path = "services/capture_state.rs"] mod capture_state_tests;` in main for normal portable CI.

## User flow

- **Новый снимок** is the only capture trigger. It temporarily hides the editor, invokes the interactive screenshot portal on one background worker, loads its bounded image and then restores the editor. It currently supplies an empty portal parent handle; exporting a Wayland parent can be added by the host later. Portal cancellation is neutral and retains the previous image. Errors cannot fabricate a new image. This does not exclude the island or other application windows from the screenshot.
- The Picture fits the full image with letterboxing. Drag a rectangle starting inside it. Logical widget coordinates are converted into physical decoded image pixels, including reverse drags, clipped endpoints and resized viewports. The dashed overlay is only a selection indicator; it is not written into the image.
- **Обрезать** crops to the selection. **Пикселизация** changes only its pixels using 16-pixel mean-color blocks. Both execute off the GTK thread and keep exactly one previous image for **Отменить правку**. Undo consumes that history; there is no redo stack. New captures reset editing history. Pixelation is visual, with no guarantee of irreversible information erasure.
- **Сохранить PNG** opens a modal FileChooserNative. Only an explicitly accepted local `.png` path is written, using a background encoder/write. Cancel and invalid extensions leave files untouched. The exact selected path is used; the editor never silently redirects to another extension/path. Existing-file overwrite behavior belongs to the native Save chooser. A write error is reported without claiming that an existing file was preserved.
- **Копировать снимок** offers the current full image as a GDK texture. Copying only a selected region requires cropping first. **Распознать текст** runs local Tesseract rus+eng; **QR-код** runs local rqrr. Those use the selection if present, otherwise the whole current image. Results appear as plain read-only text, with a separate **Копировать текст** action. QR content is not opened or executed. Empty results, cancellation and errors have distinct messages. Previous OCR/QR text is cleared before another recognition so failed work cannot masquerade as fresh output.

The editor performs no network upload. A screenshot request is local portal IPC; OCR uses the installed offline process. Clipboard changes happen only from explicit copy buttons. Every action callback and worker reservation is guarded against smoke mode. Construction, preview and development tests perform no such effects.

## Resources and lifecycle

Each editor permits one active worker and one bounded result slot. Save reserves that slot while its chooser is open, preventing image changes beneath an accepted save. A request sequence plus image revision rejects stale completions. Closing the window invalidates requests, closes an active chooser and stops result polling. Existing portal/OCR jobs retain their backend deadlines; closing the window does not yet cancel those backend operations immediately. They cannot update a closed editor or copy data automatically.

There is no permanent polling timer: a 50 ms GLib poll exists only while work is active. All capture, raster editing, encoding/file writes and recognition run outside the GTK main thread. The texture's GBytes owns an Arc of the immutable RGBA raster, avoiding a full pixel copy on display. Current image and one undo image share ownership where possible; a newly computed image and clipboard content may temporarily retain additional bounded images. The backend's pixel/byte limits apply; this is not a total-process memory guarantee.

Controls use a wrapping FlowBox; status/instructions and recognized text wrap rather than increasing minimum width. The ordinary window has a 560-pixel minimum width, default 900×720. It is not registered with the compositor as `tidemark-overlay`; it does not implement a fullscreen cross-monitor selection surface, island self-exclusion, annotation arrows/text/undo history beyond one raster edit, or screen recording. Those remain separate parity work.

## Evidence and native gates

The state/geometry tests first failed for missing APIs, then six tests passed with `rustc --edition 2021 -D warnings --test ...`. They cover letterbox offsets, reverse and out-of-bounds dragging, zero/NaN geometry, image-coordinate stability on resize, one active job, stale completion rejection, close invalidation and the smoke gate for all worker operations. The existing Windows app suite passes 44 tests. Both new files pass rustfmt.

The Windows app build does not compile GTK UI code. Native GTK type checking, clippy and rendering are pending root integration/Ubuntu CI. Native smoke must open the synthetic preview and inspect 560-pixel/default allocations; capture/save/clipboard/OCR actions must stay disabled in smoke. Real portal consent, capture dimensions, drag precision and native Save/clipboard behavior still need explicit Ubuntu manual testing. No live screenshots, user images, browser actions or user clipboard calls were used to develop this module.

## API references

- [GTK FileChooserNative](https://docs.rs/gtk4/0.9.7/gtk4/struct.FileChooserNative.html): native Save chooser and response handling. Its deprecated constructors are narrowly allowed here because this requested API remains available on the selected GTK version.
- [GTK GestureDrag](https://docs.rs/gtk4/0.9.7/gtk4/struct.GestureDrag.html): drag offsets are relative to the starting point.
- [GDK Clipboard](https://docs.rs/gdk4/0.9.6/gdk4/struct.Clipboard.html): explicit texture/text content setters.
- Local pinned GTK4 0.9.7, GDK4 0.9.6 and GLib 0.20 source were inspected for chooser, clipboard and owned-byte method signatures; online GTK reference cross-checks were primary documentation.
