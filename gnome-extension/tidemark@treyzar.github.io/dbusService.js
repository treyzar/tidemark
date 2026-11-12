// org.tidemark.Shell1 on the session bus: exports the interface, checks that
// callers of privileged methods are the client process this adapter launched
// (peer PID via org.freedesktop.DBus.GetConnectionUnixProcessID), converts
// between GLib.Variant and the plain values of adapter.js, and sends signals
// only to the client that called Hello (unicast; nothing is broadcast).

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import * as C from './contract.js';
import { AdapterError } from './adapter.js';

const OPEN_METHODS = new Set(['RequestLaunch', 'RequestHandover']);

export class ShellService {
    constructor(xml, log) {
        this._xml = xml;
        this._log = log;
        this._adapter = null;
        this._impl = null;
        this._nameId = 0;
        this._recipient = null; // unique bus name of the authenticated client
        this._pidBySender = new Map(); // sender -> {generation, pid}
        this._watches = new Set();
    }

    setAdapter(adapter) {
        this._adapter = adapter;
    }

    start() {
        const handler = {};
        Object.defineProperty(handler, 'Version', { get: () => C.CONTRACT_VERSION });
        Object.defineProperty(handler, 'Capabilities', { get: () => this._adapter.capabilities() });
        Object.defineProperty(handler, 'ClientState', { get: () => this._adapter.state });
        Object.defineProperty(handler, 'PrimaryMonitor', { get: () => this._adapter.primaryMonitor });
        for (const method of ['RequestLaunch', 'RequestHandover', 'Hello', 'Place', 'GetFocus', 'WindowAction', 'GetClipboard', 'GetClipboardBounded',
            'SetClipboard', 'Screenshot', 'SetCaptureHidden', 'BindShortcuts'])
            handler[`${method}Async`] = (params, invocation) => this._dispatch(method, params, invocation);
        this._impl = Gio.DBusExportedObject.wrapJSObject(this._xml, handler);
        this._impl.export(Gio.DBus.session, C.OBJECT_PATH);
        this._nameId = Gio.bus_own_name_on_connection(Gio.DBus.session, C.BUS_NAME,
            Gio.BusNameOwnerFlags.NONE, null, () => this._log(`lost bus name ${C.BUS_NAME}`));
    }

    stop() {
        if (this._nameId) {
            Gio.bus_unown_name(this._nameId);
            this._nameId = 0;
        }
        for (const id of this._watches)
            Gio.bus_unwatch_name(id);
        this._watches.clear();
        this._impl?.unexport();
        this._impl = null;
        this._recipient = null;
        this._pidBySender.clear();
    }

    // ---- sink for adapter.js ----------------------------------------------

    /** ClientState is open: an untrusted instance watches it to offer a handover. */
    stateChanged(state) {
        try {
            this._impl?.emit_property_changed('ClientState', new GLib.Variant('s', state));
        } catch (e) {
            this._log(`ClientState notification failed: ${e.message}`);
        }
    }

    clientChanged() {
        this._recipient = null;
        this._pidBySender.clear();
    }

    emit(name, args) {
        if (name === 'PrimaryMonitorChanged') {
            try {
                this._impl?.emit_property_changed('PrimaryMonitor', new GLib.Variant('u', args[0]));
            } catch (e) {
                this._log(`PrimaryMonitor notification failed: ${e.message}`);
            }
        }
        if (!this._recipient)
            return;
        const [, signature] = C.SIGNALS[name];
        const value = name === 'FocusChanged' ? [toDict(args[0], C.FOCUS_FIELDS)] : args;
        try {
            Gio.DBus.session.emit_signal(this._recipient, C.OBJECT_PATH, C.INTERFACE, name,
                new GLib.Variant(signature, value));
        } catch (e) {
            this._log(`signal ${name} failed: ${e.message}`);
        }
    }

    // ---- method calls --------------------------------------------------------

    async _dispatch(method, params, invocation) {
        try {
            if (!OPEN_METHODS.has(method) && !await this._authorized(invocation.get_sender()))
                throw new AdapterError('access_denied', 'caller is not the Tidemark client launched by this adapter');
            const reply = await this._call(method, params.map(unwrap), invocation.get_sender());
            invocation.return_value(reply);
            if (method === 'Hello')
                this._adapter.emitSnapshot();
        } catch (e) {
            const kind = e instanceof AdapterError ? e.kind : 'failed';
            if (!(e instanceof AdapterError))
                this._log(`${method} failed: ${e.message}`);
            invocation.return_dbus_error(`${C.ERROR_PREFIX}.${C.ERROR_NAMES[kind]}`, e.message ?? String(e));
        }
    }

    async _call(method, p, sender) {
        const a = this._adapter;
        switch (method) {
        case 'RequestLaunch':
            return new GLib.Variant('(b)', [a.requestLaunch(p[0])]);
        case 'RequestHandover':
            return new GLib.Variant('(b)', [a.requestHandover(p[0], fn => this._whenGone(sender, fn))]);
        case 'Hello': {
            this._recipient = sender;
            const { version, capabilities } = a.hello();
            return new GLib.Variant('(uas)', [version, capabilities]);
        }
        case 'Place':
            a.place(p[0], p[1], p[2]);
            return null;
        case 'GetFocus':
            return new GLib.Variant('(a{sv})', [toDict(a.getFocus(), C.FOCUS_FIELDS)]);
        case 'WindowAction':
            return new GLib.Variant('(a{sv})', [toDict(a.windowAction(p[0], p[1], p[2]), C.WINDOW_STATE_FIELDS)]);
        case 'GetClipboard':
            return new GLib.Variant('(ay)', [await a.getClipboard(p[0])]);
        case 'GetClipboardBounded':
            return new GLib.Variant('(ay)', [await a.getClipboard(p[0], p[1])]);
        case 'SetClipboard':
            a.setClipboard(p[0], p[1]);
            return null;
        case 'Screenshot':
            return a.screenshot();
        case 'SetCaptureHidden':
            return a.setCaptureHidden();
        case 'BindShortcuts':
            return new GLib.Variant('(a(sb))', [a.bindShortcuts(p[0])]);
        }
        throw new AdapterError('invalid_args', `unknown method ${method}`);
    }

    /** Calls `fn` once `sender` (a unique bus name) has left the bus. */
    _whenGone(sender, fn) {
        const id = Gio.bus_watch_name_on_connection(Gio.DBus.session, sender, Gio.BusNameWatcherFlags.NONE,
            null, () => {
                if (!this._watches.delete(id))
                    return;
                Gio.bus_unwatch_name(id);
                fn();
            });
        this._watches.add(id);
    }

    /**
     * The caller is the client launched by this adapter *now*: its peer PID
     * was looked up under the current client generation, and still matches
     * after the asynchronous lookup (the client may have exited, restarted or
     * been re-adopted meanwhile).
     */
    async _authorized(sender) {
        const generation = this._adapter.generation;
        const pid = await this._senderPid(sender, generation);
        return generation === this._adapter.generation && this._adapter.isClientPid(pid);
    }

    _senderPid(sender, generation) {
        const cached = this._pidBySender.get(sender);
        if (cached && cached.generation === generation)
            return Promise.resolve(cached.pid);
        return new Promise(resolve => {
            Gio.DBus.session.call('org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
                'GetConnectionUnixProcessID', new GLib.Variant('(s)', [sender]), new GLib.VariantType('(u)'),
                Gio.DBusCallFlags.NONE, -1, null, (conn, res) => {
                    let pid = 0;
                    try {
                        [pid] = conn.call_finish(res).deepUnpack();
                        if (generation === this._adapter?.generation)
                            this._pidBySender.set(sender, { generation, pid });
                    } catch (e) {
                        this._log(`cannot identify caller ${sender}: ${e.message}`);
                    }
                    resolve(pid);
                });
        });
    }
}

function unwrap(value) {
    return value instanceof GLib.Variant ? value.recursiveUnpack() : value;
}

/** Plain object -> a{sv}; see C.toWireDict (null => empty dict => Rust None). */
function toDict(obj, fields) {
    const dict = {};
    for (const [key, [type, value]] of Object.entries(C.toWireDict(obj, fields)))
        dict[key] = new GLib.Variant(type, value);
    return dict;
}
