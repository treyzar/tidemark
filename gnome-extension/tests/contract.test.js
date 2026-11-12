// Cross-language contract checks: the D-Bus XML, contract.js and
// crates/tidemark-core/src/shell.rs must agree on every wire name.
// Run: cd gnome-extension && npm test  (= node --test)

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';

import * as C from '../tidemark@treyzar.github.io/contract.js';

const EXT = new URL('../tidemark@treyzar.github.io/', import.meta.url);
const xml = readFileSync(new URL('org.tidemark.Shell1.xml', EXT), 'utf8');
const rust = readFileSync(new URL('../../crates/tidemark-core/src/shell.rs', import.meta.url), 'utf8');

const snake = s => s.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase();

function rustBlock(kind, name) {
    const m = rust.match(new RegExp(`pub ${kind} ${name}(?:<[^>]*>)?\\s*\\{([\\s\\S]*?)\\n\\}`));
    assert.ok(m, `${kind} ${name} not found in shell.rs`);
    return m[1].split('\n').map(l => l.replace(/\/\/.*$/, '')).join('\n');
}

function enumVariants(name) {
    return [...rustBlock('enum', name).matchAll(/^\s*([A-Z]\w*)\s*(\{([^}]*)\}|\([^)]*\))?\s*,/gm)].map(m => ({
        name: m[1],
        fields: m[3] ? [...m[3].matchAll(/(\w+)\s*:/g)].map(f => f[1]) : [],
    }));
}

function structFields(name) {
    return [...rustBlock('struct', name).matchAll(/^\s*pub (\w+):\s*([^,]+),/gm)].map(m => ({ name: m[1], type: m[2].trim() }));
}

function xmlMembers(tag) {
    const out = {};
    for (const m of xml.matchAll(new RegExp(`<${tag} name="(\\w+)">([\\s\\S]*?)</${tag}>`, 'g')))
        out[m[1]] = [...m[2].matchAll(/<arg name="(\w+)" type="([^"]+)"(?: direction="(in|out)")?/g)]
            .map(a => ({ name: a[1], type: a[2], dir: a[3] ?? 'out' }));
    return out;
}

test('XML is well formed enough for GDBus: one interface, no DOCTYPE', () => {
    assert.match(xml, /^<node>/);
    assert.equal((xml.match(/<interface name="/g) ?? []).length, 1);
    assert.match(xml, new RegExp(`<interface name="${C.INTERFACE.replace(/\./g, '\\.')}">`));
    assert.doesNotMatch(xml, /<!DOCTYPE/);
    for (const tag of ['interface', 'method', 'signal'])
        assert.equal((xml.match(new RegExp(`<${tag} `, 'g')) ?? []).length,
            (xml.match(new RegExp(`</${tag}>`, 'g')) ?? []).length, `unbalanced <${tag}>`);
});

test('contract version matches Rust', () => {
    assert.equal(Number(rust.match(/SHELL_CONTRACT_VERSION: u32 = (\d+)/)[1]), C.CONTRACT_VERSION);
    assert.match(xml, /<property name="Version" type="u" access="read"\/>/);
    assert.match(xml, /<property name="Capabilities" type="as" access="read"\/>/);
    assert.match(xml, /<property name="ClientState" type="s" access="read"\/>/);
    assert.deepEqual(C.CLIENT_STATES, ['none', 'starting', 'running', 'blocked', 'failed']);
});

test('capability wire names match Rust (enum and pinned JSON)', () => {
    assert.deepEqual(enumVariants('Capability').map(v => snake(v.name)), C.CAPABILITIES);
    const pinned = rust.match(/json!\(\[\s*([\s\S]*?)\]\)/)[1].match(/"(\w+)"/g).map(s => s.slice(1, -1));
    assert.deepEqual(pinned, C.CAPABILITIES);
});

test('only implemented capabilities are advertised', () => {
    for (const c of C.IMPLEMENTED_CAPABILITIES)
        assert.ok(C.CAPABILITIES.includes(c), c);
    for (const c of ['input_region', 'screenshot', 'capture_hiding', 'brightness'])
        assert.ok(!C.IMPLEMENTED_CAPABILITIES.includes(c), `${c} must not be advertised`);
});

test('roles, window actions and shortcut ids match Rust', () => {
    assert.deepEqual(enumVariants('WindowRole').map(v => snake(v.name)), C.ROLES);
    assert.deepEqual(enumVariants('WindowAction').map(v => snake(v.name)), C.WINDOW_ACTIONS);
    assert.deepEqual(enumVariants('ShortcutId').map(v => snake(v.name)), C.SHORTCUT_IDS);
    assert.deepEqual(Object.keys(C.DEFAULT_SHORTCUTS), C.SHORTCUT_IDS);
});

test('a{sv} keys are the serde field names', () => {
    const focus = structFields('FocusInfo');
    assert.deepEqual(focus.map(f => f.name), Object.keys(C.FOCUS_FIELDS));
    assert.deepEqual(focus.filter(f => f.type.startsWith('Option<')).map(f => f.name), C.OPTIONAL_FOCUS_FIELDS);
    assert.deepEqual(structFields('WindowState').map(f => f.name), Object.keys(C.WINDOW_STATE_FIELDS));
    assert.deepEqual(structFields('Rect').map(f => f.name), ['x', 'y', 'w', 'h']);
    // Integer widths: u64 -> t, u32 -> u, u8 -> y, i32 -> i.
    const dbus = { u64: 't', u32: 'u', u8: 'y', bool: 'b', String: 's', Rect: '(iiii)' };
    for (const f of focus) {
        const inner = f.type.replace(/^Option<(.*)>$/, '$1');
        assert.equal(C.FOCUS_FIELDS[f.name], dbus[inner], `FocusInfo.${f.name}`);
    }
});

test('signals match ShellSignal variants, field names and signatures', () => {
    const signals = xmlMembers('signal');
    assert.deepEqual(Object.keys(signals), Object.keys(C.SIGNALS));
    const variants = enumVariants('ShellSignal');
    assert.deepEqual(variants.map(v => snake(v.name)), Object.values(C.SIGNALS).map(([tag]) => tag));
    const offer = structFields('ClipboardOffer').map(f => f.name);
    for (const v of variants) {
        const args = signals[v.name];
        assert.ok(args, `signal ${v.name} missing in XML`);
        // ClipboardChanged flattens its ClipboardOffer into (mimes, sensitive).
        const expected = v.name === 'ClipboardChanged' ? offer : v.fields;
        assert.deepEqual(args.map(a => a.name), expected, v.name);
        assert.equal(`(${args.map(a => a.type).join('')})`, C.SIGNALS[v.name][1], v.name);
    }
});

test('every ShellBackend method has a D-Bus method', () => {
    const methods = xmlMembers('method');
    const trait = rust.match(/pub trait ShellBackend: Send \{([\s\S]*?)\n\}/)[1];
    const fns = [...trait.matchAll(/fn (\w+)\(/g)].map(m => m[1]).filter(f => !['capabilities', 'supports'].includes(f));
    const pascal = s => s.split('_').map(p => p[0].toUpperCase() + p.slice(1)).join('');
    const map = { focus: 'GetFocus' };
    for (const f of fns)
        assert.ok(methods[map[f] ?? pascal(f)], `ShellBackend::${f} has no D-Bus method`);
    assert.deepEqual(methods.WindowAction.map(a => a.type), ['t', 's', 'v', 'a{sv}']);
    assert.deepEqual(methods.Place.map(a => a.type), ['s', '(iiii)', 'a(iiii)']);
    assert.deepEqual(methods.BindShortcuts.map(a => a.type), ['a(ss)', 'a(sb)']);
    assert.deepEqual(methods.Hello.map(a => a.type), ['u', 'as']);
    assert.deepEqual(methods.RequestLaunch.map(a => a.type), ['as', 'b']);
    assert.deepEqual(methods.RequestHandover.map(a => a.type), ['as', 'b']);
});

test('error names cover every ShellError kind', () => {
    assert.deepEqual(enumVariants('ShellError').map(v => snake(v.name)), ['unsupported', 'failed']);
    for (const kind of ['unsupported', 'failed', 'access_denied', 'invalid_args'])
        assert.ok(C.ERROR_NAMES[kind]);
});

test('metadata: uuid is the folder name, required files exist, 46 and 50 declared', () => {
    const meta = JSON.parse(readFileSync(new URL('metadata.json', EXT), 'utf8'));
    assert.equal(meta.uuid, 'tidemark@treyzar.github.io');
    assert.ok(meta['shell-version'].includes('46') && meta['shell-version'].includes('50'));
    const files = readdirSync(EXT);
    for (const f of ['metadata.json', 'extension.js', 'org.tidemark.Shell1.xml', 'contract.js', 'adapter.js',
        'dbusService.js', 'gnomePlatform.js'])
        assert.ok(files.includes(f), f);
});

test('app identity matches the Linux app', () => {
    assert.equal(C.APP_ID, 'io.github.treyzar.Tidemark');
    assert.equal(C.EXECUTABLE, 'tidemark');
    assert.equal(C.CLIENT_PATH, '/usr/bin/tidemark');
    assert.deepEqual(C.DEFAULT_SHORTCUTS, {
        launcher: '<Super><Alt>space', toggle_theme: '<Super><Alt>t', like_track: '<Super><Alt>l',
    });
});

test('pure helpers', () => {
    assert.equal(C.roleForTitle('tidemark-island'), 'island');
    assert.equal(C.roleForTitle('tidemark-overlay'), 'overlay');
    assert.equal(C.roleForTitle('Tidemark — настройки'), 'settings');
    assert.deepEqual(C.islandPosition({ monitor: { x: 0, y: 0, width: 1920, height: 1080 }, panelBottom: 32, width: 400, offsetY: 6 }),
        { x: 760, y: 38 });
    assert.deepEqual(C.islandPosition({ monitor: { x: -1280, y: 0, width: 1280, height: 1024 }, panelBottom: -5, width: 201, offsetY: -3 }),
        { x: -1280 + 539, y: 0 });
    assert.deepEqual(C.tileRect({ x: 0, y: 32, width: 1921, height: 1048 }, 'left'), { x: 0, y: 32, w: 960, h: 1048 });
    assert.deepEqual(C.tileRect({ x: 0, y: 32, width: 1921, height: 1048 }, 'right'), { x: 960, y: 32, w: 961, h: 1048 });
    assert.equal(C.isSensitive(['text/plain', 'x-kde-passwordManagerHint']), true);
    assert.equal(C.isSensitive(['text/plain;charset=utf-8']), false);
    assert.equal(C.validMime('text/plain;charset=utf-8'), true);
    assert.equal(C.validMime('image/png'), true);
    assert.equal(C.validMime('../etc'), false);
    assert.equal(C.normalizeLayout(' ru '), 'RU');
    assert.equal(C.normalizeLayout(''), null);
    assert.deepEqual(C.toRect([1, 2, 3, 4]), { x: 1, y: 2, w: 3, h: 4 });
    assert.equal(C.toRect([1, 2, 3.5, 4]), null);
    assert.equal(C.coversIslandStrip({ x: 0, y: 32, w: 1000, h: 500 }, { x: 0, y: 32, width: 1920, height: 1048 }, false), true);
    assert.equal(C.coversIslandStrip({ x: 0, y: 40, w: 1000, h: 500 }, { x: 0, y: 32, width: 1920, height: 1048 }, false), false);
    assert.equal(C.coversIslandStrip({ x: 0, y: 32, w: 900, h: 500 }, { x: 0, y: 32, width: 1920, height: 1048 }, false), false);
});

test('a{sv} wire form: null is the empty dict, None options omitted, rect as (iiii)', () => {
    assert.deepEqual(C.toWireDict(null, C.FOCUS_FIELDS), {});
    const info = { id: 7, app_id: 'org.gnome.Nautilus', title: 'x', pid: null, maximized: false, above: true,
        opacity: 255, rect: { x: -10, y: 0, w: 800, h: 600 }, monitor: 1 };
    const wire = C.toWireDict(info, C.FOCUS_FIELDS);
    assert.deepEqual(Object.keys(wire), ['id', 'app_id', 'title', 'maximized', 'above', 'opacity', 'rect', 'monitor']);
    assert.deepEqual(wire.rect, ['(iiii)', [-10, 0, 800, 600]]);
    assert.deepEqual(wire.id, ['t', 7]);
    const required = Object.keys(C.FOCUS_FIELDS).filter(k => !C.OPTIONAL_FOCUS_FIELDS.includes(k));
    for (const k of required)
        assert.ok(k in wire, k);
    assert.match(xml, /EMPTY dict means\s+Rust None/);
});

test('Unsupported errors map methods to capability wire names', () => {
    const methods = xmlMembers('method');
    const flat = xml.replace(/\s+/g, ' ');
    for (const [method, cap] of Object.entries(C.METHOD_CAPABILITY)) {
        assert.ok(methods[method], method);
        assert.ok(C.CAPABILITIES.includes(cap), cap);
        assert.ok(flat.includes(`${method} ${cap}`), `XML documents ${method} -> ${cap}`);
    }
});

test('version-sensitive Mutter calls are feature-detected', () => {
    const platform = readFileSync(new URL('gnomePlatform.js', EXT), 'utf8');
    // Removed in Mutter 50: may only be called behind a typeof check.
    const code = platform.split('\n').filter(l => !/^\s*(\/\/|\*|\/\*\*)/.test(l)).join('\n');
    const calls = code.match(/Meta\.is_wayland_compositor\(\)/g) ?? [];
    assert.equal(calls.length, 1, 'exactly one, guarded call');
    assert.match(platform, /typeof Meta\.is_wayland_compositor === 'function' \? Meta\.is_wayland_compositor\(\) : true/);
    assert.match(platform, /typeof Meta\.WaylandClient\.new_subprocess === 'function'/);
    assert.match(platform, /typeof w\.hide_from_window_list === 'function'/);
    assert.match(platform, /typeof w\.get_maximized === 'function'/);
});
