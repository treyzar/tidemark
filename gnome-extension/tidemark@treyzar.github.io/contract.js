// Pure constants and helpers of the org.tidemark.Shell1 contract.
// No gi:// imports: this module also runs under Node for the contract tests.
// Wire names must match crates/tidemark-core/src/shell.rs (checked by
// gnome-extension/tests/contract.test.js).

export const CONTRACT_VERSION = 1;

export const BUS_NAME = 'org.tidemark.Shell';
export const OBJECT_PATH = '/org/tidemark/Shell';
export const INTERFACE = 'org.tidemark.Shell1';
export const ERROR_PREFIX = 'org.tidemark.Shell1.Error';
export const ERROR_NAMES = {
    access_denied: 'AccessDenied',
    unsupported: 'Unsupported',
    failed: 'Failed',
    invalid_args: 'InvalidArgs',
};

// The Linux app (apps/linux): GTK application id and executable.
export const APP_ID = 'io.github.treyzar.Tidemark';
export const EXECUTABLE = 'tidemark';
// Launched by absolute path, never looked up in PATH.
export const CLIENT_PATH = '/usr/bin/tidemark';
// Developer override, read from gnome-shell's own environment (for example
// ~/.config/environment.d/). Must be an absolute path to an executable; it is
// logged when used. Unset in normal installs.
export const DEV_CLIENT_ENV = 'TIDEMARK_ADAPTER_CLIENT';
// Set in the client's environment; value = CONTRACT_VERSION.
export const ENV_MARKER = 'TIDEMARK_SHELL_ADAPTER';
// Arguments a caller may ask RequestLaunch to pass on.
export const LAUNCH_ARGS_ALLOWED = ['--settings'];

// Window roles are recognised by the exact GTK window title of windows owned
// by the launched client. Any other window of the client is "settings".
export const ISLAND_TITLE = 'tidemark-island';
export const OVERLAY_TITLE = 'tidemark-overlay';

// shell.rs Capability, in declaration order.
export const CAPABILITIES = [
    'place_windows', 'input_region', 'focus_tracking', 'window_actions',
    'fullscreen_tracking', 'maximized_tracking', 'clipboard_read', 'clipboard_write',
    'screenshot', 'capture_hiding', 'global_shortcuts', 'keyboard_layout', 'caps_lock', 'brightness',
];

// What this adapter version really implements. input_region is done by the
// client itself (GTK surface input region); screenshot, capture_hiding and
// brightness are not implemented yet and answer Unsupported.
export const IMPLEMENTED_CAPABILITIES = [
    'place_windows', 'focus_tracking', 'window_actions', 'fullscreen_tracking',
    'maximized_tracking', 'clipboard_read', 'clipboard_write', 'global_shortcuts',
    'keyboard_layout', 'caps_lock',
];

export const ROLES = ['island', 'overlay', 'settings'];
export const WINDOW_ACTIONS = [
    'activate', 'minimize', 'toggle_maximize', 'set_above', 'set_opacity', 'tile_left', 'tile_right',
];
export const SHORTCUT_IDS = ['launcher', 'toggle_theme', 'like_track'];
// Linux defaults chosen to avoid Ubuntu's Alt+Space (window menu) and
// Ctrl+Alt+T (terminal). The client sends them in BindShortcuts.
export const DEFAULT_SHORTCUTS = {
    launcher: '<Super><Alt>space',
    toggle_theme: '<Super><Alt>t',
    like_track: '<Super><Alt>l',
};

// a{sv} layouts: key = serde field name, value = D-Bus type.
export const FOCUS_FIELDS = {
    id: 't', app_id: 's', title: 's', pid: 'u', icon: 's',
    maximized: 'b', above: 'b', opacity: 'y', rect: '(iiii)', monitor: 'u',
};
// Option<_> fields: omitted from the dict when None.
export const OPTIONAL_FOCUS_FIELDS = ['pid', 'icon'];
export const WINDOW_STATE_FIELDS = { maximized: 'b', above: 'b', opacity: 'y', minimized: 'b' };

/**
 * Plain object -> {key: [D-Bus type, value]} for an a{sv}. `null` (Rust
 * `None`, e.g. FocusChanged with no focused app) is the EMPTY dict; a
 * non-empty dict always has every non-Option field. Option fields that are
 * None are left out. Rect becomes [x, y, w, h] for (iiii).
 * Rust decoding rule: empty a{sv} => None, otherwise Some(struct).
 */
export function toWireDict(obj, fields) {
    const dict = {};
    if (obj === null || obj === undefined)
        return dict;
    for (const [key, type] of Object.entries(fields)) {
        let value = obj[key];
        if (value === null || value === undefined)
            continue;
        if (type === '(iiii)')
            value = [value.x, value.y, value.w, value.h];
        dict[key] = [type, value];
    }
    return dict;
}

// Which capability each privileged method needs. When a method answers
// org.tidemark.Shell1.Error.Unsupported, the error message is exactly this
// capability's wire name (never prose), so the Rust client maps it with
// serde into ShellError::Unsupported(Capability).
export const METHOD_CAPABILITY = {
    Place: 'place_windows',
    GetFocus: 'focus_tracking',
    WindowAction: 'window_actions',
    GetClipboard: 'clipboard_read',
    SetClipboard: 'clipboard_write',
    Screenshot: 'screenshot',
    SetCaptureHidden: 'capture_hiding',
    BindShortcuts: 'global_shortcuts',
};

// D-Bus signal name -> [serde tag of ShellSignal, D-Bus signature].
export const SIGNALS = {
    FocusChanged: ['focus_changed', '(a{sv})'],
    FullscreenChanged: ['fullscreen_changed', '(ub)'],
    MaximizedTopChanged: ['maximized_top_changed', '(b)'],
    ShortcutActivated: ['shortcut_activated', '(s)'],
    ClipboardChanged: ['clipboard_changed', '(asb)'],
    LayoutChanged: ['layout_changed', '(s)'],
    CapsLock: ['caps_lock', '(b)'],
    Brightness: ['brightness', '(d)'],
};

export const PASSWORD_HINT_MIME = 'x-kde-passwordManagerHint';
export const MAX_CLIPBOARD_BYTES = 64 * 1024 * 1024;
// Reads are bounded: Meta.Selection.transfer_async(size = MAX + 1) stops after
// that many bytes, and the transfer is cancelled after this deadline.
export const CLIPBOARD_READ_TIMEOUT_MS = 5000;
// Lowest opacity a window action may set (about 10 %), so a window can't be
// made invisible by accident.
export const MIN_OPACITY = 26;
export const MAX_WINDOW_SIZE = 16384;
// A focus change to the island this soon after it was mapped is undone.
export const FOCUS_GUARD_MS = 1000;
// Client restart after a crash (non-zero exit or signal).
export const RESTART_BACKOFF_MS = [1000, 2000, 5000, 10000, 30000];
export const STABLE_RUN_MS = 60000;
export const RELAUNCH_COOLDOWN_MS = 2000;
// A child that exits with status 0 this soon, without Hello, handed its
// activation to an already running (untrusted) instance: ClientState blocked.
export const EARLY_EXIT_MS = 5000;
// RequestHandover waits this long for the old instance to leave the bus.
export const HANDOVER_TIMEOUT_MS = 30000;
export const CLIENT_STATES = ['none', 'starting', 'running', 'blocked', 'failed'];

/** Which binary to launch: {path|null, override}. A bad override launches nothing. */
export function resolveClientPath(override, isExecutable) {
    if (override)
        return { path: override.startsWith('/') && isExecutable(override) ? override : null, override: true };
    return { path: isExecutable(CLIENT_PATH) ? CLIENT_PATH : null, override: false };
}

export function roleForTitle(title) {
    if (title === ISLAND_TITLE)
        return 'island';
    if (title === OVERLAY_TITLE)
        return 'overlay';
    return 'settings';
}

/** [x, y, w, h] or {x, y, w, h} -> {x, y, w, h}, or null if malformed. */
export function toRect(value) {
    const [x, y, w, h] = Array.isArray(value) ? value : [value?.x, value?.y, value?.w, value?.h];
    if (![x, y, w, h].every(Number.isInteger))
        return null;
    return { x, y, w, h };
}

export function validSize(rect) {
    return rect.w > 0 && rect.h > 0 && rect.w <= MAX_WINDOW_SIZE && rect.h <= MAX_WINDOW_SIZE;
}

/**
 * Top-centre placement on the primary monitor, just below the top bar.
 * For the island role, Place's rect.x is ignored and rect.y is the offset
 * below the top bar (config `top_offset`).
 */
export function islandPosition({ monitor, panelBottom, width, offsetY }) {
    return {
        x: monitor.x + Math.floor((monitor.width - width) / 2),
        y: Math.max(monitor.y, panelBottom) + Math.max(0, offsetY),
    };
}

/** Left or right half of a work area (Windows "snap"). */
export function tileRect(workArea, side) {
    const half = Math.floor(workArea.width / 2);
    return side === 'left'
        ? { x: workArea.x, y: workArea.y, w: half, h: workArea.height }
        : { x: workArea.x + half, y: workArea.y, w: workArea.width - half, h: workArea.height };
}

/** The focused window reaches the top of the work area across its centre. */
export function coversIslandStrip(rect, workArea, maximized) {
    if (maximized)
        return true;
    const centre = workArea.x + Math.floor(workArea.width / 2);
    return rect.y <= workArea.y && rect.x <= centre && rect.x + rect.w >= centre;
}

export function isSensitive(mimes) {
    return mimes.some(m => m.split(';')[0].trim() === PASSWORD_HINT_MIME);
}

const MIME_RE = /^[A-Za-z0-9][A-Za-z0-9!#$&^_.+-]*\/[A-Za-z0-9][A-Za-z0-9!#$&^_.+-]*(;.*)?$/;
export function validMime(mime) {
    return typeof mime === 'string' && mime.length <= 255 && MIME_RE.test(mime);
}

/** "ru" / "us" / "en" -> "RU" / "US" / "EN"; at most 3 characters. */
export function normalizeLayout(shortName) {
    if (typeof shortName !== 'string')
        return null;
    const s = shortName.trim().toUpperCase().slice(0, 3);
    return s.length ? s : null;
}

export function validAccelerator(accel) {
    return typeof accel === 'string' && accel.length > 0 && accel.length <= 64 && !/[\0\n]/.test(accel);
}
