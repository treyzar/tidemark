# Linux launcher component

New files only: `apps/linux/src/ui/launcher.rs` and
`apps/linux/src/services/launcher.rs`. No manifests, module registries, header or
shortcut wiring are changed by this component.

## GTK integration

Register `pub mod launcher;` in the existing UI module, then construct
`LauncherView::build(shared: Shared)`. Mount its public `root: gtk::Box` in the
parent search popover/window. Call `focus_entry()` after showing it. Use
`connect_dismiss(callback)` to close the parent on Escape or a successful launch;
capture the popover/window weakly to avoid a reference cycle. The UI locally
imports the new services file by path, so no services/mod.rs change is necessary.

The layout uses the existing page/header/list CSS and native GTK widgets at the
parent's 560-pixel width. A search entry sits over icon/title/subtitle rows in a
height-bounded scrolling list. Up/Down cycles selection and scrolls it into view,
Enter activates the selected row, and Escape requests dismissal. Mouse activation
is explicit. A calculator selection copies its value and keeps search open.

## Behavior

Installed apps come from `gio::AppInfo::all`, filtered by `should_show`, deduplicated
by desktop ID, and capped at 4096. Empty search shows alphabetically sorted apps.
Case-insensitive, Unicode-safe matching ranks exact name, prefix, word prefix,
substring, then initials, following the existing Windows score hierarchy. At most
20 total result rows are rendered.

The existing Windows arithmetic parser/formatter is reused verbatim with input
limits (256 bytes, at most 32 opening parentheses) to prevent pathological parsing.
It supports + - * / % ^, parentheses and decimal comma, rejects nonfinite results,
and formats at most eight decimal places. Calculation takes the first result row.

A typed absolute existing path gets its own result row; relative paths and URLs
are never interpreted as filesystem paths. Existence is checked again when the
user activates it. GIO opens the file URI using its registered default handler.

Web search uses `Config.search_url` (Google by default). Only an HTTP(S) URL with
`{q}` outside the authority is accepted. Scheme/authority/userinfo/port/host/braces
are validated, input is capped to 1024 bytes, and UTF-8 query bytes are percent
encoded. Queries are always data and never shell commands. Web results are opened
only after the user selects/activates them.

App activation uses the selected stored `AppInfo.launch` object. Desktop Exec
lines and query text are never parsed, concatenated into commands or sent to a
shell. File and web activation use `AppInfo::launch_default_for_uri`.

Construction reads local desktop metadata only: no application launch, clipboard
write or network request occurs. Smoke-mode activation is also a no-op. Tests do
not invoke any live desktop operation.

## Verification

Six pure helper tests compile and pass on Windows with warnings denied:

```
rustc --edition 2021 -D warnings --test apps/linux/src/services/launcher.rs -o <temporary-test-executable>
<temporary-test-executable>
```

Tests cover scoring precedence/case/Unicode/empty-query bounds, original calculator
behavior and limits, escaped UTF-8 web queries and invalid templates, absolute
existing path validation, and bounded keyboard selection. A RED run with missing
scoring produced two expected failures; implementing the Windows ranking resolved
them. Direct rustfmt formatting/checking covers both new files.

GTK production APIs still require the parent's native Ubuntu compile after module
integration, followed by smoke construction. This Windows host has not launched
applications, a browser, files, or the user's desktop while implementing/testing.
