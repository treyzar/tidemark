# Linux Yandex Music favorites through AT-SPI2

The standalone `crates/tidemark-yandex` library implements actual, blocking Linux
accessibility transport and explicit desired-state favorite changes. This is a
port component; the parent owns UI integration and its bounded worker. There are
no credentials, music API/private endpoints, browser automation, shell-outs,
simulated input, browser activation, or live desktop operations in development.

## API and trust checks

`Client::connect()` discovers the dedicated accessibility bus through the session
bus `org.a11y.Bus.GetAddress`, then captures the accessibility registry's unique
owner. `liked(title, artist)` performs read-only verification;
`set_liked(title, artist, desired_bool)` performs at most one accessibility action
and returns `Ok(bool)` only after observing the requested state. Call these from
a worker, never the GTK main loop. UI must retain errors and must not optimistically
mark a track liked on a method acknowledgment alone.

Discovery uses the registry root `/org/a11y/atspi/accessible/root` and
`org.a11y.atspi.Accessible.GetChildren`. The official Socket interface is the
application embedding/registration protocol; it has no GetChildren method.
Clients do not register themselves as accessible applications merely to query
the tree. References use the canonical `(so)` bus-name/object-path structure.

Every application must be owned by the current effective UID, have a real bus
PID, and have a `/proc/<pid>/exe` basename in the explicit Chromium/Firefox/browser
allowlist, mirroring the Windows executable filter. All traversal and mutation
destinations are pinned unique owners. A changed registry owner, vanished owner,
foreign owner in a browser subtree, or unverifiable process fails closed.

A unique visible browser document must expose a browser-provided Document `URI`
with HTTPS and the exact authority `music.yandex.ru`, `.com`, `.by`, or `.kz`.
Userinfo, ports, suffix hosts, trailing dots, encoded hosts, Unicode lookalikes,
whitespace/control characters and backslashes fail. Page text, arbitrary form
inputs, branded titles, and title-only matching never establish origin. Nested
document boundaries stop traversal so an iframe's player cannot establish the
parent document's identity. No native-app exception weakens URL verification.

Inside that document, a unique visible `Плеер` container must contain the exact
normalized title and artist in text/label/static descendants. Whitespace/case and
an artist's trailing separator follow the existing Windows rules; substring
matches are rejected. There must be exactly one `Нравится` button, with a supported
toggle/checkable role, enabled/sensitive/showing/visible states, and a definite
non-stale, non-defunct, non-indeterminate favorite state. GetState's `au` payload
is decoded as two packed uint32 words, as implemented by GNOME, rather than a
list of state enum IDs.

Two complete evidence snapshots must agree before mutation; after checking the
single Action capability, a third snapshot must agree. Only one `DoAction(0)` is
allowed, only when current state differs from the explicit desired bool. The
transport awaits its result and re-reads origin, exact track, object identities
and state afterward. It never tries another player after a possibly applied
mutation. Post-action track change, state that never converges, false action
result, remote exception or lost accessibility object cannot report success.

## Bounds

Both session/accessibility connection establishment use cancellation-safe async
races bounded to two seconds. Method waits are 700 ms each. Each read/change has
a twelve-second operation budget and 12,000 transaction-check cap; a pending
method may exceed the operation deadline by at most its 700 ms timeout. State
confirmation has a two-second subdeadline. Tree walks are capped at 4096 nodes,
64 levels and 256 children per node, and reject cycles. Registry discovery caps
applications at 64; names/URIs at 8192 bytes; interface lists at 32; state words
must number exactly two. Errors are fixed enums without private document/track
text or remote exception details. No operation retries an action automatically.

## Verification and limits

Windows portable tests cover strict trusted origins, exact title+artist matching,
and definite state decoding. An origin-validator RED failure was observed before
implementation, then all three portable tests passed. Linux-target all-target
checking compiles production transport and canonical fake D-Bus fixtures.

Twelve native unit fixtures use a private `dbus-run-session` bus, unique names per
connection, actual bus UID/PID lookups and proc executable verification (the
private fixture client allowlist admits its own test binary). They cover dedicated
bus discovery, observed favorite changes/idempotence, spoof hosts, changed tracks,
ambiguous buttons, remote errors, post-action track changes, acknowledgments with
no state effect, and rejection of an ordinary process under the production browser
allowlist, missing URI, indeterminate state, cyclic tree limits and changed registry owner. These tests never inspect or control a real desktop. Native execution
must occur in parent Ubuntu CI:

```
dbus-run-session -- cargo test --locked --manifest-path crates/tidemark-yandex/Cargo.toml
```

This Windows host cannot execute native fixtures. Fake transport success does not
prove the current Yandex website/browser exports the required accessible URI,
player, title/artist, toggle state and action. No physical browser probe has been
performed, and real browser favorite support remains unverified. If any required
evidence is omitted by a browser, the result is an explicit error, not a title-only
trust downgrade or a claimed successful like.

## Primary references inspected

- [GNOME accessibility bus discovery](https://github.com/GNOME/at-spi2-core/blob/main/bus/README.md)
- [Accessible methods and wire types](https://gnome.pages.gitlab.gnome.org/at-spi2-core/devel-docs/doc-org.a11y.atspi.Accessible.html)
- [Document attribute API](https://gnome.pages.gitlab.gnome.org/at-spi2-core/devel-docs/doc-org.a11y.atspi.Document.html)
- [Action API](https://gnome.pages.gitlab.gnome.org/at-spi2-core/devel-docs/doc-org.a11y.atspi.Action.html)
- [Socket embedding protocol](https://gnome.pages.gitlab.gnome.org/at-spi2-core/devel-docs/doc-org.a11y.atspi.Socket.html)
- [GNOME state serialization implementation](https://raw.githubusercontent.com/GNOME/at-spi2-core/main/atk-adaptor/adaptors/accessible-adaptor.c)
- [State enumeration](https://gnome.pages.gitlab.gnome.org/at-spi2-core/libatspi/enum.StateType.html)
- [Role enumeration](https://gnome.pages.gitlab.gnome.org/at-spi2-core/libatspi/enum.Role.html)
- [Chromium document attribute implementation: URI](https://chromium.googlesource.com/chromium/src/+/refs/heads/main/ui/accessibility/platform/ax_platform_node_auralinux.cc)

