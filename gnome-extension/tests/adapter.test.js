// Adapter behaviour against a fake GNOME platform (no GNOME Shell needed).
// Run: cd gnome-extension && npm test  (= node --test)

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { Adapter, AdapterError } from '../tidemark@treyzar.github.io/adapter.js';
import * as C from '../tidemark@treyzar.github.io/contract.js';

class FakeWindow {
    constructor(id, props = {}) {
        Object.assign(this, {
            id, title: '', pid: 1000 + id, owned: false, tracked: true, app_id: `app${id}`, icon: null,
            maximized: false, above: false, opacity: 255, rect: { x: 100, y: 200, w: 800, h: 600 },
            monitor: 0, minimized: false, fullscreen: false, chrome: false,
            workArea: { x: 0, y: 32, width: 1920, height: 1048 },
        }, props);
    }
}

class FakePlatform {
    constructor() {
        this.t = 0;
        this.windows = [];
        this.focus = null;
        this.timers = new Map();
        this.nextTimer = 1;
        this.spawned = [];
        this.program = '/usr/bin/tidemark';
        this.nextPid = 4242;
        this.exitHandlers = [];
        this.fullscreen = [false, false];
        this.grabbed = new Map();
        this.takenAccels = new Set(['<Alt>space']);
        this.nextAction = 100;
        this.mimes = [];
        this.clipboard = new Map();
        this.written = [];
        this.layout = 'us';
        this.caps = false;
        this.activated = [];
        this.moves = [];
        this.panel = 32;
        this.primary = { index: 0, x: 0, y: 0, width: 1920, height: 1080 };
    }

    now() { return this.t; }
    setTimeout(ms, fn) { const id = this.nextTimer++; this.timers.set(id, { at: this.t + ms, fn }); return id; }
    clearTimeout(id) { this.timers.delete(id); }
    advance(ms) {
        this.t += ms;
        for (const [id, { at, fn }] of [...this.timers]) {
            if (at <= this.t) {
                this.timers.delete(id);
                fn();
            }
        }
    }

    supports() { return true; }
    clientPath() { return C.resolveClientPath(this.override ?? null, path => path === this.program || path === this.devProgram); }
    spawnClient(argv, onExit) {
        const pid = this.nextPid++;
        this.spawned.push(argv);
        this.exitHandlers.push(onExit);
        return { pid };
    }
    exit(status) { this.exitHandlers.at(-1)(status); }
    adoptClient() { return this.adopt ?? null; }
    releaseClient() { this.released = true; }

    forEachWindow(fn) { this.windows.forEach(fn); }
    windowById(id) { return this.windows.find(w => w.id === id) ?? null; }
    windowId(w) { return w.id; }
    title(w) { return w.title; }
    ownsWindow(w) { return w.owned; }
    isTracked(w) { return w.tracked; }
    prepareChrome(w) { w.chrome = true; }
    describe(w) {
        const { id, app_id, title, pid, icon, maximized, above, opacity, rect, monitor, minimized, fullscreen } = w;
        return { id, app_id, title, pid, icon, maximized, above, opacity, rect: { ...rect }, monitor, minimized, fullscreen };
    }
    focusWindow() { return this.focus; }
    primaryMonitor() { return this.primary; }
    panelBottom() { return this.panel; }
    workArea(w) { return w.workArea; }
    monitorsInFullscreen() { return [...this.fullscreen]; }
    moveFrame(w, x, y) { this.moves.push([w.id, x, y]); w.rect = { ...w.rect, x, y }; }
    moveResizeFrame(w, x, y, width, height, userOp) { this.moves.push([w.id, x, y, width, height, userOp]); w.rect = { x, y, w: width, h: height }; }
    activate(w) { this.activated.push(w.id); w.minimized = false; }
    minimize(w) { w.minimized = true; }
    setMaximized(w, on) { w.maximized = on; }
    setAbove(w, on) { w.above = on; }
    setOpacity(w, v) { w.opacity = v; }
    grabAccelerator(accel) {
        if (this.takenAccels.has(accel) || [...this.grabbed.values()].includes(accel))
            return 0;
        const action = this.nextAction++;
        this.grabbed.set(action, accel);
        return action;
    }
    ungrabAccelerator(action) { this.grabbed.delete(action); }
    clipboardMimes() { return [...this.mimes]; }
    readClipboard(mime, maxBytes) {
        this.readLimits = [...(this.readLimits ?? []), maxBytes];
        this.duringRead?.();
        const data = this.clipboard.get(mime);
        if (!data)
            return Promise.reject(new Error('no data'));
        if (data.length > maxBytes)
            return Promise.reject(new Error('too_large'));
        return Promise.resolve(data);
    }
    writeClipboard(mime, bytes) { this.written.push([mime, bytes]); }
    currentLayout() { return this.layout; }
    capsLock() { return this.caps; }
}

class Sink {
    constructor() { this.events = []; this.changes = 0; this.states = []; }
    stateChanged(state) { this.states.push(state); }
    emit(name, args) { this.events.push([name, args]); }
    clientChanged() { this.changes++; }
    take() { const e = this.events; this.events = []; return e; }
}

function setup(configure) {
    const p = new FakePlatform();
    configure?.(p);
    const sink = new Sink();
    const a = new Adapter(p, sink);
    a.enable();
    return { p, sink, a };
}

function island(p, id = 1) {
    const w = new FakeWindow(id, { owned: true, tracked: false, title: C.ISLAND_TITLE, rect: { x: 0, y: 0, w: 400, h: 60 } });
    p.windows.push(w);
    return w;
}

const err = kind => e => e instanceof AdapterError && e.kind === kind;

test('primary monitor snapshot precedes per-monitor fullscreen and follows monitor changes', () => {
    const { p, a, sink } = setup(p => {
        p.primary.index = 1;
        p.fullscreen = [true, false];
    });
    assert.equal(a.primaryMonitor, 1);
    sink.take();
    a.emitSnapshot();
    const events = sink.take();
    const primaryAt = events.findIndex(([name]) => name === 'PrimaryMonitorChanged');
    const fullAt = events.findIndex(([name]) => name === 'FullscreenChanged');
    assert.deepEqual(events[primaryAt], ['PrimaryMonitorChanged', [1]]);
    assert.ok(primaryAt < fullAt);
    p.primary.index = 0;
    a.onMonitorsChanged();
    assert.equal(a.primaryMonitor, 0);
    assert.deepEqual(sink.take().filter(([name]) => name === 'PrimaryMonitorChanged'),
        [['PrimaryMonitorChanged', [0]]]);
    a.onMonitorsChanged();
    assert.ok(!sink.take().some(([name]) => name === 'PrimaryMonitorChanged'));
});

test('launches /usr/bin/tidemark once and authenticates only its pid', () => {
    const { p, a, sink } = setup();
    assert.deepEqual(p.spawned, [['/usr/bin/tidemark']]);
    assert.equal(sink.changes, 1);
    assert.equal(a.isClientPid(4242), true);
    assert.equal(a.isClientPid(4243), false);
    assert.equal(a.isClientPid(0), false);
    p.exit({ code: 0, signaled: false });
    assert.equal(a.isClientPid(4242), false, 'a dead client is not trusted');
});

test('missing executable: nothing launched, nobody authenticated', () => {
    const { p, a } = setup(p => { p.program = null; });
    assert.deepEqual(p.spawned, []);
    assert.equal(a.isClientPid(4242), false);
});

test('normal quit is not restarted; crashes back off and give up', () => {
    const { p } = setup();
    p.exit({ code: 0, signaled: false });
    p.advance(60000);
    assert.equal(p.spawned.length, 1);

    const s = setup();
    for (const delay of C.RESTART_BACKOFF_MS) {
        s.p.exit({ code: 1, signaled: false });
        s.p.advance(delay - 1);
        const before = s.p.spawned.length;
        s.p.advance(1);
        assert.equal(s.p.spawned.length, before + 1);
    }
    s.p.exit({ code: 0, signaled: true });
    s.p.advance(120000);
    assert.equal(s.p.spawned.length, 1 + C.RESTART_BACKOFF_MS.length, 'gives up after the last backoff');
});

test('a crash after a stable run restarts quickly again', () => {
    const { p } = setup();
    p.exit({ code: 1, signaled: false });
    p.advance(C.RESTART_BACKOFF_MS[0]);
    p.advance(C.STABLE_RUN_MS);
    p.exit({ code: 1, signaled: false });
    p.advance(C.RESTART_BACKOFF_MS[0]);
    assert.equal(p.spawned.length, 3);
});

test('RequestLaunch: only when not running, whitelisted args, cooldown', () => {
    const { p, a } = setup();
    assert.equal(a.requestLaunch(['--settings']), false, 'already running');
    p.exit({ code: 0, signaled: false });
    assert.equal(a.requestLaunch([]), false, 'cooldown after the first launch');
    p.advance(C.RELAUNCH_COOLDOWN_MS);
    assert.equal(a.requestLaunch(['--settings', '--evil', '-e', 'rm']), true);
    assert.deepEqual(p.spawned.at(-1), ['/usr/bin/tidemark', '--settings']);
    p.exit({ code: 0, signaled: false });
    assert.equal(a.requestLaunch([]), false, 'cooldown');
    p.advance(C.RELAUNCH_COOLDOWN_MS);
    assert.equal(a.requestLaunch([]), true);
    assert.throws(() => a.requestLaunch([1]), err('invalid_args'));
});

test('island window: chrome, centred below the top bar, not moved twice', () => {
    const { p, a } = setup();
    const w = island(p);
    a.onWindowChanged(w);
    assert.equal(w.chrome, true);
    a.place('island', [999, 6, 400, 60], [[0, 0, 400, 60]]);
    assert.deepEqual(w.rect, { x: 760, y: 38, w: 400, h: 60 });
    const moves = p.moves.length;
    a.onWindowGeometry(w);
    assert.equal(p.moves.length, moves, 'already in place');
    p.panel = 0;
    a.onMonitorsChanged();
    assert.deepEqual([w.rect.x, w.rect.y], [760, 6]);
});

test('Place(island) before the window maps is applied on map', () => {
    const { p, a } = setup();
    a.place('island', [0, 0, 300, 40], []);
    const w = island(p);
    w.title = '';
    a.onWindowChanged(w);
    assert.equal(w.chrome, false, 'untitled window is settings, not chrome');
    w.title = C.ISLAND_TITLE;
    a.onWindowChanged(w);
    assert.equal(w.chrome, true);
    assert.deepEqual([w.rect.x, w.rect.y], [760, 32]);
});

test('Place validation and other roles', () => {
    const { p, a } = setup();
    assert.throws(() => a.place('dock', [0, 0, 1, 1], []), err('invalid_args'));
    assert.throws(() => a.place('island', [0, 0, 0, 10], []), err('invalid_args'));
    assert.throws(() => a.place('island', [0, 0, 10, 10], [[1, 2]]), err('invalid_args'));
    assert.throws(() => a.place('overlay', [0, 0, 10, 10], []), err('failed'));
    const settings = new FakeWindow(5, { owned: true, title: 'Настройки' });
    p.windows.push(settings);
    a.onWindowChanged(settings);
    a.place('settings', [100, 100, 900, 700], []);
    assert.deepEqual([settings.rect.x, settings.rect.y], [100, 100]);
    assert.deepEqual(p.activated, [5]);
    const overlay = new FakeWindow(6, { owned: true, title: C.OVERLAY_TITLE });
    p.windows.push(overlay);
    a.onWindowChanged(overlay);
    a.place('overlay', [-1280, 0, 3200, 1080], []);
    assert.deepEqual(overlay.rect, { x: -1280, y: 0, w: 3200, h: 1080 });
    assert.equal(overlay.chrome, true);
});

test('windows of other processes are never treated as the client', () => {
    const { p, a } = setup();
    const foreign = new FakeWindow(9, { title: C.ISLAND_TITLE });
    p.windows.push(foreign);
    a.onWindowChanged(foreign);
    assert.equal(foreign.chrome, false);
});

test('focus: serde-shaped info, own windows ignored, deduplicated', () => {
    const { p, a, sink } = setup();
    const app = new FakeWindow(2, { title: 'Документы', icon: 'org.gnome.Nautilus' });
    p.windows.push(app);
    p.focus = app;
    a.onFocusChanged();
    const [[name, [info]]] = sink.take().filter(([n]) => n === 'FocusChanged');
    assert.equal(name, 'FocusChanged');
    assert.deepEqual(Object.keys(info), ['id', 'app_id', 'title', 'maximized', 'above', 'opacity', 'rect', 'monitor', 'pid', 'icon']);
    assert.equal(info.title, 'Документы');
    a.onFocusChanged();
    assert.equal(sink.take().filter(([n]) => n === 'FocusChanged').length, 0, 'no duplicate');
    app.rect = { ...app.rect, x: 5 };
    a.onWindowGeometry(app);
    assert.equal(sink.take().filter(([n]) => n === 'FocusChanged').length, 0, 'moving does not re-announce');
    app.title = 'Загрузки';
    a.onWindowState(app);
    assert.equal(sink.take().filter(([n]) => n === 'FocusChanged').length, 1, 'title change does');

    const settings = new FakeWindow(3, { owned: true, title: 'x' });
    p.windows.push(settings);
    a.onWindowChanged(settings);
    p.focus = settings;
    a.onFocusChanged();
    assert.equal(sink.take().length, 0, 'focus on our own window keeps the last app');
    assert.equal(a.getFocus().id, 2);

    const noPid = new FakeWindow(4, { pid: null, icon: null });
    p.focus = noPid;
    a.onFocusChanged();
    const [[, [info2]]] = sink.take().filter(([n]) => n === 'FocusChanged');
    assert.ok(!('pid' in info2) && !('icon' in info2), 'None optionals are omitted');
    p.focus = null;
    a.onFocusChanged();
    assert.deepEqual(sink.take().filter(([n]) => n === 'FocusChanged'), [['FocusChanged', [null]]]);
});

test('mapping the island does not steal focus', () => {
    const { p, a } = setup();
    const app = new FakeWindow(2);
    p.windows.push(app);
    p.focus = app;
    a.onFocusChanged();
    const w = island(p);
    a.onWindowChanged(w);
    p.focus = w;
    a.onFocusChanged();
    assert.deepEqual(p.activated, [2]);
    p.advance(C.FOCUS_GUARD_MS);
    a.onFocusChanged();
    assert.deepEqual(p.activated, [2], 'a later click may focus the island');
});

test('maximized / top-covering focused window', () => {
    const { p, a, sink } = setup();
    const app = new FakeWindow(2, { rect: { x: 0, y: 32, w: 1920, h: 500 } });
    p.windows.push(app);
    p.focus = app;
    a.onFocusChanged();
    assert.deepEqual(sink.take().filter(([n]) => n === 'MaximizedTopChanged'), [['MaximizedTopChanged', [true]]]);
    app.rect = { x: 0, y: 300, w: 1920, h: 500 };
    a.onWindowGeometry(app);
    assert.deepEqual(sink.take(), [['MaximizedTopChanged', [false]]]);
    app.maximized = true;
    a.onWindowState(app);
    assert.ok(sink.take().some(([n, args]) => n === 'MaximizedTopChanged' && args[0] === true));
    app.monitor = 1;
    a.onWindowState(app);
    assert.ok(sink.take().some(([n, args]) => n === 'MaximizedTopChanged' && args[0] === false), 'other monitor');
});

test('fullscreen per monitor', () => {
    const { p, a, sink } = setup();
    p.fullscreen = [false, true];
    a.onFullscreenChanged();
    p.fullscreen = [true, false];
    a.onFullscreenChanged();
    p.fullscreen = [true];
    a.onFullscreenChanged();
    assert.deepEqual(sink.take(), [
        ['FullscreenChanged', [1, true]],
        ['FullscreenChanged', [0, true]], ['FullscreenChanged', [1, false]],
    ]);
});

test('window actions report the resulting state and refuse bad input', () => {
    const { p, a } = setup();
    const app = new FakeWindow(2);
    const own = new FakeWindow(3, { owned: true, tracked: false });
    const shell = new FakeWindow(4, { tracked: false });
    p.windows.push(app, own, shell);
    assert.deepEqual(a.windowAction(2, 'set_above', true), { maximized: false, above: true, opacity: 255, minimized: false });
    assert.equal(a.windowAction(2, 'set_opacity', 217).opacity, 217);
    assert.equal(a.windowAction(2, 'toggle_maximize', null).maximized, true);
    assert.equal(a.windowAction(2, 'tile_right', null).maximized, false);
    assert.deepEqual(app.rect, { x: 960, y: 32, w: 960, h: 1048 });
    assert.deepEqual(p.moves.at(-1), [2, 960, 32, 960, 1048, true]);
    assert.equal(a.windowAction(2, 'minimize', null).minimized, true);
    a.windowAction(2, 'activate', null);
    assert.deepEqual(p.activated.at(-1), 2);
    assert.throws(() => a.windowAction(2, 'close', null), err('invalid_args'));
    assert.throws(() => a.windowAction(2, 'set_above', 1), err('invalid_args'));
    assert.throws(() => a.windowAction(2, 'set_opacity', 10), err('invalid_args'));
    assert.throws(() => a.windowAction(2, 'set_opacity', 300), err('invalid_args'));
    assert.throws(() => a.windowAction(99, 'activate', null), err('failed'));
    assert.throws(() => a.windowAction(3, 'minimize', null), err('failed'), 'own window');
    assert.throws(() => a.windowAction(4, 'minimize', null), err('failed'), 'shell window');
});

test('opacity set by us survives re-show and is reset on disable', () => {
    const { p, a } = setup();
    const app = new FakeWindow(2);
    p.windows.push(app);
    a.windowAction(2, 'set_opacity', 179);
    app.opacity = 255; // e.g. a minimize animation
    a.onWindowShown(app);
    assert.equal(app.opacity, 179);
    a.disable();
    assert.equal(app.opacity, 255);
    assert.equal(p.released, true);
});

test('shortcuts: validation, conflicts reported, activation, rebinding', () => {
    const { p, a, sink } = setup();
    assert.throws(() => a.bindShortcuts([['open_door', '<Super>d']]), err('invalid_args'));
    assert.throws(() => a.bindShortcuts([['launcher', '']]), err('invalid_args'));
    assert.throws(() => a.bindShortcuts([['launcher', 'a'], ['launcher', 'b']]), err('invalid_args'));
    const defaults = Object.entries(C.DEFAULT_SHORTCUTS);
    assert.deepEqual(a.bindShortcuts(defaults), [['launcher', true], ['toggle_theme', true], ['like_track', true]]);
    const [[action]] = [...p.grabbed].filter(([, accel]) => accel === '<Super><Alt>t');
    a.onAccelerator(action);
    a.onAccelerator(12345);
    assert.deepEqual(sink.take(), [['ShortcutActivated', ['toggle_theme']]]);
    assert.deepEqual(a.bindShortcuts([['launcher', '<Alt>space']]), [['launcher', false]], 'taken combo is not stolen');
    assert.equal(p.grabbed.size, 0, 'previous grabs released');
    p.exit({ code: 1, signaled: false });
    assert.equal(p.grabbed.size, 0);
});

test('clipboard: secrets are neither announced nor readable', async () => {
    const { p, a, sink } = setup();
    p.mimes = ['text/plain;charset=utf-8', 'image/png'];
    p.clipboard.set('image/png', new Uint8Array([137, 80, 78, 71]));
    a.onClipboardChanged();
    assert.deepEqual(sink.take(), [['ClipboardChanged', [['text/plain;charset=utf-8', 'image/png'], false]]]);
    assert.deepEqual(await a.getClipboard('image/png'), new Uint8Array([137, 80, 78, 71]));
    await assert.rejects(a.getClipboard('text/html'), err('failed'));
    await assert.rejects(a.getClipboard('nonsense'), err('invalid_args'));
    p.mimes = ['text/plain', 'x-kde-passwordManagerHint'];
    p.clipboard.set('text/plain', new Uint8Array([1]));
    a.onClipboardChanged();
    assert.deepEqual(sink.take(), [['ClipboardChanged', [[], true]]]);
    await assert.rejects(a.getClipboard('text/plain'), err('failed'));
    a.setClipboard('text/plain;charset=utf-8', new TextEncoder().encode('привет'));
    assert.equal(p.written.length, 1);
    assert.throws(() => a.setClipboard('text plain', new Uint8Array()), err('invalid_args'));
    assert.throws(() => a.setClipboard('text/plain', 'not bytes'), err('invalid_args'));
});

test('keyboard layout and Caps Lock are announced on change only', () => {
    const { p, a, sink } = setup();
    a.onLayoutChanged();
    p.layout = 'ru';
    a.onLayoutChanged();
    a.onLayoutChanged();
    p.caps = true;
    a.onCapsLockChanged();
    a.onCapsLockChanged();
    assert.deepEqual(sink.take(), [['LayoutChanged', ['RU']], ['CapsLock', [true]]]);
});

test('unimplemented capabilities answer Unsupported with the wire name', () => {
    const { a } = setup();
    assert.throws(() => a.screenshot(), e => e.kind === 'unsupported' && e.message === C.METHOD_CAPABILITY.Screenshot);
    assert.throws(() => a.setCaptureHidden(), e => e.kind === 'unsupported' && e.message === C.METHOD_CAPABILITY.SetCaptureHidden);
    assert.deepEqual(a.capabilities(), C.IMPLEMENTED_CAPABILITIES);
});

test('Hello snapshot carries the current state', () => {
    const { p, a, sink } = setup(p => {
        p.fullscreen = [false, true];
        p.layout = 'ru';
        p.caps = true;
    });
    assert.deepEqual(a.hello(), { version: 1, capabilities: C.IMPLEMENTED_CAPABILITIES });
    a.emitSnapshot();
    assert.deepEqual(sink.take(), [
        ['FocusChanged', [null]], ['PrimaryMonitorChanged', [0]],
        ['FullscreenChanged', [1, true]], ['MaximizedTopChanged', [false]],
        ['LayoutChanged', ['RU']], ['CapsLock', [true]],
    ]);
    void p;
});

test('re-enable adopts the running client instead of launching another', () => {
    const { p, a } = setup();
    a.disable();
    p.adopt = { pid: 4242 };
    a.enable();
    assert.equal(p.spawned.length, 1);
    assert.equal(a.isClientPid(4242), true);
});

test('client is launched by fixed path; developer override must be absolute and executable', () => {
    const { p } = setup(p => { p.devProgram = '/home/dev/tidemark/target/debug/tidemark'; p.override = p.devProgram; });
    assert.deepEqual(p.spawned, [['/home/dev/tidemark/target/debug/tidemark']]);
    for (const bad of ['tidemark', 'target/debug/tidemark', '/nonexistent/tidemark']) {
        const s = setup(p => { p.override = bad; });
        assert.deepEqual(s.p.spawned, [], bad);
        assert.equal(s.a.state, 'failed');
    }
    assert.deepEqual(C.resolveClientPath(null, () => true), { path: '/usr/bin/tidemark', override: false });
    assert.deepEqual(C.resolveClientPath(null, () => false), { path: null, override: false });
});

test('generation changes with every client change', () => {
    const { p, a } = setup();
    const g1 = a.generation;
    p.exit({ code: 1, signaled: false });
    assert.ok(a.generation > g1);
    const g2 = a.generation;
    p.advance(C.RESTART_BACKOFF_MS[0]);
    assert.ok(a.generation > g2);
    a.disable();
    assert.equal(a.isClientPid(4243), false);
});

test('an untrusted instance owning the app id blocks; handover after it exits', () => {
    const { p, a, sink } = setup();
    // The child finds the existing GApplication primary, forwards, exits 0.
    p.advance(300);
    p.exit({ code: 0, signaled: false });
    assert.equal(a.state, 'blocked');
    assert.equal(a.isClientPid(4242), false);
    p.advance(10 * 60000);
    assert.equal(p.spawned.length, 1, 'never restarted into the same wall');

    // The old instance saves its state, asks for a handover, then quits.
    let gone = null;
    assert.equal(a.requestHandover(['--settings'], fn => { gone = fn; }), true);
    assert.equal(a.requestHandover([], () => {}), false, 'one handover at a time');
    assert.equal(a.requestLaunch([]), false, 'no launch while the old instance is still there');
    assert.equal(p.spawned.length, 1);
    gone();
    assert.deepEqual(p.spawned.at(-1), ['/usr/bin/tidemark', '--settings']);
    assert.equal(a.state, 'starting');
    assert.equal(a.isClientPid(4243), true);
    a.hello();
    assert.equal(a.state, 'running');
    assert.equal(a.requestHandover([], () => {}), false, 'trusted client running');
    assert.deepEqual(sink.states, ['starting', 'blocked', 'starting', 'running']);
});

test('handover is cancelled when the old instance does not exit', () => {
    const { p, a } = setup();
    p.exit({ code: 0, signaled: false });
    let gone = null;
    assert.equal(a.requestHandover([], fn => { gone = fn; }), true);
    p.advance(C.HANDOVER_TIMEOUT_MS);
    gone();
    assert.equal(p.spawned.length, 1, 'late exit after the timeout launches nothing');
    p.advance(C.RELAUNCH_COOLDOWN_MS);
    assert.equal(a.requestHandover([], fn => fn()), true, 'a new request works');
    assert.equal(p.spawned.length, 2);
});

test('early crash is a crash, a clean exit after Hello is a quit', () => {
    const s1 = setup();
    s1.p.advance(100);
    s1.p.exit({ code: 1, signaled: false });
    assert.equal(s1.a.state, 'none');
    s1.p.advance(C.RESTART_BACKOFF_MS[0]);
    assert.equal(s1.p.spawned.length, 2, 'restarted');

    const s2 = setup();
    s2.a.hello();
    s2.p.advance(100);
    s2.p.exit({ code: 0, signaled: false });
    assert.equal(s2.a.state, 'none', 'user quit, not blocked');

    const s3 = setup();
    s3.p.advance(C.EARLY_EXIT_MS);
    s3.p.exit({ code: 0, signaled: false });
    assert.equal(s3.a.state, 'none', 'slow clean exit without Hello is a quit');

    const s4 = setup();
    for (const delay of C.RESTART_BACKOFF_MS) {
        s4.p.exit({ code: 2, signaled: false });
        s4.p.advance(delay);
    }
    s4.p.exit({ code: 2, signaled: false });
    assert.equal(s4.a.state, 'failed');
    s4.p.advance(C.RELAUNCH_COOLDOWN_MS);
    assert.equal(s4.a.requestLaunch([]), true, 'RequestLaunch recovers from failed');
});

test('re-adopted client must say Hello again', () => {
    const { p, a } = setup();
    a.hello();
    a.disable();
    p.adopt = { pid: 4242 };
    a.enable();
    assert.equal(a.state, 'starting');
    a.hello();
    assert.equal(a.state, 'running');
});

test('clipboard reads are bounded and voided by any owner change during the read', async () => {
    const { p, a } = setup();
    p.mimes = ['text/plain'];
    p.clipboard.set('text/plain', new Uint8Array([104, 105]));
    a.onClipboardChanged();
    assert.deepEqual(await a.getClipboard('text/plain'), new Uint8Array([104, 105]));
    assert.deepEqual(p.readLimits, [C.MAX_CLIPBOARD_BYTES], 'the platform is told the bound');

    // A -> B (secret) -> A while reading: mimes look the same before and after.
    p.duringRead = () => {
        a.onClipboardChanged();
        a.onClipboardChanged();
    };
    await assert.rejects(a.getClipboard('text/plain'), err('failed'));
    p.duringRead = null;

    p.clipboard.set('text/plain', { length: C.MAX_CLIPBOARD_BYTES + 1 });
    await assert.rejects(a.getClipboard('text/plain'), e => e.kind === 'failed' && /большие/.test(e.message));
    p.clipboard.delete('text/plain');
    await assert.rejects(a.getClipboard('text/plain'), err('failed'));
});

test('windows dimmed for the client get their original opacity back when it exits or crashes', () => {
    const { p, a } = setup();
    const app = new FakeWindow(2, { opacity: 230 });
    const doomed = new FakeWindow(3);
    p.windows.push(app, doomed);
    a.windowAction(2, 'set_opacity', 217);
    a.windowAction(2, 'set_opacity', 179);
    a.windowAction(3, 'set_opacity', 179);
    // A destroyed window's actor is gone: setting its opacity throws.
    const setOpacity = p.setOpacity.bind(p);
    p.setOpacity = (w, v) => {
        if (w === doomed)
            throw new Error('actor destroyed');
        setOpacity(w, v);
    };
    p.exit({ code: 1, signaled: true });
    assert.equal(app.opacity, 230, 'original, not 255');
    app.opacity = 120;
    a.onWindowShown(app);
    assert.equal(app.opacity, 120, 'no longer managed after the client is gone');
});

test('setting a window back to its original opacity forgets it', () => {
    const { p, a } = setup();
    const app = new FakeWindow(2);
    p.windows.push(app);
    a.windowAction(2, 'set_opacity', 179);
    a.windowAction(2, 'set_opacity', 255);
    app.opacity = 100;
    a.onWindowShown(app);
    assert.equal(app.opacity, 100, 'no longer managed');
});
