// Tidemark compositor adapter for GNOME Shell 46-50.
// Implements only what Mutter does not let a Wayland client do itself
// (placement above other windows, focus/fullscreen tracking, actions on
// other windows, global shortcuts, clipboard while unfocused, keyboard
// state) and exposes it as org.tidemark.Shell1 to the Tidemark client it
// launches. No product logic or UI lives here.

import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';

import { Adapter } from './adapter.js';
import { ShellService } from './dbusService.js';
import { GnomePlatform } from './gnomePlatform.js';

export default class TidemarkExtension extends Extension {
    enable() {
        const log = message => console.log(`[tidemark] ${message}`);
        const [, bytes] = this.dir.get_child('org.tidemark.Shell1.xml').load_contents(null);
        const xml = new TextDecoder().decode(bytes);
        this._platform = new GnomePlatform(log);
        this._service = new ShellService(xml, log);
        this._adapter = new Adapter(this._platform, this._service, log);
        this._service.setAdapter(this._adapter);
        this._platform.connect(this._adapter);
        this._adapter.enable();
        this._service.start();
    }

    disable() {
        this._adapter?.disable();
        this._service?.stop();
        this._platform?.disconnect();
        this._adapter = null;
        this._service = null;
        this._platform = null;
    }
}
