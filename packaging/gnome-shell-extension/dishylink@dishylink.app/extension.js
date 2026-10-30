// Dishylink in GNOME Quick Settings (spec 0189).
//
// The app publishes its live status on D-Bus as com.dishylink.Status1; this
// extension only reads it. It never talks to the dish or the router itself.

import Clutter from "gi://Clutter";
import GObject from "gi://GObject";
import Gio from "gi://Gio";
import GLib from "gi://GLib";
import St from "gi://St";

import { Extension, gettext as _ } from "resource:///org/gnome/shell/extensions/extension.js";
import * as Main from "resource:///org/gnome/shell/ui/main.js";
import * as PopupMenu from "resource:///org/gnome/shell/ui/popupMenu.js";
import { QuickMenuToggle, SystemIndicator } from "resource:///org/gnome/shell/ui/quickSettings.js";

const BUS_NAME = "com.dishylink.app";
const APP_PATH = "/com/dishylink/app";
const STATUS_PATH = "/com/dishylink/app/Status";
const DESKTOP_ID = "com.dishylink.app.desktop";

const STATUS_XML = `<node>
  <interface name="com.dishylink.Status1">
    <property name="Online" type="b" access="read"/>
    <property name="Connection" type="s" access="read"/>
    <property name="Country" type="s" access="read"/>
    <property name="UptimeS" type="t" access="read"/>
    <property name="DownBps" type="d" access="read"/>
    <property name="UpBps" type="d" access="read"/>
    <property name="LatencyMs" type="d" access="read"/>
    <property name="ActiveAlerts" type="u" access="read"/>
    <property name="Summary" type="s" access="read"/>
  </interface>
</node>`;
const StatusProxy = Gio.DBusProxy.makeProxyWrapper(STATUS_XML);

/** Compact bitrate, as the app's tray shows it. */
function rate(bitsPerSecond) {
  if (bitsPerSecond >= 1e9) return `${(bitsPerSecond / 1e9).toFixed(1)} Gb/s`;
  if (bitsPerSecond >= 1e6) return `${(bitsPerSecond / 1e6).toFixed(1)} Mb/s`;
  if (bitsPerSecond >= 1e3) return `${Math.round(bitsPerSecond / 1e3)} Kb/s`;
  return `${Math.round(bitsPerSecond)} b/s`;
}

/** Bring the app forward, or start it if the activation call fails. */
function openApp() {
  Gio.DBus.session.call(
    BUS_NAME,
    APP_PATH,
    "org.freedesktop.Application",
    "Activate",
    new GLib.Variant("(a{sv})", [{}]),
    null,
    Gio.DBusCallFlags.NONE,
    -1,
    null,
    (connection, result) => {
      try {
        connection.call_finish(result);
      } catch {
        Gio.DesktopAppInfo.new(DESKTOP_ID)?.launch([], null);
      }
    },
  );
  Main.panel.closeQuickSettings();
}

const DishylinkToggle = GObject.registerClass(
  class DishylinkToggle extends QuickMenuToggle {
    constructor(icon) {
      super({ title: "Starlink", subtitle: "", gicon: icon, toggleMode: false });
      this.menu.setHeader(icon, "Starlink", "Dishylink Linux");
      this._down = this._row(_("Download"));
      this._up = this._row(_("Upload"));
      this._latency = this._row(_("Latency"));
      this._alerts = this._row(_("Active alerts"));
      this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
      this.menu.addAction(_("Open Dishylink Linux"), openApp);
      // The pill itself opens the app; the arrow opens the figures.
      this.connect("clicked", openApp);
    }

    _row(label) {
      const item = new PopupMenu.PopupMenuItem(label, { reactive: false });
      const value = new St.Label({
        x_expand: true,
        x_align: Clutter.ActorAlign.END,
        style_class: "dishylink-figure",
      });
      item.add_child(value);
      this.menu.addMenuItem(item);
      return value;
    }

    update(proxy) {
      this.subtitle = proxy.Summary ?? "";
      this.checked = Boolean(proxy.Online);
      this._down.text = rate(proxy.DownBps ?? 0);
      this._up.text = rate(proxy.UpBps ?? 0);
      const latency = proxy.LatencyMs ?? -1;
      this._latency.text = latency >= 0 ? `${Math.round(latency)} ms` : "—";
      this._alerts.text = `${proxy.ActiveAlerts ?? 0}`;
    }
  },
);

const DishylinkIndicator = GObject.registerClass(
  class DishylinkIndicator extends SystemIndicator {
    constructor(icon) {
      super();
      this._indicator = this._addIndicator();
      this._indicator.gicon = icon;
      this.toggle = new DishylinkToggle(icon);
      this.quickSettingsItems.push(this.toggle);
      this.setRunning(false);
    }

    setRunning(running) {
      this.visible = running;
      this._indicator.visible = running;
      this.toggle.visible = running;
    }
  },
);

export default class DishylinkExtension extends Extension {
  enable() {
    const icon = Gio.icon_new_for_string(`${this.path}/icons/dishylink-symbolic.svg`);
    this._indicator = new DishylinkIndicator(icon);
    Main.panel.statusArea.quickSettings.addExternalIndicator(this._indicator);
    this._watch = Gio.bus_watch_name(
      Gio.BusType.SESSION,
      BUS_NAME,
      Gio.BusNameWatcherFlags.NONE,
      () => this._appeared(),
      () => this._vanished(),
    );
  }

  _appeared() {
    this._proxy = new StatusProxy(Gio.DBus.session, BUS_NAME, STATUS_PATH, (proxy, error) => {
      if (error || !this._indicator) return;
      this._indicator.setRunning(true);
      this._indicator.toggle.update(proxy);
    });
    this._changed = this._proxy.connect("g-properties-changed", (proxy) =>
      this._indicator?.toggle.update(proxy),
    );
  }

  _vanished() {
    if (this._proxy && this._changed) this._proxy.disconnect(this._changed);
    this._proxy = null;
    this._changed = null;
    this._indicator?.setRunning(false);
  }

  disable() {
    if (this._watch) Gio.bus_unwatch_name(this._watch);
    this._watch = null;
    this._vanished();
    this._indicator?.quickSettingsItems.forEach((item) => item.destroy());
    this._indicator?.destroy();
    this._indicator = null;
  }
}
