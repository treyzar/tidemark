// Exercise the actual polling function without starting a shell session or
// touching settings. GetExtensionInfo and sleep are local Bash function stubs.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';

const script = readFileSync(new URL('../integration/run-headless.sh', import.meta.url), 'utf8');
const bash = process.platform === 'win32' && existsSync('C:/Program Files/Git/bin/bash.exe')
    ? 'C:/Program Files/Git/bin/bash.exe' : 'bash';
const quote = value => `'${value.replaceAll("'", "'\\''")}'`;
const info = (state, error = '') => `({'state': <${state}.0>, 'error': <'${error}'>, 'canChange': <false>},)`;

function poll(responses, entry = 'wait_extension_active') {
    const fn = script.slice(script.indexOf('wait_extension_active() {'), script.indexOf('\ninner() {'));
    const result = spawnSync(bash, ['--noprofile', '--norc'], {
        encoding: 'utf8',
        input: `set -euo pipefail
UUID=tidemark@treyzar.github.io
counter=$(mktemp)
trap 'rm -f "$counter"' EXIT
echo 0 >"$counter"
responses=(${responses.map(quote).join(' ')})
gdbus() {
    local n
    n=$(cat "$counter")
    echo $((n + 1)) >"$counter"
    if ((n >= \${#responses[@]})); then n=$((\${#responses[@]} - 1)); fi
    printf '%s\\n' "\${responses[n]}"
}
sleep() { :; }
result() { printf '%s\\n' "$*"; }
${fn}
${entry}
`,
    });
    assert.ifError(result.error);
    return result;
}

test('startup ERROR sentinel with no error can finish loading and become ACTIVE', () => {
    const result = poll([info(3), info(8), info(1)]);
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stdout, /^PASS extension ACTIVE/m);
    assert.doesNotMatch(result.stdout, /^FAIL/m);
});

test('persistent ERROR sentinel still fails the ACTIVE deadline', () => {
    const result = poll([info(3)]);
    assert.equal(result.status, 1, result.stdout + result.stderr);
    assert.match(result.stdout, /FAIL extension not ACTIVE after 30 s/);
});

test('a reported extension error cannot be hidden by a later ACTIVE response', () => {
    const result = poll([info(3, 'enable exploded'), info(1)]);
    assert.equal(result.status, 1, result.stdout + result.stderr);
    assert.match(result.stdout, /FAIL.*enable exploded/);
});

test('OUT_OF_DATE remains a failure', () => {
    const result = poll([info(4), info(1)]);
    assert.equal(result.status, 1, result.stdout + result.stderr);
    assert.match(result.stdout, /FAIL/);
});

test('ACTIVE with a recorded error is rejected', () => {
    const result = poll([info(1, 'earlier error')]);
    assert.equal(result.status, 1, result.stdout + result.stderr);
    assert.match(result.stdout, /FAIL.*earlier error/);
});

test('final state must still be ACTIVE', () => {
    const result = poll([info(3)], 'check_extension_final');
    assert.equal(result.status, 1, result.stdout + result.stderr);
    assert.match(result.stdout, /FAIL extension not ACTIVE and error-free after the fixture/);
});

test('final ACTIVE state cannot hide an error recorded earlier', () => {
    const result = poll([info(1), "(['callback exploded'],)"], 'check_extension_final');
    assert.equal(result.status, 1, result.stdout + result.stderr);
    assert.match(result.stdout, /FAIL extension recorded errors/);
});

test('final ACTIVE state and empty error history pass', () => {
    const result = poll([info(1), '(@as [],)'], 'check_extension_final');
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stdout, /PASS extension remains ACTIVE with no recorded errors/);
});

function logStatus(log) {
    return poll([], `printf '%s\\n' ${quote(log)} >"$counter"
extension_log_has_errors "$counter"`);
}

test('extension callback errors are attributed by their following stack frames', () => {
    const result = logStatus(`(gnome-shell:42): Gjs-CRITICAL **: JS ERROR: Error: callback exploded
    callback@file:///tmp/extensions/tidemark@treyzar.github.io/gnomePlatform.js:12:3
`);
    assert.equal(result.status, 0, result.stdout + result.stderr);
});

test('unrelated dash errors do not attach to a later normal extension message', () => {
    const result = logStatus(`(gnome-shell:42): Gjs-CRITICAL **: JS ERROR: TypeError: dash callback
    callback@resource:///org/gnome/shell/ui/dash.js:12:3
GNOME Shell-Message: Extension tidemark@treyzar.github.io is ACTIVE
`);
    assert.equal(result.status, 1, result.stdout + result.stderr);
});
