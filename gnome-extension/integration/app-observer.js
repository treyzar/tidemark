#!/usr/bin/gjs -m
// Observes the installed Rust application in the harness's private GNOME.
// Shell.Eval exists only in this isolated --unsafe-mode test session.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

Gio._promisify(Gio.DBusConnection.prototype, 'call', 'call_finish');

const bus = Gio.DBus.session;
const out = GLib.getenv('TIDEMARK_HARNESS_DIR');
const loop = new GLib.MainLoop(null, false);
const results = [];
let failed = false;
const sleep = ms => new Promise(resolve => GLib.timeout_add(GLib.PRIORITY_DEFAULT, ms, () => {
    resolve(); return GLib.SOURCE_REMOVE;
}));
function assert(on, detail) { if (!on) throw new Error(detail); }
async function call(dest, path, iface, method, signature, args, reply) {
    const value = await bus.call(dest, path, iface, method,
        signature ? new GLib.Variant(signature, args) : null,
        new GLib.VariantType(reply), Gio.DBusCallFlags.NONE, 5000, null);
    return value.recursiveUnpack();
}
async function prop(name) {
    return (await call('org.tidemark.Shell', '/org/tidemark/Shell',
        'org.freedesktop.DBus.Properties', 'Get', '(ss)', ['org.tidemark.Shell1', name], '(v)'))[0];
}
async function evaluate(code) {
    const [ok, json] = await call('org.gnome.Shell', '/org/gnome/Shell',
        'org.gnome.Shell', 'Eval', '(s)', [code], '(bs)');
    assert(ok, `private Shell.Eval: ${json}`);
    return json ? JSON.parse(json) : null;
}
async function waitFor(read, accept, description) {
    for (let i = 0; i < 100; i++) {
        const value = await read();
        if (accept(value)) return value;
        await sleep(100);
    }
    throw new Error(`timeout: ${description}`);
}
async function check(name, action) {
    try { await action(); results.push(`PASS ${name}`); }
    catch (error) { failed = true; results.push(`FAIL ${name}: ${error.message}`); }
    GLib.file_set_contents(`${out}/results-app.txt`, `${results.join('\n')}\n`);
}
const windowState = () => evaluate(`(() => {
    Main.overview.hide();
    const windows = global.get_window_actors().map(a => a.meta_window)
        .filter(w => w.get_title() === 'tidemark-island');
    const w = windows[0];
    if (!w) return null;
    const r = w.get_frame_rect();
    const m = global.display.get_monitor_geometry(global.display.get_primary_monitor());
    const panel = Main.layoutManager.panelBox;
    return {count: windows.length, pid: w.get_pid(), x:r.x, y:r.y, w:r.width, h:r.height,
        above:w.is_above(), sticky:w.is_on_all_workspaces(), hidden:w.is_skip_taskbar(),
        visible:!!w.get_compositor_private()?.mapped && !w.is_hidden(),
        center:m.x + Math.floor((m.width-r.width)/2),
        top:Math.max(m.y, panel.visible ? panel.y+panel.height : m.y)+8};
})()`);
// Emit through the real service so its authenticated recipient, wire payload,
// subscriptions and Rust/GTK state pump are all exercised.
const emit = (name, args) => evaluate(`(() => {
    const extension = Main.extensionManager.lookup('tidemark@treyzar.github.io');
    extension.stateObj._service.emit(${JSON.stringify(name)}, ${JSON.stringify(args)});
    return true;
})()`);

async function run() {
    await check('installed GTK application retains authenticated Hello connection', async () => {
        await waitFor(() => prop('ClientState'), s => s === 'running', 'Rust Hello');
        const recipient = await waitFor(() => evaluate(`Main.extensionManager.lookup('tidemark@treyzar.github.io').stateObj._service._recipient`),
            name => typeof name === 'string' && name.startsWith(':'), 'Hello recipient');
        const [live] = await call('org.freedesktop.DBus', '/org/freedesktop/DBus',
            'org.freedesktop.DBus', 'NameHasOwner', '(s)', [recipient], '(b)');
        assert(live, 'Hello connection has left the bus');
        const [helloPid] = await call('org.freedesktop.DBus', '/org/freedesktop/DBus',
            'org.freedesktop.DBus', 'GetConnectionUnixProcessID', '(s)', [recipient], '(u)');
        const [appPid] = await call('org.freedesktop.DBus', '/org/freedesktop/DBus',
            'org.freedesktop.DBus', 'GetConnectionUnixProcessID', '(s)', ['io.github.treyzar.Tidemark'], '(u)');
        assert(helloPid === appPid, 'Hello belongs to a different process');
    });
    await check('one real GTK island has trusted PID and compact placement', async () => {
        const state = await waitFor(windowState, s => placed(s, 180, 36), 'compact GTK window');
        const [pid] = await call('org.freedesktop.DBus', '/org/freedesktop/DBus',
            'org.freedesktop.DBus', 'GetConnectionUnixProcessID', '(s)', ['io.github.treyzar.Tidemark'], '(u)');
        assert(state.count === 1 && state.pid === pid, JSON.stringify(state));
        assert(state.above && state.sticky && state.hidden, JSON.stringify(state));
        assert(state.x === state.center && state.y === state.top, JSON.stringify(state));
    });
    await check('unicast launcher shortcut expands and repositions real GTK island', async () => {
        await emit('ShortcutActivated', ['launcher']);
        const state = await waitFor(windowState, s => placed(s, 560, 620), 'expanded GTK window');
        assert(state.x === state.center && state.y === state.top, JSON.stringify(state));
        await emit('ShortcutActivated', ['launcher']);
        await waitFor(windowState, s => placed(s, 180, 36), 'collapsed GTK window');
    });
    await check('fullscreen signal hides only on island monitor and restores real window', async () => {
        const primary = await prop('PrimaryMonitor');
        await emit('FullscreenChanged', [primary + 1, true]);
        await sleep(300);
        assert((await windowState())?.visible, 'secondary monitor incorrectly hid island');
        await emit('FullscreenChanged', [primary, true]);
        await waitFor(windowState, s => !s || !s.visible, 'island hidden');
        await emit('FullscreenChanged', [primary, false]);
        await waitFor(windowState, s => placed(s, 180, 36), 'island restored');
        await emit('FullscreenChanged', [primary + 1, false]);
    });
    await check('trusted GTK connection remains usable after state changes', async () => {
        assert(await prop('ClientState') === 'running', 'lost authenticated session');
        await emit('ShortcutActivated', ['launcher']);
        await waitFor(windowState, s => placed(s, 560, 620), 'second expansion');
    });
    GLib.file_set_contents(`${out}/done`, failed ? 'FAIL' : 'PASS');
}

function placed(state, width, height) {
    return state?.visible && state.w === width && state.h === height
        && state.x === state.center && state.y === state.top;
}

run().catch(error => {
    failed = true;
    GLib.file_set_contents(`${out}/results-app.txt`, `${results.join('\n')}\nFAIL app observer: ${error.message}\n`);
    GLib.file_set_contents(`${out}/done`, 'FAIL');
}).finally(() => loop.quit());
loop.run();
