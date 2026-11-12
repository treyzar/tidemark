// Static checks of the real-GNOME harness (it only runs on Linux CI):
// scripts parse, the isolation guards and Mutter flags are present, and the
// fixture uses the production contract module rather than its own copy.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const dir = new URL('../integration/', import.meta.url);
const read = name => readFileSync(new URL(name, dir), 'utf8');

test('fixture and victim parse as ES modules with a gjs -m shebang and LF endings', () => {
    for (const name of ['fixture-client.js', 'victim.js']) {
        execFileSync(process.execPath, ['--check', fileURLToPath(new URL(name, dir))]);
        const text = read(name);
        assert.ok(text.startsWith('#!/usr/bin/gjs -m\n'), `${name} shebang`);
        assert.ok(!text.includes('\r'), `${name} has CR`);
    }
});

test('harness isolates the session and uses Mutter 46 headless flags', () => {
    const sh = read('run-headless.sh');
    assert.ok(!sh.includes('\r'), 'LF endings');
    for (const needle of [
        'dbus-run-session', '--headless', '--no-x11', '--virtual-monitor 1280x800', '--wayland-display',
        '--unsafe-mode', 'XDG_CONFIG_HOME="$TIDEMARK_HARNESS_ROOT/config"', 'XDG_DATA_HOME="$TIDEMARK_HARNESS_ROOT/data"',
        'unset DBUS_SESSION_BUS_ADDRESS', 'TIDEMARK_HARNESS_ISOLATED_OK', 'TIDEMARK_ADAPTER_CLIENT=',
        'GetExtensionInfo', 'AccessDenied', 'TIDEMARK_HARNESS_ARTIFACT_DIR', 'setsid gnome-shell',
    ])
        assert.ok(sh.includes(needle), needle);
    assert.doesNotMatch(sh, /(^|[\s;])(export\s+)?HOME=/m, 'HOME must not be reassigned');
    assert.doesNotMatch(sh, /pkill/, 'no pattern-based process killing');
    assert.doesNotMatch(sh, /\$TMP\b|\$OUT\b/, 'harness-specific variable names only');
});

test('inner part validates token, paths and the private bus before any gsettings write', () => {
    const sh = read('run-headless.sh');
    // '\ninner() {' so that 'validate_inner() {' does not match.
    const inner = sh.slice(sh.indexOf('\ninner() {'), sh.indexOf('if [[ "${1:-}" == "--inner" ]]'));
    assert.ok(inner.startsWith('\ninner() {'), 'inner() found');
    assert.ok(inner.indexOf('validate_inner') < inner.indexOf('gsettings set'), 'validation first');
    const validate = sh.slice(sh.indexOf('validate_inner() {'), sh.indexOf('wait_extension_active() {'));
    for (const needle of ['.inner-token', 'TIDEMARK_HARNESS_INNER_TOKEN', 'stat -c %u', 'stat -c %a',
        'XDG_CONFIG_HOME', 'XDG_RUNTIME_DIR', 'TIDEMARK_HARNESS_OUTER_BUS', 'NameHasOwner org.gnome.Shell',
        'rm -f "$root/.inner-token"'])
        assert.ok(validate.includes(needle), needle);
    const outer = sh.slice(sh.indexOf('if [[ "${1:-}" == "--inner" ]]'));
    assert.ok(!/gsettings set/.test(outer), 'no gsettings writes outside the private session');
    assert.match(sh, /for \(\(i = 0; i < 60; i\+\+\)\)/, 'extension state is polled, not asserted once');
});

test('fixture imports the shipped contract module', () => {
    const fx = read('fixture-client.js');
    assert.match(fx, /import \* as C from '\.\/contract\.js';/);
    assert.match(read('run-headless.sh'), /cp "\$HERE\/fixture-client\.js" "\$HERE\/victim\.js" "\$EXT_SRC\/contract\.js"/);
});
