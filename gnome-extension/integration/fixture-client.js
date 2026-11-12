#!/usr/bin/gjs -m
// Stand-in for the Tidemark client in the headless GNOME integration harness.
//
// The adapter launches this file itself (TIDEMARK_ADAPTER_CLIENT developer
// override, absolute path), so every privileged call below is authenticated
// by this process's real PID, exactly like the production client. Nothing in
// the adapter is relaxed for tests.
//
// Independent oracle: org.gnome.Shell.Eval (only available because the
// harness starts its private gnome-shell with --unsafe-mode) reads Mutter's
// own view of windows, so placement/actions are not checked by trusting the
// adapter's replies.
//
// Phase 1 runs the checks, dims the victim window, then exits with status 3
// (a crash for the adapter). The adapter restarts this file: phase 2 checks
// that the new PID is authenticated and that the victim's opacity was put
// back, then writes `done` and idles until the harness stops the session.
//
// Output: $TIDEMARK_HARNESS_DIR/results-phase<N>.txt, one line per check:
//   PASS|FAIL|SKIP [optional] <name> <detail>

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Gtk from 'gi://Gtk?version=4.0';
import System from 'system';

import * as C from './contract.js';

Gio._promisify(Gio.DBusConnection.prototype, 'call');
// Fail visibly instead of being terminated if the bus drops this connection.
Gio.DBus.session.set_exit_on_close(false);
Gio.DBus.session.connect('closed', () => record('FAIL', false, 'session bus connection closed', ''));

const OUT = GLib.getenv('TIDEMARK_HARNESS_DIR');
const VICTIM = GLib.getenv('TIDEMARK_VICTIM');
const VICTIM_DISPLAY = GLib.getenv('TIDEMARK_TEST_WAYLAND_DISPLAY');
const PHASE1_STATE = `${OUT}/phase1.json`;
const phase = GLib.file_test(PHASE1_STATE, GLib.FileTest.EXISTS) ? 2 : 1;
const bus = Gio.DBus.session;
const lines = [];
const loop = new GLib.MainLoop(null, false);
// Set before loop.quit(); the process exits from the top level after
// loop.run() returns (System.exit inside an async job did not end gjs in CI).
let exitCode = null;

class Skip extends Error {}

function record(status, optional, name, detail) {
    const line = `${status}${optional ? ' [optional]' : ''} ${name}${detail ? ` ${detail}` : ''}`.replace(/\n/g, ' ');
    lines.push(line);
    print(`[fixture phase ${phase}] ${line}`);
    GLib.file_set_contents(`${OUT}/results-phase${phase}.txt`, `${lines.join('\n')}\n`);
}

async function check(name, fn, { optional = false } = {}) {
    try {
        const detail = await fn();
        record('PASS', optional, name, detail ?? '');
        return true;
    } catch (e) {
        record(e instanceof Skip ? 'SKIP' : 'FAIL', optional, name, e.message);
        return false;
    }
}

function assert(cond, message) {
    if (!cond)
        throw new Error(message);
}

function sleep(ms) {
    return new Promise(resolve => GLib.timeout_add(GLib.PRIORITY_DEFAULT, ms, () => {
        resolve();
        return GLib.SOURCE_REMOVE;
    }));
}

// ---- D-Bus helpers -------------------------------------------------------------

async function call(method, signature = null, args = [], reply = null) {
    const r = await bus.call(C.BUS_NAME, C.OBJECT_PATH, C.INTERFACE, method,
        signature ? new GLib.Variant(signature, args) : null, reply ? new GLib.VariantType(reply) : null,
        Gio.DBusCallFlags.NONE, 15000, null);
    return r.recursiveUnpack();
}

/** {name, message} of the D-Bus error a call fails with, or null on success. */
async function callError(method, signature, args, reply) {
    try {
        await call(method, signature, args, reply);
        return null;
    } catch (e) {
        const name = Gio.DBusError.get_remote_error(e);
        Gio.DBusError.strip_remote_error(e);
        return { name, message: e.message };
    }
}

async function prop(name) {
    const r = await bus.call(C.BUS_NAME, C.OBJECT_PATH, 'org.freedesktop.DBus.Properties', 'Get',
        new GLib.Variant('(ss)', [C.INTERFACE, name]), new GLib.VariantType('(v)'), Gio.DBusCallFlags.NONE, 5000, null);
    return r.recursiveUnpack()[0];
}

async function shellEval(code) {
    const r = await bus.call('org.gnome.Shell', '/org/gnome/Shell', 'org.gnome.Shell', 'Eval',
        new GLib.Variant('(s)', [code]), new GLib.VariantType('(bs)'), Gio.DBusCallFlags.NONE, 15000, null);
    const [ok, json] = r.deepUnpack();
    if (!ok)
        throw new Error(`Shell Eval failed (is --unsafe-mode set?): ${json}`);
    return json ? JSON.parse(json) : null;
}

const signals = [];
const waiters = new Set();
bus.signal_subscribe(null, C.INTERFACE, null, C.OBJECT_PATH, null, Gio.DBusSignalFlags.NONE,
    (_conn, _sender, _path, _iface, name, params) => {
        const ev = { name, args: params.recursiveUnpack() };
        signals.push(ev);
        for (const w of [...waiters])
            w(ev);
    });

/** Resolves with the args of the first matching signal at index >= `from`. */
function waitSignal(name, pred = () => true, ms = 5000, from = 0) {
    const found = signals.slice(from).find(e => e.name === name && pred(e.args));
    if (found)
        return Promise.resolve(found.args);
    return new Promise((resolve, reject) => {
        let timer = 0;
        const w = ev => {
            if (ev.name !== name || !pred(ev.args))
                return;
            waiters.delete(w);
            GLib.source_remove(timer);
            resolve(ev.args);
        };
        timer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, ms, () => {
            waiters.delete(w);
            reject(new Error(`no ${name} within ${ms} ms`));
            return GLib.SOURCE_REMOVE;
        });
        waiters.add(w);
    });
}

// ---- Mutter oracle (via Eval) -------------------------------------------------------

const describeByTitle = title => shellEval(`(() => {
    const w = global.get_window_actors().map(a => a.meta_window).find(w => w.get_title() === ${JSON.stringify(title)});
    if (!w) return null;
    const r = w.get_frame_rect(); const wa = w.get_work_area_current_monitor();
    const mon = global.display.get_monitor_geometry(global.display.get_primary_monitor());
    const box = Main.layoutManager.panelBox;
    return { id: w.get_id(), x: r.x, y: r.y, w: r.width, h: r.height,
        above: w.is_above(), sticky: w.is_on_all_workspaces(), skipTaskbar: w.is_skip_taskbar(),
        minimized: w.minimized, opacity: w.get_compositor_private()?.opacity ?? null,
        maximized: typeof w.is_maximized === 'function' ? w.is_maximized() : w.get_maximized() === Meta.MaximizeFlags.BOTH,
        workArea: { x: wa.x, y: wa.y, width: wa.width, height: wa.height },
        monitor: { x: mon.x, y: mon.y, width: mon.width, height: mon.height },
        panelBottom: box.visible ? box.y + box.height : mon.y,
        focused: global.display.focus_window === w };
})()`);

const opacityById = id => shellEval(`(() => {
    const a = global.get_window_actors().find(a => a.meta_window.get_id() === ${Number(id)});
    return a ? a.opacity : null;
})()`);

/** Synthetic key presses through a Clutter virtual keyboard inside gnome-shell. */
const pressKeys = keys => shellEval(`(async () => {
    const { default: Clutter } = await import('gi://Clutter');
    globalThis.__tidemarkKeyboard ??= Clutter.get_default_backend().get_default_seat()
        .create_virtual_device(Clutter.InputDeviceType.KEYBOARD_DEVICE);
    const dev = globalThis.__tidemarkKeyboard;
    const keys = ${JSON.stringify(keys)}.map(k => Clutter['KEY_' + k]);
    for (const k of keys) dev.notify_keyval(GLib.get_monotonic_time(), k, Clutter.KeyState.PRESSED);
    for (const k of [...keys].reverse()) dev.notify_keyval(GLib.get_monotonic_time(), k, Clutter.KeyState.RELEASED);
    return true;
})()`);

// ---- windows --------------------------------------------------------------------

Gtk.init();

function openWindow(title, width, height, decorated) {
    const win = new Gtk.Window({ title, default_width: width, default_height: height, decorated, resizable: decorated });
    win.set_child(new Gtk.Label({ label: title }));
    win.present();
    return win;
}

/**
 * Gives the victim keyboard focus the way a user click would, once Mutter has
 * mapped and stacked it (activating earlier hits Mutter's stack_position
 * assertion on 46 and does nothing). Retries until Mutter reports the focus.
 */
async function activateVictim() {
    for (let i = 0; i < 75; i++) {
        const state = await shellEval(`(() => {
            const a = global.get_window_actors().find(a => a.meta_window.get_title() === 'tidemark-victim');
            if (!a || !a.mapped || a.meta_window.is_hidden()) return 'waiting';
            const w = a.meta_window;
            if (global.display.focus_window === w) return 'focused';
            Main.activateWindow(w, global.get_current_time());
            return 'activated';
        })()`);
        if (state === 'focused')
            return;
        await sleep(200);
    }
    throw new Error('victim never got focus in Mutter');
}

async function victimCall(method, signature, args) {
    for (let i = 0; i < 50; i++) {
        try {
            return await bus.call('org.tidemark.HarnessVictim', '/org/tidemark/HarnessVictim', 'org.tidemark.HarnessVictim',
                method, new GLib.Variant(signature, args), null, Gio.DBusCallFlags.NONE, 10000, null);
        } catch (e) {
            if (!Gio.DBusError.get_remote_error(e)?.endsWith('ServiceUnknown'))
                throw e;
            await sleep(200);
        }
    }
    throw new Error('victim never appeared on the bus');
}

function spawnVictim(extraEnv = {}) {
    const launcher = new Gio.SubprocessLauncher({ flags: Gio.SubprocessFlags.NONE });
    // Not our trusted Wayland socket: a normal, separate client.
    launcher.unsetenv('WAYLAND_SOCKET');
    launcher.unsetenv(C.ENV_MARKER);
    launcher.setenv('WAYLAND_DISPLAY', VICTIM_DISPLAY, true);
    for (const [k, v] of Object.entries(extraEnv))
        launcher.setenv(k, v, true);
    return launcher.spawnv(['/usr/bin/gjs', '-m', VICTIM]);
}

// ---- phase 1 ------------------------------------------------------------------------

async function phase1() {
    const ownPid = new Gio.Credentials().get_unix_pid();

    await check('open properties before Hello', async () => {
        assert(await prop('Version') === C.CONTRACT_VERSION, 'Version');
        assert(await prop('PrimaryMonitor') === 0, 'primary monitor of the private single-monitor session');
        const caps = await prop('Capabilities');
        assert(JSON.stringify(caps) === JSON.stringify(C.IMPLEMENTED_CAPABILITIES), `Capabilities ${caps}`);
        const state = await prop('ClientState');
        assert(state === 'starting', `ClientState ${state}`);
    });

    await check('oracle available (private gnome-shell --unsafe-mode)', async () => {
        await shellEval('Main.overview.hide(); true');
    });

    const helloAt = signals.length;
    await check('Hello authenticates the launched PID and sends a snapshot', async () => {
        const [version, caps] = await call('Hello', null, [], '(uas)');
        assert(version === 1 && caps.length === C.IMPLEMENTED_CAPABILITIES.length, `${version} ${caps}`);
        assert(await prop('ClientState') === 'running', 'ClientState not running');
        const [focus] = await waitSignal('FocusChanged', () => true, 3000, helloAt);
        assert(Object.keys(focus).length === 0, `expected empty a{sv} (None), got ${JSON.stringify(focus)}`);
        const [caps2] = await waitSignal('CapsLock', () => true, 3000, helloAt);
        const [primary] = await waitSignal('PrimaryMonitorChanged', () => true, 3000, helloAt);
        assert(primary === 0, 'PrimaryMonitor snapshot');
        assert(caps2 === false, 'CapsLock snapshot');
        await waitSignal('MaximizedTopChanged', a => a[0] === false, 3000, helloAt);
        return `pid ${ownPid}`;
    });

    openWindow(C.ISLAND_TITLE, 400, 60, false);
    await sleep(1500);
    await check('island role: chrome flags and top-centre placement below the panel', async () => {
        await call('Place', '(s(iiii)a(iiii))', ['island', [0, 6, 400, 60], [[0, 0, 400, 60]]]);
        await sleep(700);
        const d = await describeByTitle(C.ISLAND_TITLE);
        assert(d, 'island window not found in Mutter');
        const x = d.monitor.x + Math.floor((d.monitor.width - d.w) / 2);
        const y = Math.max(d.monitor.y, d.panelBottom) + 6;
        assert(d.x === x && d.y === y, `frame ${d.x},${d.y} expected ${x},${y}`);
        assert(d.above && d.sticky && d.skipTaskbar, `above ${d.above} sticky ${d.sticky} skipTaskbar ${d.skipTaskbar}`);
        return `${d.x},${d.y} ${d.w}x${d.h}`;
    });

    await check('Place rejects an unknown role', async () => {
        const e = await callError('Place', '(s(iiii)a(iiii))', ['dock', [0, 0, 10, 10], []]);
        assert(e?.name === `${C.ERROR_PREFIX}.InvalidArgs`, JSON.stringify(e));
    });

    // The island (shown first) keeps keyboard focus: a new window does not
    // steal it without user input. Simulate the user clicking the victim, via
    // the oracle, then expect the adapter to report it.
    const victimAt = signals.length;
    const victim = spawnVictim();
    const victimPid = Number(victim.get_identifier());
    let victimId = null;
    await check('FocusChanged describes another app window (serde keys, real pid)', async () => {
        await activateVictim();
        const [focus] = await waitSignal('FocusChanged', a => a[0]?.title === 'tidemark-victim', 10000, victimAt);
        for (const k of ['id', 'app_id', 'title', 'maximized', 'above', 'opacity', 'rect', 'monitor'])
            assert(k in focus, `missing ${k}`);
        assert(focus.pid === victimPid, `pid ${focus.pid} != ${victimPid}`);
        assert(Array.isArray(focus.rect) && focus.rect.length === 4, 'rect is (iiii)');
        victimId = focus.id;
        const [again] = await call('GetFocus', null, [], '(a{sv})');
        assert(again.id === victimId, 'GetFocus id');
        return `id ${victimId}`;
    });

    // Another app (the focused victim) owns the selection from here on.
    await check('password-manager copy from another app is announced empty and unreadable', async () => {
        const from = signals.length;
        await victimCall('OfferBytes', '(sub)', ['text/plain', 7, true]);
        const [mimes, sensitive] = await waitSignal('ClipboardChanged', a => a[1] === true, 5000, from);
        assert(mimes.length === 0, `mimes leaked: ${mimes}`);
        const e = await callError('GetClipboard', '(s)', ['text/plain'], '(ay)');
        assert(e?.name === `${C.ERROR_PREFIX}.Failed`, JSON.stringify(e));
        const bounded = await callError('GetClipboardBounded', '(su)', ['text/plain', 1024], '(ay)');
        assert(bounded?.name === `${C.ERROR_PREFIX}.Failed`, `bounded: ${JSON.stringify(bounded)}`);
        return `sensitive ${sensitive}`;
    });

    await check('oversized clipboard from another app is refused by the bounded read', async () => {
        const from = signals.length;
        await victimCall('OfferBytes', '(sub)', ['text/plain', C.MAX_CLIPBOARD_BYTES + 1, false]);
        const [mimes, sensitive] = await waitSignal('ClipboardChanged', a => a[0].includes('text/plain'), 10000, from);
        assert(sensitive === false, `sensitive ${sensitive} ${mimes}`);
        const e = await callError('GetClipboard', '(s)', ['text/plain'], '(ay)');
        assert(e?.name === `${C.ERROR_PREFIX}.Failed`, JSON.stringify(e));
        return e.message;
    });

    await check('clipboard of normal size from another app is read', async () => {
        const from = signals.length;
        await victimCall('OfferBytes', '(sub)', ['text/plain', 1024 * 1024, false]);
        await waitSignal('ClipboardChanged', a => a[0].includes('text/plain') && a[1] === false, 5000, from);
        const [bytes] = await call('GetClipboard', '(s)', ['text/plain'], '(ay)');
        assert(bytes.length === 1024 * 1024 && bytes[0] === 97, `got ${bytes.length} bytes`);
    });

    await check('GetClipboardBounded: caller limit refuses larger content, keeps smaller, rejects bad limits', async () => {
        const from = signals.length;
        await victimCall('OfferBytes', '(sub)', ['text/plain', 4096, false]);
        await waitSignal('ClipboardChanged', a => a[0].includes('text/plain') && a[1] === false, 5000, from);
        const small = await callError('GetClipboardBounded', '(su)', ['text/plain', 1024], '(ay)');
        assert(small?.name === `${C.ERROR_PREFIX}.Failed`, `limit 1024: ${JSON.stringify(small)}`);
        const [exact] = await call('GetClipboardBounded', '(su)', ['text/plain', 4096], '(ay)');
        assert(exact.length === 4096 && exact[0] === 97, `limit 4096 got ${exact.length}`);
        const [roomy] = await call('GetClipboardBounded', '(su)', ['text/plain', 65536], '(ay)');
        assert(roomy.length === 4096, `limit 65536 got ${roomy.length}`);
        for (const bad of [0, C.MAX_CLIPBOARD_BYTES + 1]) {
            const e = await callError('GetClipboardBounded', '(su)', ['text/plain', bad], '(ay)');
            assert(e?.name === `${C.ERROR_PREFIX}.InvalidArgs`, `limit ${bad}: ${JSON.stringify(e)}`);
        }
        return small.message;
    });

    if (victimId !== null) {
        const act = (action, arg) => call('WindowAction', '(tsv)', [victimId, action, arg], '(a{sv})').then(([s]) => s);
        await check('set_above', async () => {
            const s = await act('set_above', new GLib.Variant('b', true));
            const d = await describeByTitle('tidemark-victim');
            assert(s.above === true && d.above === true, `${s.above} / mutter ${d.above}`);
        });
        await check('set_opacity', async () => {
            const s = await act('set_opacity', new GLib.Variant('y', 179));
            const d = await describeByTitle('tidemark-victim');
            assert(s.opacity === 179 && d.opacity === 179, `${s.opacity} / mutter ${d.opacity}`);
        });
        await check('set_opacity below the floor is InvalidArgs', async () => {
            const e = await callError('WindowAction', '(tsv)', [victimId, 'set_opacity', new GLib.Variant('y', 10)], '(a{sv})');
            assert(e?.name === `${C.ERROR_PREFIX}.InvalidArgs`, JSON.stringify(e));
        });
        await check('tile_left gives the left half of the work area and covers the island strip', async () => {
            const from = signals.length;
            await act('tile_left', new GLib.Variant('b', false));
            await sleep(800);
            const d = await describeByTitle('tidemark-victim');
            const wa = d.workArea;
            const ok = Math.abs(d.x - wa.x) <= 1 && Math.abs(d.y - wa.y) <= 1 &&
                Math.abs(d.w - Math.floor(wa.width / 2)) <= 2 && Math.abs(d.h - wa.height) <= 2;
            assert(ok, `frame ${d.x},${d.y} ${d.w}x${d.h}, work area ${JSON.stringify(wa)}`);
            // The left half reaches the top of the work area across its centre.
            await waitSignal('MaximizedTopChanged', a => a[0] === true, 3000, from);
        });
        await check('toggle_maximize is reported by Mutter both ways', async () => {
            const s = await act('toggle_maximize', new GLib.Variant('b', false));
            await sleep(500);
            assert(s.maximized === true && (await describeByTitle('tidemark-victim')).maximized === true, 'maximized');
            const s2 = await act('toggle_maximize', new GLib.Variant('b', false));
            await sleep(500);
            assert(s2.maximized === false && (await describeByTitle('tidemark-victim')).maximized === false, 'restored');
        });
        await check('minimize uncovers the strip, activate covers it again', async () => {
            let from = signals.length;
            const s = await act('minimize', new GLib.Variant('b', false));
            assert(s.minimized === true, 'minimized');
            await waitSignal('MaximizedTopChanged', a => a[0] === false, 3000, from);
            from = signals.length;
            await act('activate', new GLib.Variant('b', false));
            await sleep(500);
            assert((await describeByTitle('tidemark-victim')).minimized === false, 'still minimized');
            await waitSignal('MaximizedTopChanged', a => a[0] === true, 3000, from);
        });
        await check('actions on Tidemark\'s own island are refused', async () => {
            const island = await describeByTitle(C.ISLAND_TITLE);
            const e = await callError('WindowAction', '(tsv)', [island.id, 'minimize', new GLib.Variant('b', false)], '(a{sv})');
            assert(e?.name === `${C.ERROR_PREFIX}.Failed`, JSON.stringify(e));
        });
    }

    await check('clipboard round trip through Meta.Selection', async () => {
        const text = new TextEncoder().encode('Tidemark ✓ приливы');
        const from = signals.length;
        await call('SetClipboard', '(say)', ['text/plain;charset=utf-8', text]);
        await waitSignal('ClipboardChanged', a => a[1] === false && a[0].includes('text/plain;charset=utf-8'), 3000, from);
        const [back] = await call('GetClipboard', '(s)', ['text/plain;charset=utf-8'], '(ay)');
        assert(new TextDecoder().decode(back) === 'Tidemark ✓ приливы', 'content differs');
    });

    // SetClipboard over 64 MiB is not exercised here: such a message exceeds
    // the bus limits and the bus would close the caller's connection before
    // the adapter sees it. The adapter's own bound is covered by the mock tests.

    await check('Screenshot is Unsupported with the capability name', async () => {
        const e = await callError('Screenshot', '(bb(iiii))', [false, false, [0, 0, 0, 0]], '(ay)');
        assert(e?.name === `${C.ERROR_PREFIX}.Unsupported` && e.message === 'screenshot', JSON.stringify(e));
    });

    let bound = false;
    await check('BindShortcuts: defaults grabbed, Alt+Space reported as taken', async () => {
        const [taken] = await call('BindShortcuts', '(a(ss))', [[['launcher', '<Alt>space']]], '(a(sb))');
        assert(JSON.stringify(taken) === JSON.stringify([['launcher', false]]), JSON.stringify(taken));
        const [res] = await call('BindShortcuts', '(a(ss))', [Object.entries(C.DEFAULT_SHORTCUTS)], '(a(sb))');
        assert(res.every(([, ok]) => ok), JSON.stringify(res));
        bound = true;
    });

    await check('Super+Alt+Space reaches the client', async () => {
        if (!bound)
            throw new Skip('shortcuts not bound');
        const from = signals.length;
        await pressKeys(['Super_L', 'Alt_L', 'space']);
        await waitSignal('ShortcutActivated', a => a[0] === 'launcher', 3000, from);
    }, { optional: true });

    await check('Caps Lock is reported', async () => {
        const from = signals.length;
        await pressKeys(['Caps_Lock']);
        await waitSignal('CapsLock', a => a[0] === true, 3000, from);
        await pressKeys(['Caps_Lock']);
        await waitSignal('CapsLock', a => a[0] === false, 3000, from);
    }, { optional: true });

    await check('input source switch is reported as LayoutChanged', async () => {
        const from = signals.length;
        await pressKeys(['Super_L', 'space']);
        const [layout] = await waitSignal('LayoutChanged', () => true, 3000, from);
        return layout;
    }, { optional: true });

    // Leave the victim dimmed and crash: the adapter must restore its opacity.
    if (victimId !== null) {
        await call('WindowAction', '(tsv)', [victimId, 'set_opacity', new GLib.Variant('y', 179)], '(a{sv})');
        GLib.file_set_contents(PHASE1_STATE, JSON.stringify({ victimId, pid: ownPid }));
    }
    record('INFO', false, 'phase 1 exiting with status 3 to simulate a crash', '');
    exitCode = 3;
    loop.quit();
}

// ---- phase 2 ------------------------------------------------------------------------

async function phase2() {
    const [, bytes] = GLib.file_get_contents(PHASE1_STATE);
    const { victimId, pid: oldPid } = JSON.parse(new TextDecoder().decode(bytes));
    const ownPid = new Gio.Credentials().get_unix_pid();

    await check('restarted client (new PID) is authenticated after a crash', async () => {
        assert(ownPid !== oldPid, 'same pid');
        assert(await prop('ClientState') === 'starting', 'ClientState should be starting before Hello');
        const [version] = await call('Hello', null, [], '(uas)');
        assert(version === 1, 'version');
        assert(await prop('ClientState') === 'running', 'not running');
        return `old ${oldPid} new ${ownPid}`;
    });

    await check('victim opacity restored after the client crashed', async () => {
        const opacity = await opacityById(victimId);
        assert(opacity === 255, `opacity ${opacity}`);
    });

    GLib.file_set_contents(`${OUT}/done`, 'done\n');
}

(phase === 1 ? phase1() : phase2()).catch(e => {
    record('FAIL', false, `fixture phase ${phase} aborted`, `${e.message} ${e.stack ?? ''}`);
    GLib.file_set_contents(`${OUT}/done`, 'aborted\n');
});
loop.run();
if (exitCode !== null)
    System.exit(exitCode);
