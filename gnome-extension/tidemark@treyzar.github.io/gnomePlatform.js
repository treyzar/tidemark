// Everything that touches GNOME Shell / Mutter. adapter.js calls these
// methods; GNOME signals are forwarded to the adapter's on*() handlers.
//
// Version differences (checked against Mutter headers 46.0 and 49.0):
// - launching a trusted Wayland client:
//     46-48: Meta.WaylandClient.new(context, launcher) + client.spawnv(display, argv)
//     49+:   Meta.WaylandClient.new_subprocess(context, launcher, argv) + client.get_subprocess()
// - hiding from the window list / Alt+Tab:
//     46-48: client.hide_from_window_list(window)
//     49+:   window.hide_from_window_list()
// - maximize: 46-48 take Meta.MaximizeFlags and have get_maximized();
//     49+ take no flags and have is_maximized().

import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as Keyboard from 'resource:///org/gnome/shell/ui/status/keyboard.js';

import * as C from './contract.js';

// The launched client outlives disable()/enable() cycles (GNOME disables
// extensions while the screen is locked), so it can be re-adopted instead of
// killed and restarted. Module state lives as long as the gnome-shell process.
// {pid, proc, wayland: Meta.WaylandClient|null, exited, onExit}
let liveClient = null;

const TRACKED_TYPES = [
    Meta.WindowType.NORMAL, Meta.WindowType.DIALOG, Meta.WindowType.MODAL_DIALOG, Meta.WindowType.UTILITY,
];

export class GnomePlatform {
    constructor(log) {
        this._log = log;
        this._adapter = null;
        this._signals = []; // [object, id]
        this._windowSignals = new Map(); // MetaWindow -> [ids]
        this._timeouts = new Set();
        this._keymap = Clutter.get_default_backend().get_default_seat().get_keymap();
    }

    // ---- wiring -------------------------------------------------------------------

    connect(adapter) {
        this._adapter = adapter;
        const display = global.display;
        this._on(display, 'window-created', (_d, w) => {
            this._watchWindow(w);
            adapter.onWindowChanged(w);
        });
        this._on(display, 'notify::focus-window', () => adapter.onFocusChanged());
        this._on(display, 'in-fullscreen-changed', () => adapter.onFullscreenChanged());
        this._on(display, 'accelerator-activated', (_d, action) => adapter.onAccelerator(action));
        this._on(display.get_selection(), 'owner-changed', (_s, type) => {
            if (type === Meta.SelectionType.SELECTION_CLIPBOARD)
                adapter.onClipboardChanged();
        });
        this._on(Main.layoutManager, 'monitors-changed', () => adapter.onMonitorsChanged());
        this._on(Main.layoutManager.panelBox, 'notify::height', () => adapter.onMonitorsChanged());
        this._on(Main.layoutManager.panelBox, 'notify::visible', () => adapter.onMonitorsChanged());
        this._on(Keyboard.getInputSourceManager(), 'current-source-changed', () => adapter.onLayoutChanged());
        this._on(this._keymap, 'state-changed', () => adapter.onCapsLockChanged());
        this.forEachWindow(w => this._watchWindow(w));
    }

    disconnect() {
        for (const [obj, id] of this._signals)
            obj.disconnect(id);
        this._signals = [];
        for (const [w, ids] of this._windowSignals)
            ids.forEach(id => w.disconnect(id));
        this._windowSignals.clear();
        for (const id of this._timeouts)
            GLib.source_remove(id);
        this._timeouts.clear();
        this._adapter = null;
    }

    _on(obj, signal, fn) {
        this._signals.push([obj, obj.connect(signal, fn)]);
    }

    _watchWindow(w) {
        if (this._windowSignals.has(w))
            return;
        const a = this._adapter;
        const ids = [];
        const on = (signal, fn) => {
            try {
                ids.push(w.connect(signal, fn));
            } catch (e) {
                // A property renamed in a newer Mutter must not break the rest.
                this._log(`window signal ${signal} unavailable: ${e.message}`);
            }
        };
        on('notify::title', () => {
            a.onWindowChanged(w);
            a.onWindowState(w);
        });
        on('size-changed', () => a.onWindowGeometry(w));
        on('position-changed', () => a.onWindowGeometry(w));
        on('shown', () => a.onWindowShown(w));
        for (const prop of ['above', 'minimized', 'fullscreen', 'maximized-horizontally', 'maximized-vertically'])
            on(`notify::${prop}`, () => a.onWindowState(w));
        on('unmanaged', () => {
            const own = this._windowSignals.get(w) ?? [];
            own.forEach(id => w.disconnect(id));
            this._windowSignals.delete(w);
            a.onWindowGone(w);
        });
        this._windowSignals.set(w, ids);
    }

    // ---- time -----------------------------------------------------------------------

    now() {
        return GLib.get_monotonic_time() / 1000;
    }

    setTimeout(ms, fn) {
        const id = GLib.timeout_add(GLib.PRIORITY_DEFAULT, ms, () => {
            this._timeouts.delete(id);
            fn();
            return GLib.SOURCE_REMOVE;
        });
        this._timeouts.add(id);
        return id;
    }

    clearTimeout(id) {
        if (this._timeouts.delete(id))
            GLib.source_remove(id);
    }

    // ---- client process ---------------------------------------------------------------

    supports(_capability) {
        return true;
    }

    clientPath() {
        return C.resolveClientPath(GLib.getenv(C.DEV_CLIENT_ENV),
            // file_test with OR-ed flags is true if *any* holds, so test separately.
            path => GLib.file_test(path, GLib.FileTest.IS_REGULAR) && GLib.file_test(path, GLib.FileTest.IS_EXECUTABLE));
    }

    spawnClient(argv, onExit) {
        const launcher = new Gio.SubprocessLauncher({ flags: Gio.SubprocessFlags.NONE });
        launcher.setenv(C.ENV_MARKER, String(C.CONTRACT_VERSION), true);
        let wayland = null;
        let proc;
        if (isWaylandCompositor()) {
            if (typeof Meta.WaylandClient.new_subprocess === 'function') {
                wayland = Meta.WaylandClient.new_subprocess(global.context, launcher, argv);
                proc = wayland.get_subprocess();
            } else {
                wayland = Meta.WaylandClient.new(global.context, launcher);
                proc = wayland.spawnv(global.display, argv);
            }
        } else {
            proc = launcher.spawnv(argv);
        }
        const record = { pid: Number(proc.get_identifier()), proc, wayland, exited: false, onExit };
        liveClient = record;
        proc.wait_async(null, (p, res) => {
            try {
                p.wait_finish(res);
            } catch (e) {
                this._log(`waiting for client failed: ${e.message}`);
            }
            record.exited = true;
            if (liveClient === record)
                liveClient = null;
            const signaled = p.get_if_signaled();
            const code = p.get_if_exited() ? p.get_exit_status() : -1;
            record.onExit?.({ code, signaled });
        });
        return { pid: record.pid };
    }

    adoptClient(onExit) {
        if (!liveClient || liveClient.exited)
            return null;
        liveClient.onExit = onExit;
        return { pid: liveClient.pid };
    }

    releaseClient() {
        if (liveClient)
            liveClient.onExit = null;
    }

    // ---- windows -----------------------------------------------------------------------

    forEachWindow(fn) {
        for (const actor of global.get_window_actors()) {
            const w = actor.get_meta_window();
            if (w)
                fn(w);
        }
    }

    windowById(id) {
        let found = null;
        this.forEachWindow(w => {
            if (w.get_id() === id)
                found = w;
        });
        return found;
    }

    windowId(w) {
        return w.get_id();
    }

    title(w) {
        return w.get_title() ?? '';
    }

    ownsWindow(w) {
        const client = liveClient;
        if (!client || client.exited)
            return false;
        if (client.wayland && w.get_client_type() === Meta.WindowClientType.WAYLAND)
            return client.wayland.owns_window(w);
        return w.get_pid() === client.pid;
    }

    isTracked(w) {
        return TRACKED_TYPES.includes(w.get_window_type()) && !w.is_skip_taskbar();
    }

    /**
     * Island/overlay: on all workspaces, above other windows, not in Alt+Tab.
     * Applied from a BEFORE_REDRAW later, not inside window-created: there
     * the window may not be stacked yet, and make_above()/stick() then hit
     * Mutter's "window->stack_position >= 0" assertion (seen on 46.2 in CI).
     */
    prepareChrome(w) {
        const apply = () => {
            if (!this._windowSignals.has(w))
                return; // unmanaged meanwhile
            w.stick();
            w.make_above();
            if (typeof w.hide_from_window_list === 'function')
                w.hide_from_window_list();
            else if (liveClient?.wayland && w.get_client_type() === Meta.WindowClientType.WAYLAND)
                liveClient.wayland.hide_from_window_list(w);
        };
        const laters = global.compositor?.get_laters?.();
        if (laters) {
            laters.add(Meta.LaterType.BEFORE_REDRAW, () => {
                apply();
                return GLib.SOURCE_REMOVE;
            });
        } else {
            GLib.idle_add(GLib.PRIORITY_DEFAULT, () => {
                apply();
                return GLib.SOURCE_REMOVE;
            });
        }
    }

    describe(w) {
        const r = w.get_frame_rect();
        const app = Shell.WindowTracker.get_default().get_window_app(w);
        let appId = '';
        let icon = null;
        if (app && !app.is_window_backed()) {
            appId = (app.get_id() ?? '').replace(/\.desktop$/, '');
            icon = app.get_app_info()?.get_icon()?.to_string() ?? null;
        }
        const pid = w.get_pid();
        const actor = w.get_compositor_private();
        return {
            id: w.get_id(),
            app_id: appId,
            title: w.get_title() ?? '',
            pid: pid > 0 ? pid : null,
            icon,
            maximized: isMaximized(w),
            above: w.is_above(),
            opacity: actor ? actor.opacity : 255,
            rect: { x: r.x, y: r.y, w: r.width, h: r.height },
            monitor: Math.max(0, w.get_monitor()),
            minimized: w.minimized,
            fullscreen: w.is_fullscreen(),
        };
    }

    focusWindow() {
        return global.display.focus_window;
    }

    primaryMonitor() {
        const index = global.display.get_primary_monitor();
        const g = global.display.get_monitor_geometry(index);
        return { index, x: g.x, y: g.y, width: g.width, height: g.height };
    }

    panelBottom() {
        const mon = this.primaryMonitor();
        const box = Main.layoutManager.panelBox;
        if (box && box.visible && box.height > 0)
            return Math.max(mon.y, box.y + box.height);
        return mon.y;
    }

    workArea(w) {
        const r = w.get_work_area_current_monitor();
        return { x: r.x, y: r.y, width: r.width, height: r.height };
    }

    monitorsInFullscreen() {
        return Main.layoutManager.monitors.map(m => !!m.inFullscreen);
    }

    moveFrame(w, x, y) {
        w.move_frame(false, x, y);
    }

    moveResizeFrame(w, x, y, width, height, userOp) {
        w.move_resize_frame(userOp, x, y, width, height);
    }

    activate(w) {
        Main.activateWindow(w);
    }

    minimize(w) {
        w.minimize();
    }

    setMaximized(w, on) {
        const flagsApi = typeof w.get_maximized === 'function';
        if (on)
            flagsApi ? w.maximize(Meta.MaximizeFlags.BOTH) : w.maximize();
        else
            flagsApi ? w.unmaximize(Meta.MaximizeFlags.BOTH) : w.unmaximize();
    }

    setAbove(w, on) {
        if (on)
            w.make_above();
        else
            w.unmake_above();
    }

    setOpacity(w, value) {
        const actor = w.get_compositor_private();
        if (actor)
            actor.opacity = value;
    }

    // ---- shortcuts ---------------------------------------------------------------------

    grabAccelerator(accel) {
        const action = global.display.grab_accelerator(accel, Meta.KeyBindingFlags.NONE);
        if (action === Meta.KeyBindingAction.NONE)
            return 0;
        Main.wm.allowKeybinding(Meta.external_binding_name_for_action(action),
            Shell.ActionMode.NORMAL | Shell.ActionMode.OVERVIEW);
        return action;
    }

    ungrabAccelerator(action) {
        global.display.ungrab_accelerator(action);
        Main.wm.allowKeybinding(Meta.external_binding_name_for_action(action), Shell.ActionMode.NONE);
    }

    // ---- clipboard ---------------------------------------------------------------------

    clipboardMimes() {
        return global.display.get_selection().get_mimetypes(Meta.SelectionType.SELECTION_CLIPBOARD) ?? [];
    }

    /**
     * Reads at most maxBytes + 1 bytes (Meta.Selection's `size` is a hard
     * limit: mutter 46 core/meta-selection.c stops reading when it is used
     * up), cancelled after CLIPBOARD_READ_TIMEOUT_MS. Rejects with
     * Error('too_large') when the provider offered more than maxBytes, so
     * unbounded provider data is never buffered in gnome-shell.
     */
    readClipboard(mime, maxBytes) {
        return new Promise((resolve, reject) => {
            const out = Gio.MemoryOutputStream.new_resizable();
            const cancellable = new Gio.Cancellable();
            let timer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, C.CLIPBOARD_READ_TIMEOUT_MS, () => {
                timer = 0;
                cancellable.cancel();
                return GLib.SOURCE_REMOVE;
            });
            global.display.get_selection().transfer_async(Meta.SelectionType.SELECTION_CLIPBOARD, mime,
                maxBytes + 1, out, cancellable, (selection, res) => {
                    if (timer) {
                        GLib.source_remove(timer);
                        timer = 0;
                    }
                    try {
                        selection.transfer_finish(res);
                        out.close(null);
                    } catch (e) {
                        reject(e);
                        return;
                    }
                    if (out.get_data_size() > maxBytes) {
                        reject(new Error('too_large'));
                        return;
                    }
                    resolve(out.steal_as_bytes().toArray());
                });
        });
    }

    writeClipboard(mime, bytes) {
        St.Clipboard.get_default().set_content(St.ClipboardType.CLIPBOARD, mime, new GLib.Bytes(bytes));
    }

    // ---- keyboard ----------------------------------------------------------------------

    currentLayout() {
        return Keyboard.getInputSourceManager().currentSource?.shortName ?? null;
    }

    capsLock() {
        return this._keymap.get_caps_lock_state();
    }
}

/**
 * Mutter 46-48 export Meta.is_wayland_compositor(); Mutter 50 (X11 session
 * removed) no longer does and is always a Wayland compositor. Found by the
 * headless harness on Ubuntu 26.04: calling it there threw and no client
 * was ever launched.
 */
function isWaylandCompositor() {
    return typeof Meta.is_wayland_compositor === 'function' ? Meta.is_wayland_compositor() : true;
}

function isMaximized(w) {
    if (typeof w.is_maximized === 'function')
        return w.is_maximized();
    return w.get_maximized() === Meta.MaximizeFlags.BOTH;
}
