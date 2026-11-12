# Ubuntu foundation: shared core + workspace — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Cargo workspace in which a platform-neutral `tidemark-core` crate holds the portable models and the new Linux contracts, while the Windows app keeps building and behaving exactly as v1.3.0.

**Architecture:**
- The Windows package stays at the repository root (`src/`, `build.rs`, scripts unchanged) and is the workspace's `default-members`.
- Portable modules move to `crates/tidemark-core/src`, so each has a single owning source. The Windows crate re-exports them at its root (`use tidemark_core::layout;`), so every `crate::layout::…` path keeps compiling.
- The `windows` crate becomes a `cfg(windows)` target dependency of the root package only.

**Tech Stack:** Rust 2021, serde/serde_json/toml (already used). The core adds no new runtime dependencies.

**Spec:** `docs/ubuntu/architecture.md` (§3, §4.1–4.3, as simplified by the parent: no mass move to `apps/windows`).

## Global Constraints

- Windows output names, `%APPDATA%\win-island` data and config path, and behaviour stay unchanged.
- `cargo build --release` and `cargo test --release` at the root must pass on Windows. Baseline before the change: 9 + 140 passed, 8 ignored (157 total).
- No test reads or writes the real user config or data directories.
- Linux paths:
  - config: `$XDG_CONFIG_HOME/tidemark` (only if absolute), else `$HOME/.config/tidemark`;
  - data: `$XDG_DATA_HOME/tidemark`, else `$HOME/.local/share/tidemark`.
- `tidemark-core` must not depend on `windows`, GTK, zbus or any OS API crate. It must build on Linux.
- `crates/tidemark-mpris` (owned by another agent) is **not** a workspace member in this milestone.
- `.github/workflows/ubuntu.yml` and packaging docs belong to the parent; don't touch them.

## Review Focus

- `XDG_CONFIG_HOME` set to a relative path must be ignored (XDG spec). Test: `paths::tests::relative_xdg_is_ignored`.
- `HOME` unset on Linux must not panic; fall back to `.`. Test: `paths::tests::no_home_falls_back_to_cwd`.
- A corrupted `config.toml` must still load defaults and must not be overwritten. Test: `config::tests::broken_file_loads_defaults_without_overwrite` (temp dir).
- Wire-name drift in the serialized contracts would silently break the GJS adapter. Tests pin exact JSON strings: `shell::tests::wire_names_are_stable`, `event::tests::event_json_is_stable`.
- The agent `Message` must still accept old lines with missing optional fields. Test: `agent_wire::tests::minimal_line_parses`.

---

### Task 1: Workspace + core crate with moved portable modules

**Files:**
- Modify: `Cargo.toml` (add `[workspace] members = [".", "crates/tidemark-core"]`, `default-members = ["."]`; move `windows*` deps under `[target.'cfg(windows)'.dependencies]`; add `tidemark-core = { path = "crates/tidemark-core" }`)
- Create: `crates/tidemark-core/Cargo.toml`, `src/lib.rs`
- Move (git mv semantics, content unchanged except imports): `anim`, `settings_motion`, `layout`, `textinput`, `timer`, `media_artwork`, `notes`, `clip_history`, `config`, `text_tools` → `crates/tidemark-core/src/`
- Create: `crates/tidemark-core/src/keyboard.rs` (`EN`, `RU`, `pub fn swap_layout(&str) -> String`, taken from `launcher.rs`, along with its test)
- Modify: `src/main.rs` (replace those `mod x;` with `use tidemark_core::{…};`), `src/launcher.rs` (`pub use tidemark_core::keyboard::swap_layout;`, tables removed)

- [ ] Run `cargo test --release`. Expected: the root crate plus the core together report 157 tests (149 runnable), 0 failed.

### Task 2: Paths (Windows-compatible and XDG)

**Files:** Create `crates/tidemark-core/src/paths.rs`. Modify `config.rs`, `notes.rs`, `clip_history.rs` in the core.

**Interfaces (produces):**
- `pub struct AppDirs { pub config: PathBuf, pub data: PathBuf }`
- `pub enum Platform { Windows, Unix }`, `Platform::current()`
- `pub fn resolve(platform: Platform, env: impl Fn(&str) -> Option<OsString>) -> AppDirs`
- `pub fn current() -> AppDirs`
- `Config::dir()` = `current().config` (Windows: unchanged `%APPDATA%\win-island`)
- `Config::data_dir()` = `current().data`
- `Config::load_from(&Path) -> Config`, `Config::save_to(&Path) -> io::Result<()>`
- `notes::Store::load_from` / `save_to`; `clip_history::History::load_from(dir)` / `save_to(dir)`
- notes and clipboard go under `data`; `config.toml` under `config`

- [ ] Tests:
  - `windows_uses_appdata_for_both`
  - `linux_uses_xdg_dirs`
  - `linux_defaults_under_home`
  - `relative_xdg_is_ignored`
  - `no_home_falls_back_to_cwd`
  - config round-trip through a temp dir (`std::env::temp_dir()` + unique name, removed afterwards)
  - `broken_file_loads_defaults_without_overwrite`
  - notes and history round-trip in a temp dir

### Task 3: Serializable Linux contracts

**Files:** Create `crates/tidemark-core/src/{shell.rs, event.rs, service.rs, agent_wire.rs}`. Modify `src/agent.rs` (Windows) to use `tidemark_core::agent_wire::Message`.

**Interfaces (produces):**
- `shell.rs`:
  - `Capability` (snake_case wire names, matching `Shell1.Capabilities`)
  - `WindowRole { Island, Overlay, Settings }`
  - `Rect { x, y, w, h: i32 }`
  - `FocusInfo`
  - `WindowAction` (tag `"action"`, payload `"arg"`)
  - `WindowState`
  - `ShortcutId { Launcher, ToggleTheme, LikeTrack }`
  - `ShortcutBinding { id, accel }`, `BindResult { id, bound }`
  - `ClipboardOffer { mimes, sensitive }`
  - `ShellSignal` (tag `"signal"`)
  - `ShellError { Unsupported(Capability), Failed(String) }`
  - `trait ShellBackend`: `capabilities`, `place`, `focus`, `window_action`, `get_clipboard`, `set_clipboard`, `screenshot`, `set_capture_hidden`, `bind_shortcuts`
  - `NullBackend` returns `Unsupported` for everything and has an empty capability set
- `event.rs`:
  - `Osd`
  - `ServiceId`
  - `Event { Shell(ShellSignal), Osd(Osd), ServiceError { service, message } }`, tag `"type"`
- `service.rs`:
  - `trait EventSink: Send { fn emit(&self, e: Event) -> bool }`
  - `trait Service { type Command; fn send(&self, c: Self::Command); }`
  - `impl EventSink for std::sync::mpsc::Sender<Event>`
- `agent_wire.rs`:
  - `Message { kind, source, title, text, options }` (Serialize + Deserialize, defaults as today)
  - `Reply { choice: Option<usize> }`

- [ ] Tests:
  - `wire_names_are_stable` (exact JSON strings)
  - round-trip of every enum variant
  - `null_backend_reports_unsupported`
  - `event_json_is_stable`
  - `mpsc_sender_is_event_sink`
  - `minimal_line_parses`
  - `reply_null_and_index`

### Task 4: Verify and document

- [ ] `cargo test --release` (root; Windows): all pass, count ≥ baseline.
- [ ] `cargo test -p tidemark-core --release`: all pass.
- [ ] `cargo build --release`: `target/release/win-island.exe`, `island-hook.exe`.
- [ ] `cargo check -p tidemark-core --target x86_64-unknown-linux-gnu` if that target is installed; otherwise note that CI covers it.
- [ ] Update `docs/ubuntu/architecture.md` §3 to describe the simplified root layout.
- [ ] Don't commit until the parent asks.
