#!/usr/bin/env bash
# Real GNOME Shell integration test for the Tidemark adapter (Linux CI only).
#
# Starts a private, headless gnome-shell (Mutter native backend, virtual
# monitor, no X11) on a private D-Bus session bus. Everything it writes goes
# to a fresh harness root: XDG config/data/cache/state/runtime directories
# point there. HOME is left unchanged and nothing is written to it (on CI
# runners it is the runner's own ephemeral home). The extension is enabled
# only in that root's dconf. The adapter launches
# integration/fixture-client.js as its client via the developer override; the
# fixture talks to org.tidemark.Shell1 from that launched process (real PID
# authentication) and checks results against Mutter through
# org.gnome.Shell.Eval, which only this private --unsafe-mode shell exposes.
#
# Never touches the caller's desktop: no host session bus, no host dconf or
# gsettings, no host extension directory. The outer part refuses to run
# unless CI=true or TIDEMARK_HARNESS_ISOLATED_OK=1; the inner part refuses to
# run unless it was started by the outer part (one-time token in the harness
# root, validated root and XDG paths, a private bus without gnome-shell).
#
# Ubuntu 24.04 packages:
#   sudo apt-get install -y --no-install-recommends gnome-shell gjs gir1.2-gtk-4.0 \
#     dbus dbus-daemon dconf-service dconf-gsettings-backend dconf-cli libglib2.0-bin \
#     libgl1-mesa-dri mesa-vulkan-drivers
# Run:
#   CI=true bash gnome-extension/integration/run-headless.sh
# Optional:
#   TIDEMARK_HARNESS_ARTIFACT_DIR=dir  copy results, phase state and the shell log there
#   TIDEMARK_HARNESS_STRICT=1          [optional] checks fail the run too
#   TIDEMARK_HARNESS_TIMEOUT=seconds   whole private session (default 240)
# Exit status: 0 when every required check passed; 1 when a check failed;
# 2 when the harness refused to run.

set -euo pipefail

UUID=tidemark@treyzar.github.io
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
EXT_SRC="$HERE/../$UUID"
WAYLAND_NAME=tidemark-test-0
ROOT_PREFIX="${TMPDIR:-/tmp}/tidemark-gnome-harness."

die() { echo "run-headless: $*" >&2; exit 2; }
result() { echo "$*" >>"$TIDEMARK_HARNESS_OUT/results-harness.txt"; }

# ---- inner part: runs inside dbus-run-session ------------------------------------

validate_inner() {
    local root=${TIDEMARK_HARNESS_ROOT:-}
    [[ -n "$root" && "$root" == "$ROOT_PREFIX"* && -d "$root" && ! -L "$root" ]] \
        || die "inner: harness root missing or not created by this script"
    [[ "$(stat -c %u "$root")" == "$(id -u)" && "$(stat -c %a "$root")" == 700 ]] \
        || die "inner: harness root not private to this user"
    [[ -f "$root/.inner-token" && -n "${TIDEMARK_HARNESS_INNER_TOKEN:-}" &&
        "$(cat "$root/.inner-token")" == "$TIDEMARK_HARNESS_INNER_TOKEN" ]] \
        || die "inner: invalid token; run the script without --inner"
    [[ "${XDG_CONFIG_HOME:-}" == "$root/config" && "${XDG_DATA_HOME:-}" == "$root/data" &&
        "${XDG_CACHE_HOME:-}" == "$root/cache" && "${XDG_STATE_HOME:-}" == "$root/state" &&
        "${XDG_RUNTIME_DIR:-}" == "$root/runtime" ]] \
        || die "inner: XDG directories are not inside the harness root"
    [[ -n "${DBUS_SESSION_BUS_ADDRESS:-}" &&
        "$DBUS_SESSION_BUS_ADDRESS" != "${TIDEMARK_HARNESS_OUTER_BUS:-}" ]] \
        || die "inner: not on a private session bus"
    # A real desktop bus has gnome-shell on it; the private one must not.
    gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.NameHasOwner org.gnome.Shell | grep -q '(false,)' \
        || die "inner: org.gnome.Shell already present on this bus; refusing"
    # The one-time token is consumed: a second --inner cannot reuse it.
    rm -f "$root/.inner-token"
}

wait_extension_active() {
    local info state i
    for ((i = 0; i < 60; i++)); do
        info=$(gdbus call --session --dest org.gnome.Shell --object-path /org/gnome/Shell \
            --method org.gnome.Shell.Extensions.GetExtensionInfo "$UUID" 2>&1 || true)
        state=$(grep -oE "'state': <[0-9.]+>" <<<"$info" | grep -oE '[0-9]+' | head -n1 || true)
        if [[ -n "$state" ]] && ! grep -qF "'error': <''>" <<<"$info"; then
            result "FAIL extension reported an error: $info"
            return 1
        fi
        case "$state" in
        1) result "PASS extension ACTIVE in the private shell"; return 0 ;;
        4) result "FAIL extension OUT_OF_DATE: $info"; return 1 ;;
        esac
        # GNOME 46.2 and 50.1 loadExtension() default to ERROR (3) before
        # awaiting the extension import, with error:'' and canChange:false.
        # Only the empty-error sentinel may settle; a real error fails above,
        # and a sentinel that never reaches ACTIVE still fails the deadline.
        # https://github.com/GNOME/gnome-shell/blob/50.1/js/ui/extensionSystem.js
        sleep 0.5
    done
    result "FAIL extension not ACTIVE after 30 s: $info"
    return 1
}

check_extension_final() {
    local info errors
    info=$(gdbus call --session --dest org.gnome.Shell --object-path /org/gnome/Shell \
        --method org.gnome.Shell.Extensions.GetExtensionInfo "$UUID" 2>&1 || true)
    if ! grep -qE "'state': <1(\\.0)?>" <<<"$info" || ! grep -qF "'error': <''>" <<<"$info"; then
        result "FAIL extension not ACTIVE and error-free after the fixture: $info"
        return 1
    fi
    errors=$(gdbus call --session --dest org.gnome.Shell --object-path /org/gnome/Shell \
        --method org.gnome.Shell.Extensions.GetExtensionErrors "$UUID" 2>&1 || true)
    if [[ "$errors" != '(@as [],)' && "$errors" != '([],)' ]]; then
        result "FAIL extension recorded errors: $errors"
        return 1
    fi
    result "PASS extension remains ACTIVE with no recorded errors after the fixture"
}

extension_log_has_errors() {
    # GJS prints a callback error header and its indented stack on separate
    # lines. Keep the record together: grepping only the header drops the
    # extension path, while grepping the entire file misattributes dash errors.
    awk -v uuid="$UUID" '
        /JS ERROR|JS WARNING/ { in_error = 1; if (index($0, uuid)) found = 1; next }
        in_error && /^[[:space:]]/ { if (index($0, uuid)) found = 1; next }
        /^[[:space:]]*$/ { next }
        { in_error = 0 }
        END { exit !found }
    ' "$1"
}

inner() {
    validate_inner

    gsettings set org.gnome.shell disable-user-extensions false
    gsettings set org.gnome.shell enabled-extensions "['$UUID']"
    gsettings set org.gnome.shell welcome-dialog-last-shown-version '999' || true
    gsettings set org.gnome.desktop.input-sources sources "[('xkb', 'us'), ('xkb', 'ru')]"
    [[ "$(gsettings get org.gnome.shell enabled-extensions)" == *"$UUID"* ]] \
        || die "private gsettings write did not stick (are dconf-service and dconf-gsettings-backend installed?)"

    # Own process group: the shell, the fixture it launches and the victim the
    # fixture launches are all stopped together, nothing else.
    setsid gnome-shell --headless --wayland --no-x11 --virtual-monitor 1280x800 \
        --wayland-display "$WAYLAND_NAME" --unsafe-mode >"$TIDEMARK_HARNESS_ROOT/gnome-shell.log" 2>&1 &
    local shell_pid=$!
    trap 'kill -TERM -- "-'"$shell_pid"'" 2>/dev/null || true; sleep 1; kill -KILL -- "-'"$shell_pid"'" 2>/dev/null || true' EXIT

    if ! gdbus wait --session --timeout 60 org.gnome.Shell; then
        result "FAIL gnome-shell did not start"
        return 0
    fi
    wait_extension_active || true

    if gdbus wait --session --timeout 30 org.tidemark.Shell; then
        result "PASS org.tidemark.Shell owned"
        if gdbus call --session --dest org.tidemark.Shell --object-path /org/tidemark/Shell \
            --method org.freedesktop.DBus.Properties.Get org.tidemark.Shell1 Version | grep -q 'uint32 1'; then
            result "PASS Version readable by any caller"
        else
            result "FAIL Version not readable"
        fi
        local denied
        denied=$(gdbus call --session --dest org.tidemark.Shell --object-path /org/tidemark/Shell \
            --method org.tidemark.Shell1.GetFocus 2>&1 || true)
        if [[ "$denied" == *org.tidemark.Shell1.Error.AccessDenied* ]]; then
            result "PASS foreign caller gets AccessDenied"
        else
            result "FAIL foreign caller not denied: $denied"
        fi
    else
        result "FAIL org.tidemark.Shell never appeared"
    fi

    local waited=0
    while [[ ! -f "$TIDEMARK_HARNESS_OUT/done" ]]; do
        if ! kill -0 "$shell_pid" 2>/dev/null; then
            result "FAIL gnome-shell exited early"
            break
        fi
        if ((waited++ >= 150)); then
            result "FAIL fixture did not finish within 150 s"
            break
        fi
        sleep 1
    done

    result "INFO final ClientState: $(gdbus call --session --dest org.tidemark.Shell --object-path /org/tidemark/Shell \
        --method org.freedesktop.DBus.Properties.Get org.tidemark.Shell1 ClientState 2>&1 || true)"
    check_extension_final || true
    if extension_log_has_errors "$TIDEMARK_HARNESS_ROOT/gnome-shell.log"; then
        result "FAIL gnome-shell logged JS errors from the extension"
    fi
}

if [[ "${1:-}" == "--inner" ]]; then
    inner
    exit 0
fi

# ---- outer part -------------------------------------------------------------------

[[ "$(uname -s)" == Linux ]] || die "Linux only"
[[ "${CI:-}" == true || "${TIDEMARK_HARNESS_ISOLATED_OK:-}" == 1 ]] \
    || die "refusing to run outside CI; set TIDEMARK_HARNESS_ISOLATED_OK=1 to run the isolated session on purpose"
for cmd in gnome-shell gjs dbus-run-session gsettings gdbus timeout setsid stat; do
    command -v "$cmd" >/dev/null || die "missing $cmd (see the apt line at the top)"
done

TIDEMARK_HARNESS_ROOT=$(mktemp -d "${ROOT_PREFIX}XXXXXX")
chmod 700 "$TIDEMARK_HARNESS_ROOT"
TIDEMARK_HARNESS_OUT="$TIDEMARK_HARNESS_ROOT/out"
mkdir -p "$TIDEMARK_HARNESS_OUT" "$TIDEMARK_HARNESS_ROOT"/{config,data/gnome-shell/extensions,cache,state,runtime,fixture}
chmod 700 "$TIDEMARK_HARNESS_ROOT/runtime"
cp -r "$EXT_SRC" "$TIDEMARK_HARNESS_ROOT/data/gnome-shell/extensions/$UUID"
cp "$HERE/fixture-client.js" "$HERE/victim.js" "$EXT_SRC/contract.js" "$TIDEMARK_HARNESS_ROOT/fixture/"
chmod 755 "$TIDEMARK_HARNESS_ROOT/fixture/fixture-client.js" "$TIDEMARK_HARNESS_ROOT/fixture/victim.js"
TIDEMARK_HARNESS_INNER_TOKEN=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
(umask 077 && printf '%s' "$TIDEMARK_HARNESS_INNER_TOKEN" >"$TIDEMARK_HARNESS_ROOT/.inner-token")

TIDEMARK_HARNESS_OUTER_BUS=${DBUS_SESSION_BUS_ADDRESS:-}
unset DBUS_SESSION_BUS_ADDRESS DISPLAY WAYLAND_DISPLAY WAYLAND_SOCKET XDG_SESSION_ID
export XDG_CONFIG_HOME="$TIDEMARK_HARNESS_ROOT/config" XDG_DATA_HOME="$TIDEMARK_HARNESS_ROOT/data" \
    XDG_CACHE_HOME="$TIDEMARK_HARNESS_ROOT/cache" XDG_STATE_HOME="$TIDEMARK_HARNESS_ROOT/state" \
    XDG_RUNTIME_DIR="$TIDEMARK_HARNESS_ROOT/runtime" XDG_CURRENT_DESKTOP=GNOME XDG_SESSION_TYPE=wayland
export GSETTINGS_BACKEND=dconf GSK_RENDERER=cairo GDK_BACKEND=wayland LIBGL_ALWAYS_SOFTWARE=1 NO_AT_BRIDGE=1 GTK_A11Y=none
# The adapter's developer override (absolute, executable) points at the fixture.
export TIDEMARK_ADAPTER_CLIENT="$TIDEMARK_HARNESS_ROOT/fixture/fixture-client.js" \
    TIDEMARK_VICTIM="$TIDEMARK_HARNESS_ROOT/fixture/victim.js" TIDEMARK_HARNESS_DIR="$TIDEMARK_HARNESS_OUT" \
    TIDEMARK_TEST_WAYLAND_DISPLAY="$WAYLAND_NAME" TIDEMARK_HARNESS_ROOT TIDEMARK_HARNESS_OUT \
    TIDEMARK_HARNESS_INNER_TOKEN TIDEMARK_HARNESS_OUTER_BUS

echo "run-headless: private session in $TIDEMARK_HARNESS_ROOT"
timeout "${TIDEMARK_HARNESS_TIMEOUT:-240}" dbus-run-session -- bash "$0" --inner \
    || result "FAIL private session timed out, crashed or refused (see above)"

echo "---- results"
cat "$TIDEMARK_HARNESS_OUT"/results-harness.txt "$TIDEMARK_HARNESS_OUT"/results-phase1.txt \
    "$TIDEMARK_HARNESS_OUT"/results-phase2.txt 2>/dev/null || true
results=$(cat "$TIDEMARK_HARNESS_OUT"/results-*.txt 2>/dev/null || true)
status=0
if grep -E '^FAIL ' <<<"$results" | grep -qv '^FAIL \[optional\]'; then status=1; fi
if [[ "${TIDEMARK_HARNESS_STRICT:-}" == 1 ]] && grep -qE '^FAIL \[optional\]' <<<"$results"; then status=1; fi
for required in results-phase1.txt results-phase2.txt; do
    if [[ ! -f "$TIDEMARK_HARNESS_OUT/$required" ]]; then
        echo "FAIL missing $required"
        status=1
    fi
done
if ((status)); then
    echo "---- gnome-shell log (tail)"
    tail -n 200 "$TIDEMARK_HARNESS_ROOT/gnome-shell.log" 2>/dev/null || true
fi

if [[ -n "${TIDEMARK_HARNESS_ARTIFACT_DIR:-}" ]]; then
    # Only files this harness produced.
    mkdir -p "$TIDEMARK_HARNESS_ARTIFACT_DIR"
    for f in "$TIDEMARK_HARNESS_OUT"/results-*.txt "$TIDEMARK_HARNESS_OUT"/phase1.json \
        "$TIDEMARK_HARNESS_OUT"/done "$TIDEMARK_HARNESS_ROOT/gnome-shell.log"; do
        [[ -f "$f" ]] && cp "$f" "$TIDEMARK_HARNESS_ARTIFACT_DIR/" || true
    done
    echo "run-headless: artifacts copied to $TIDEMARK_HARNESS_ARTIFACT_DIR"
fi
echo "run-headless: harness root kept at $TIDEMARK_HARNESS_ROOT"
exit "$status"
