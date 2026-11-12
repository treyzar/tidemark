// Adapter logic: client lifecycle, authentication decision, window roles,
// placement, focus/fullscreen/maximized tracking, window actions, shortcuts,
// clipboard privacy, keyboard state. No gi:// imports: every GNOME call goes
// through `platform` (gnomePlatform.js in GNOME Shell, a fake in the tests)
// and every signal through `sink.emit(name, args)` (dbusService.js).

import * as C from './contract.js';

export class AdapterError extends Error {
    /** kind: one of the keys of C.ERROR_NAMES. */
    constructor(kind, message) {
        super(message);
        this.kind = kind;
    }
}

const unsupported = capability => new AdapterError('unsupported', capability);
const failed = message => new AdapterError('failed', message);
const invalid = message => new AdapterError('invalid_args', message);

export class Adapter {
    constructor(platform, sink, log = () => {}) {
        this._p = platform;
        this._sink = sink;
        this._log = log;
        this._enabled = false;
        this._client = null; // {pid, startedAt, alive}
        // Bumped whenever the trusted client changes (launch, exit, adoption,
        // disable). Caller identities checked under an older generation are void.
        this._generation = 0;
        this._failures = 0;
        this._restartTimer = null;
        this._lastLaunchAt = -Infinity;
        this._state = 'none';
        this._handover = null; // {args, timer}
        this._windows = new Map(); // window id -> {w, role}
        this._island = null;
        this._islandMappedAt = -Infinity;
        this._islandRequest = null; // {y, w, h} from Place(island)
        this._lastExternal = null;
        this._focusKey = undefined;
        this._fullscreen = [];
        this._covered = false;
        this._grabs = new Map(); // accelerator action -> shortcut id
        this._opacity = new Map(); // window id -> {w, value, original}: changed by us
        this._layout = null;
        this._caps = false;
        // Bumped on every clipboard owner change; a read is only returned if
        // no change happened while it ran (catches A -> B -> A).
        this._clipboardGeneration = 0;
    }

    // ---- lifecycle -------------------------------------------------------
    //
    // ClientState (open D-Bus property):
    //   none     - no client; RequestLaunch/RequestHandover may start one
    //   starting - launched by us, has not called Hello yet
    //   running  - launched by us and said Hello
    //   blocked  - our child exited cleanly within EARLY_EXIT_MS without Hello:
    //              another (untrusted) Tidemark instance already owns the
    //              GApplication id and took the activation. Not restarted;
    //              that instance must save its state and call RequestHandover.
    //   failed   - not installed, or crashed RESTART_BACKOFF_MS.length times

    enable() {
        this._enabled = true;
        const adopted = this._p.adoptClient(status => this._onClientExit(status));
        if (adopted) {
            // Re-enabled in the same gnome-shell process (e.g. after the lock
            // screen): the same child process, which must say Hello again.
            this._client = { pid: adopted.pid, startedAt: this._p.now(), alive: true, helloed: false };
            this._clientChanged();
            this._setState('starting');
        } else {
            this.launch([]);
        }
        this._p.forEachWindow(w => this.onWindowChanged(w));
        // Initial state, not announced (the client gets a snapshot after Hello).
        this._fullscreen = this._p.monitorsInFullscreen();
        this._layout = C.normalizeLayout(this._p.currentLayout());
        this._caps = !!this._p.capsLock();
        const focus = this._p.focusWindow();
        if (focus && this._isExternal(focus))
            this._lastExternal = focus;
        this._covered = this._computeCovered();
    }

    /** Stops tracking. The client keeps running and is re-adopted on enable. */
    disable() {
        this._enabled = false;
        if (this._restartTimer !== null) {
            this._p.clearTimeout(this._restartTimer);
            this._restartTimer = null;
        }
        this._cancelHandover();
        this._releaseShortcuts();
        this._restoreOpacity();
        this._p.releaseClient();
        this._client = null;
        this._clientChanged();
        this._state = 'none';
        this._windows.clear();
        this._island = null;
        this._lastExternal = null;
    }

    get generation() {
        return this._generation;
    }

    get state() {
        return this._state;
    }

    _setState(state) {
        if (state !== this._state) {
            this._state = state;
            this._sink.stateChanged(state);
        }
    }

    _clientChanged() {
        this._generation++;
        this._sink.clientChanged();
    }

    capabilities() {
        return C.IMPLEMENTED_CAPABILITIES.filter(c => this._p.supports(c));
    }

    isClientPid(pid) {
        return !!this._client?.alive && Number.isInteger(pid) && pid > 0 && pid === this._client.pid;
    }

    _validArgs(args) {
        if (!Array.isArray(args) || !args.every(a => typeof a === 'string'))
            throw invalid('args must be strings');
    }

    /**
     * Open to any caller. A manually started Tidemark calls this *before*
     * registering its GApplication; on true it exits and the trusted instance
     * starts. On false with ClientState running/starting it registers anyway
     * and GApplication forwards the activation to the trusted primary.
     */
    requestLaunch(args) {
        this._validArgs(args);
        if (this._client?.alive || this._handover)
            return false;
        if (this._p.now() - this._lastLaunchAt < C.RELAUNCH_COOLDOWN_MS)
            return false;
        this._failures = 0;
        return this.launch(args);
    }

    /**
     * Open to any caller: an untrusted running Tidemark (ClientState blocked,
     * or Hello denied) saves its state, calls this, then exits. The adapter
     * launches the official client only once the caller's bus connection is
     * gone, so the new process becomes the GApplication primary. `whenGone`
     * is supplied by the D-Bus layer (it watches the caller's unique name).
     */
    requestHandover(args, whenGone) {
        this._validArgs(args);
        if (this._client?.alive || this._handover || !this._enabled)
            return false;
        const handover = { args, timer: null };
        handover.timer = this._p.setTimeout(C.HANDOVER_TIMEOUT_MS, () => {
            this._log('handover: the old instance did not exit in time; cancelled');
            this._handover = null;
        });
        this._handover = handover;
        whenGone(() => {
            if (this._handover !== handover)
                return;
            this._cancelHandover();
            this._failures = 0;
            this.launch(args);
        });
        return true;
    }

    _cancelHandover() {
        if (this._handover && this._handover.timer !== null)
            this._p.clearTimeout(this._handover.timer);
        this._handover = null;
    }

    launch(args) {
        if (!this._enabled || this._client?.alive)
            return false;
        const { path: exe, override } = this._p.clientPath();
        if (!exe) {
            this._log(override
                ? `${C.DEV_CLIENT_ENV} is not an absolute path to an executable; not launching`
                : `${C.CLIENT_PATH} is not installed; not launching`);
            this._setState('failed');
            return false;
        }
        if (override)
            this._log(`developer override: launching ${exe} from ${C.DEV_CLIENT_ENV}`);
        const argv = [exe, ...args.filter(a => C.LAUNCH_ARGS_ALLOWED.includes(a))];
        this._lastLaunchAt = this._p.now();
        let handle;
        try {
            handle = this._p.spawnClient(argv, status => this._onClientExit(status));
        } catch (e) {
            this._log(`launch failed: ${e.message}`);
            this._scheduleRestart();
            return false;
        }
        this._client = { pid: handle.pid, startedAt: this._p.now(), alive: true, helloed: false };
        this._windows.clear();
        this._island = null;
        this._clientChanged();
        this._setState('starting');
        return true;
    }

    _onClientExit({ code, signaled }) {
        if (!this._client?.alive)
            return;
        const { startedAt, helloed } = this._client;
        const ran = this._p.now() - startedAt;
        this._client.alive = false;
        this._windows.clear();
        this._island = null;
        this._releaseShortcuts();
        // Windows dimmed for the client must not stay dimmed after it is gone.
        this._restoreOpacity();
        this._clientChanged();
        if (!this._enabled)
            return;
        const clean = !signaled && code === 0;
        if (clean && !helloed && ran < C.EARLY_EXIT_MS) {
            this._log('client handed its activation to another Tidemark instance; waiting for RequestHandover');
            this._setState('blocked');
            return;
        }
        if (clean) {
            this._log('client quit normally; not restarting');
            this._setState('none');
            return;
        }
        if (ran >= C.STABLE_RUN_MS)
            this._failures = 0;
        this._scheduleRestart();
    }

    _scheduleRestart() {
        if (this._failures >= C.RESTART_BACKOFF_MS.length) {
            this._log('client keeps failing; giving up until RequestLaunch');
            this._setState('failed');
            return;
        }
        this._setState('none');
        const delay = C.RESTART_BACKOFF_MS[this._failures++];
        this._restartTimer = this._p.setTimeout(delay, () => {
            this._restartTimer = null;
            this.launch([]);
        });
    }

    /** Called for the authenticated client only. */
    hello() {
        if (this._client?.alive) {
            this._client.helloed = true;
            this._setState('running');
        }
        return { version: C.CONTRACT_VERSION, capabilities: this.capabilities() };
    }

    emitSnapshot() {
        const focus = this._currentFocus();
        this._focusKey = focusKey(focus);
        this._sink.emit('FocusChanged', [focus]);
        this._fullscreen.forEach((on, i) => {
            if (on)
                this._sink.emit('FullscreenChanged', [i, true]);
        });
        this._sink.emit('MaximizedTopChanged', [this._covered]);
        if (this._layout)
            this._sink.emit('LayoutChanged', [this._layout]);
        this._sink.emit('CapsLock', [this._caps]);
    }

    // ---- windows of the client ---------------------------------------------

    _owned(w) {
        return !!this._client?.alive && this._p.ownsWindow(w);
    }

    _isExternal(w) {
        return this._p.isTracked(w) && !this._owned(w);
    }

    onWindowChanged(w) {
        if (!this._owned(w))
            return;
        const id = this._p.windowId(w);
        const role = C.roleForTitle(this._p.title(w));
        const previous = this._windows.get(id)?.role;
        this._windows.set(id, { w, role });
        if ((role === 'island' || role === 'overlay') && previous !== role)
            this._p.prepareChrome(w);
        if (role === 'island') {
            if (previous !== 'island')
                this._islandMappedAt = this._p.now();
            this._island = w;
            this._applyIsland();
        } else if (this._island && this._p.windowId(this._island) === id) {
            this._island = null;
        }
    }

    onWindowGone(w) {
        const id = this._p.windowId(w);
        this._windows.delete(id);
        this._opacity.delete(id);
        if (this._island && this._p.windowId(this._island) === id)
            this._island = null;
        if (this._lastExternal && this._p.windowId(this._lastExternal) === id) {
            this._lastExternal = null;
            this.onFocusChanged();
        }
    }

    onWindowGeometry(w) {
        if (this._island && this._p.windowId(this._island) === this._p.windowId(w))
            this._applyIsland();
        else if (this._lastExternal && this._p.windowId(this._lastExternal) === this._p.windowId(w))
            this._updateCovered();
    }

    onWindowState(w) {
        if (this._lastExternal && this._p.windowId(this._lastExternal) === this._p.windowId(w)) {
            this.onFocusChanged();
            this._updateCovered();
        }
    }

    /** Mutter's minimize/map animations may reset the actor's opacity. */
    onWindowShown(w) {
        const set = this._opacity.get(this._p.windowId(w));
        if (set)
            this._p.setOpacity(w, set.value);
    }

    onMonitorsChanged() {
        this._applyIsland();
        this.onFullscreenChanged();
        this._updateCovered();
    }

    _byRole(role) {
        let found = null;
        for (const entry of this._windows.values()) {
            if (entry.role === role)
                found = entry.w; // the most recently seen window of that role
        }
        return found;
    }

    place(role, rectValue, input) {
        if (!C.ROLES.includes(role))
            throw invalid(`unknown role: ${role}`);
        const rect = C.toRect(rectValue);
        if (!rect || !C.validSize(rect))
            throw invalid('rect must be four integers with a positive size');
        if (!Array.isArray(input) || !input.every(r => C.toRect(r)))
            throw invalid('input must be a list of rects');
        // `input` is applied by the client itself (GTK input region).
        if (role === 'island') {
            this._islandRequest = { y: rect.y, w: rect.w, h: rect.h };
            this._applyIsland();
            return;
        }
        const w = this._byRole(role);
        if (!w)
            throw failed(`no ${role} window is open`);
        if (role === 'overlay') {
            this._p.moveResizeFrame(w, rect.x, rect.y, rect.w, rect.h, false);
        } else {
            this._p.moveFrame(w, rect.x, rect.y);
            this._p.activate(w);
        }
    }

    _applyIsland() {
        if (!this._island || !this._islandRequest)
            return;
        const current = this._p.describe(this._island).rect;
        const width = current.w > 0 ? current.w : this._islandRequest.w;
        const target = C.islandPosition({
            monitor: this._p.primaryMonitor(),
            panelBottom: this._p.panelBottom(),
            width,
            offsetY: this._islandRequest.y,
        });
        if (current.x !== target.x || current.y !== target.y)
            this._p.moveFrame(this._island, target.x, target.y);
    }

    // ---- focus, fullscreen, maximized ---------------------------------------

    _currentFocus() {
        return this._lastExternal ? focusInfo(this._p.describe(this._lastExternal)) : null;
    }

    onFocusChanged() {
        const f = this._p.focusWindow();
        if (f && this._island && this._p.windowId(f) === this._p.windowId(this._island)) {
            // Mapping the island must not steal focus from the user's app.
            if (this._p.now() - this._islandMappedAt < C.FOCUS_GUARD_MS && this._lastExternal)
                this._p.activate(this._lastExternal);
            return;
        }
        if (f && !this._isExternal(f))
            return; // our other windows, shell surfaces, docks
        this._lastExternal = f ?? null;
        const focus = this._currentFocus();
        const key = focusKey(focus);
        if (key !== this._focusKey) {
            this._focusKey = key;
            this._sink.emit('FocusChanged', [focus]);
        }
        this._updateCovered();
    }

    getFocus() {
        return this._currentFocus();
    }

    onFullscreenChanged() {
        const now = this._p.monitorsInFullscreen();
        const count = Math.max(now.length, this._fullscreen.length);
        for (let i = 0; i < count; i++) {
            const on = !!now[i];
            if (on !== !!this._fullscreen[i])
                this._sink.emit('FullscreenChanged', [i, on]);
        }
        this._fullscreen = now;
    }

    _computeCovered() {
        const w = this._lastExternal;
        if (!w)
            return false;
        const d = this._p.describe(w);
        if (d.minimized || d.fullscreen || d.monitor !== this._p.primaryMonitor().index)
            return false;
        return C.coversIslandStrip(d.rect, this._p.workArea(w), d.maximized);
    }

    _updateCovered() {
        const covered = this._computeCovered();
        if (covered !== this._covered) {
            this._covered = covered;
            this._sink.emit('MaximizedTopChanged', [covered]);
        }
    }

    // ---- window actions -------------------------------------------------------

    windowAction(id, action, arg) {
        if (!C.WINDOW_ACTIONS.includes(action))
            throw invalid(`unknown action: ${action}`);
        const w = this._p.windowById(id);
        if (!w)
            throw failed('Окно уже закрыто');
        if (!this._isExternal(w))
            throw failed('Это не окно приложения');
        switch (action) {
        case 'activate':
            this._p.activate(w);
            break;
        case 'minimize':
            this._p.minimize(w);
            break;
        case 'toggle_maximize':
            this._p.setMaximized(w, !this._p.describe(w).maximized);
            break;
        case 'set_above':
            if (typeof arg !== 'boolean')
                throw invalid('set_above needs a boolean');
            this._p.setAbove(w, arg);
            break;
        case 'set_opacity':
            if (!Number.isInteger(arg) || arg < C.MIN_OPACITY || arg > 255)
                throw invalid(`set_opacity needs ${C.MIN_OPACITY}..255`);
        {
            const original = this._opacity.get(id)?.original ?? this._p.describe(w).opacity;
            this._p.setOpacity(w, arg);
            if (arg === original)
                this._opacity.delete(id);
            else
                this._opacity.set(id, { w, value: arg, original });
        }
            break;
        case 'tile_left':
        case 'tile_right': {
            const d = this._p.describe(w);
            if (d.minimized)
                this._p.activate(w);
            if (d.maximized)
                this._p.setMaximized(w, false);
            const r = C.tileRect(this._p.workArea(w), action === 'tile_left' ? 'left' : 'right');
            this._p.moveResizeFrame(w, r.x, r.y, r.w, r.h, true);
            break;
        }
        }
        const d = this._p.describe(w);
        this.onWindowState(w);
        return { maximized: d.maximized, above: d.above, opacity: d.opacity, minimized: d.minimized };
    }

    // ---- shortcuts ------------------------------------------------------------

    bindShortcuts(bindings) {
        if (!Array.isArray(bindings))
            throw invalid('bindings must be a list');
        const seen = new Set();
        for (const b of bindings) {
            if (!Array.isArray(b) || b.length !== 2 || !C.SHORTCUT_IDS.includes(b[0]) || !C.validAccelerator(b[1]))
                throw invalid('each binding is (shortcut id, accelerator)');
            if (seen.has(b[0]))
                throw invalid(`duplicate shortcut id: ${b[0]}`);
            seen.add(b[0]);
        }
        this._releaseShortcuts();
        return bindings.map(([id, accel]) => {
            // grab fails (0) when Mutter or another app already owns the combo;
            // existing bindings are never overridden.
            const action = this._p.grabAccelerator(accel);
            if (action)
                this._grabs.set(action, id);
            return [id, !!action];
        });
    }

    onAccelerator(action) {
        const id = this._grabs.get(action);
        if (id)
            this._sink.emit('ShortcutActivated', [id]);
    }

    /** Puts back the opacity windows had before we changed it. */
    _restoreOpacity() {
        for (const { w, original } of this._opacity.values()) {
            try {
                this._p.setOpacity(w, original);
            } catch (e) {
                this._log(`opacity restore skipped: ${e.message}`); // window already destroyed
            }
        }
        this._opacity.clear();
    }

    _releaseShortcuts() {
        for (const action of this._grabs.keys())
            this._p.ungrabAccelerator(action);
        this._grabs.clear();
    }

    // ---- clipboard ------------------------------------------------------------

    onClipboardChanged() {
        this._clipboardGeneration++;
        const mimes = this._p.clipboardMimes();
        const sensitive = C.isSensitive(mimes);
        // Secret content: say that something was copied, but not even what kinds.
        this._sink.emit('ClipboardChanged', [sensitive ? [] : mimes, sensitive]);
    }

    async getClipboard(mime) {
        if (!C.validMime(mime))
            throw invalid('bad MIME type');
        const generation = this._clipboardGeneration;
        const mimes = this._p.clipboardMimes();
        if (C.isSensitive(mimes))
            throw failed('Содержимое помечено как секретное');
        if (!mimes.includes(mime))
            throw failed('В буфере нет данных этого типа');
        let bytes;
        try {
            bytes = await this._p.readClipboard(mime, C.MAX_CLIPBOARD_BYTES);
        } catch (e) {
            throw failed(e?.message === 'too_large' ? 'Слишком большие данные в буфере' : 'Не удалось прочитать буфер обмена');
        }
        // Any owner change during the read (even back to the same kind of
        // content) voids it: the bytes may come from a different, secret owner.
        if (generation !== this._clipboardGeneration)
            throw failed('Буфер обмена изменился во время чтения');
        if (!(bytes instanceof Uint8Array) || bytes.length > C.MAX_CLIPBOARD_BYTES)
            throw failed('Не удалось прочитать буфер обмена');
        return bytes;
    }

    setClipboard(mime, bytes) {
        if (!C.validMime(mime))
            throw invalid('bad MIME type');
        if (!(bytes instanceof Uint8Array) || bytes.length > C.MAX_CLIPBOARD_BYTES)
            throw invalid('bytes must be at most 64 MiB');
        this._p.writeClipboard(mime, bytes);
    }

    // ---- keyboard -------------------------------------------------------------

    onLayoutChanged() {
        const layout = C.normalizeLayout(this._p.currentLayout());
        if (layout && layout !== this._layout) {
            this._layout = layout;
            this._sink.emit('LayoutChanged', [layout]);
        }
    }

    onCapsLockChanged() {
        const on = !!this._p.capsLock();
        if (on !== this._caps) {
            this._caps = on;
            this._sink.emit('CapsLock', [on]);
        }
    }

    // ---- not implemented in contract v1 of this adapter -------------------------

    screenshot() {
        throw unsupported('screenshot');
    }

    setCaptureHidden() {
        throw unsupported('capture_hiding');
    }
}

/** describe() result -> FocusInfo (serde field names; null optionals omitted). */
export function focusInfo(d) {
    const info = {
        id: d.id, app_id: d.app_id, title: d.title,
        maximized: d.maximized, above: d.above, opacity: d.opacity,
        rect: d.rect, monitor: d.monitor,
    };
    if (d.pid !== null && d.pid !== undefined)
        info.pid = d.pid;
    if (d.icon)
        info.icon = d.icon;
    return info;
}

// Geometry is left out: moving a window must not flood FocusChanged.
function focusKey(info) {
    if (!info)
        return 'none';
    const { rect: _rect, ...rest } = info;
    return JSON.stringify(rest);
}
