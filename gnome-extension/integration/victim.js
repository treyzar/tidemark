#!/usr/bin/gjs -m
// An ordinary application window for the integration harness: a separate
// process the adapter does NOT own, so it is what GetFocus/FocusChanged and
// WindowAction act on. Optional: TIDEMARK_VICTIM_CLIPBOARD_BYTES=N offers N
// bytes of text/plain on the clipboard 1.5 s after mapping (oversize read test).

import Gdk from 'gi://Gdk?version=4.0';
import GLib from 'gi://GLib';
import Gtk from 'gi://Gtk?version=4.0';

Gtk.init();
const win = new Gtk.Window({ title: 'tidemark-victim', default_width: 640, default_height: 480 });
win.set_child(new Gtk.Label({ label: 'tidemark integration victim' }));
win.present();

const big = Number(GLib.getenv('TIDEMARK_VICTIM_CLIPBOARD_BYTES') ?? 0);
if (big > 0) {
    GLib.timeout_add(GLib.PRIORITY_DEFAULT, 1500, () => {
        const bytes = new GLib.Bytes(new Uint8Array(big).fill(97));
        win.get_display().get_clipboard().set_content(Gdk.ContentProvider.new_for_bytes('text/plain', bytes));
        return GLib.SOURCE_REMOVE;
    });
}

const loop = new GLib.MainLoop(null, false);
win.connect('close-request', () => {
    loop.quit();
    return false;
});
loop.run();
