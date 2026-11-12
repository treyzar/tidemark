# GNOME Shell adapter (contract v1 slice) — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A GJS extension that implements `org.tidemark.Shell1` for the Tidemark client it launches itself, on GNOME 46 (baseline) with a 49+ compatibility path.

**Architecture:**
- `adapter.js`: pure logic, testable in Node with a fake platform.
- `gnomePlatform.js`: Mutter/Shell calls, with version shims.
- `dbusService.js`: export, peer-PID authentication, Variants, unicast signals.
- The contract source of truth is `crates/tidemark-core/src/shell.rs`, mirrored in `org.tidemark.Shell1.xml` and `contract.js` and pinned by cross-language tests.

**Tech Stack:** GJS ESM extension API (GNOME 45+); Node `node:test` for CI.

**Spec:** `docs/ubuntu/architecture.md` §2, §4.3; parent decisions of 2026-11-12:
- AppID `io.github.treyzar.Tidemark`, binary `/usr/bin/tidemark`;
- shortcuts Super+Alt+Space / T / L;
- island below the top bar;
- `.deb`; Tesseract.

## Global Constraints

- Only `gnome-extension/**` and this plan are touched. No Rust app or UI, no workflows, no packaging.
- Mutter API facts were checked in the headers: `meta-wayland-client.h` and `window.h` at tags 46.0 and 49.0.
- Placement, focus and window actions come first. Capture and brightness answer `Unsupported` and are not advertised.
- Privileged methods require the caller to be the PID that this adapter launched from `/usr/bin/tidemark`, checked under the current client generation.
- No user gsettings are changed, nothing is enabled locally, nothing is published.

## Review Focus

- Untrusted Tidemark already owns the GApplication id → the child exits 0 early → the state must become `blocked` without a restart loop; a handover only after the old process leaves the bus. Tests: `an untrusted instance owning the app id blocks; handover after it exits`, `handover is cancelled when the old instance does not exit`.
- An authenticated sender's client exits or restarts during the async PID lookup → refused. Covered by generation checks; tested in `generation changes with every client change`. The real D-Bus path is only covered by acceptance step 3.
- A secret clipboard entry → neither announced nor readable, including when the owner changes mid-read. Test: `clipboard: secrets are neither announced nor readable`.
- A shortcut already owned by GNOME → reported `false`, never stolen. Test: `shortcuts: validation, conflicts reported, activation, rebinding`.
- Island mapping steals focus → focus is returned to the previous app within 1 s. Test: `mapping the island does not steal focus`.

---

### Task 1: Contract files
- [x] `org.tidemark.Shell1.xml` with properties `Version`, `Capabilities`, `ClientState`; methods `RequestLaunch`, `RequestHandover`, `Hello`, `Place`, `GetFocus`, `WindowAction`, `GetClipboard`, `SetClipboard`, `Screenshot`, `SetCaptureHidden`, `BindShortcuts`; the 8 `ShellSignal` signals.
- [x] `contract.js`: constants and pure helpers.
- [x] `tests/contract.test.js`: XML ↔ `contract.js` ↔ `shell.rs`.

### Task 2: Adapter logic + mocks
- [x] `adapter.js`: lifecycle and state machine, authentication decision, roles, placement, focus, fullscreen, maximized, actions, shortcuts, clipboard, keyboard, `Unsupported`.
- [x] `tests/adapter.test.js` with `FakePlatform`.

### Task 3: GNOME bindings
- [x] `gnomePlatform.js`:
  - 46–48: `WaylandClient.new` + `spawnv` + `client.hide_from_window_list`;
  - 49+: `new_subprocess` + `window.hide_from_window_list`;
  - maximize flags vs flagless;
  - `grab_accelerator` + `allowKeybinding`;
  - St clipboard and `Meta.Selection` owner-changed;
  - InputSourceManager; Clutter keymap.
- [x] `dbusService.js`: `wrapJSObject`, `*Async` handlers, `GetConnectionUnixProcessID` with generation, unicast `emit_signal`, `bus_watch_name` for handover.
- [x] `extension.js`, `metadata.json` (shell-version 46–50).

### Task 4: Verify and document
- [x] `npm test --prefix gnome-extension` passes (Windows, Node 24); `node --check` on every module.
- [x] `gnome-extension/README.md`: contract, access rules, startup and handover protocol, real GNOME acceptance steps, limits.
- [ ] Real GNOME acceptance on 24.04 (Wayland + Xorg) and 26.04: **not run**; owned by QA/parent.
