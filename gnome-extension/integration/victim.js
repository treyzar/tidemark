#!/usr/bin/gjs -m
// An ordinary application window for the integration harness: a separate
// process the adapter does NOT own, so it is what GetFocus/FocusChanged and
// WindowAction act on. It also plays a clipboard provider on request
// (org.tidemark.HarnessVictim.OfferBytes), because on Wayland only a focused
// client may set the selection: the fixture first activates this window.

import Gdk from 'gi://Gdk?version=4.0';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Gtk from 'gi://Gtk?version=4.0';

const IFACE = `<node>
  <interface name="org.tidemark.HarnessVictim">
    <method name="OfferBytes">
      <arg name="mime" type="s" direction="in"/>
      <arg name="size" type="u" direction="in"/>
      <arg name="secret" type="b" direction="in"/>
    </method>
  </interface>
</node>`;

Gtk.init();
const win = new Gtk.Window({ title: 'tidemark-victim', default_width: 640, default_height: 480 });
win.set_child(new Gtk.Label({ label: 'tidemark integration victim' }));
win.present();

const service = Gio.DBusExportedObject.wrapJSObject(IFACE, {
    // size bytes of 'a' as `mime`; with secret=true also the KDE/KeePassXC
    // password-manager hint, like a password manager copying a password.
    OfferBytes(mime, size, secret) {
        const providers = [Gdk.ContentProvider.new_for_bytes(mime, new GLib.Bytes(new Uint8Array(size).fill(97)))];
        if (secret) {
            providers.push(Gdk.ContentProvider.new_for_bytes('x-kde-passwordManagerHint',
                new GLib.Bytes(new TextEncoder().encode('secret'))));
        }
        win.get_display().get_clipboard().set_content(Gdk.ContentProvider.new_union(providers));
    },
});
service.export(Gio.DBus.session, '/org/tidemark/HarnessVictim');
Gio.bus_own_name_on_connection(Gio.DBus.session, 'org.tidemark.HarnessVictim', Gio.BusNameOwnerFlags.NONE, null, null);

const loop = new GLib.MainLoop(null, false);
win.connect('close-request', () => {
    loop.quit();
    return false;
});
loop.run();
