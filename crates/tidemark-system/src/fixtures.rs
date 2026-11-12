//! Canonical system interfaces on a private SESSION bus, never the host system bus.
//! Run with `dbus-run-session -- cargo test --locked` on Ubuntu.
use super::*;
use std::sync::{Arc, Mutex};
use zbus::blocking::connection::Builder;

const WIFI_DEVICE: &str = "/org/freedesktop/NetworkManager/Devices/1";
const BT_DEVICE: &str = "/org/bluez/hci0";
const BATTERY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
const SESSION: &str = "/org/freedesktop/login1/session/test";
struct State {
    wifi: bool,
    powered: bool,
    deny: bool,
    effect: bool,
    address: String,
    percentage: f64,
    writes: usize,
    brightness: std::path::PathBuf,
}
type Shared = Arc<Mutex<State>>;
struct Network(Shared);
#[zbus::interface(name = "org.freedesktop.NetworkManager")]
impl Network {
    fn get_devices(&self) -> Vec<OwnedObjectPath> {
        vec![OwnedObjectPath::try_from(WIFI_DEVICE).unwrap()]
    }
    #[zbus(property)]
    fn wireless_enabled(&self) -> bool {
        self.0.lock().unwrap().wifi
    }
    #[zbus(property)]
    fn wireless_hardware_enabled(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn set_wireless_enabled(&mut self, want: bool) -> zbus::fdo::Result<()> {
        let mut s = self.0.lock().unwrap();
        if s.deny {
            return Err(zbus::fdo::Error::AccessDenied("fixture".into()));
        }
        s.writes += 1;
        if s.effect {
            s.wifi = want;
        }
        Ok(())
    }
}
struct Device;
#[zbus::interface(name = "org.freedesktop.NetworkManager.Device")]
impl Device {
    #[zbus(property)]
    fn device_type(&self) -> u32 {
        2
    }
}
struct Objects(Shared);
#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl Objects {
    fn get_managed_objects(
        &self,
    ) -> HashMap<OwnedObjectPath, HashMap<String, HashMap<String, Value<'static>>>> {
        let s = self.0.lock().unwrap();
        HashMap::from([(
            OwnedObjectPath::try_from(BT_DEVICE).unwrap(),
            HashMap::from([(
                ADAPTER.into(),
                HashMap::from([
                    ("Powered".into(), Value::from(s.powered)),
                    (
                        "Address".into(),
                        Value::Str(zvariant::Str::from(s.address.clone())),
                    ),
                    ("Alias".into(), Value::from("Synthetic adapter")),
                ]),
            )]),
        )])
    }
}
struct Adapter(Shared);
#[zbus::interface(name = "org.bluez.Adapter1")]
impl Adapter {
    #[zbus(property)]
    fn alias(&self) -> &str {
        "Synthetic adapter"
    }
    #[zbus(property)]
    fn address(&self) -> String {
        self.0.lock().unwrap().address.clone()
    }
    #[zbus(property)]
    fn powered(&self) -> bool {
        self.0.lock().unwrap().powered
    }
    #[zbus(property)]
    fn set_powered(&mut self, want: bool) -> zbus::fdo::Result<()> {
        let mut s = self.0.lock().unwrap();
        if s.deny {
            return Err(zbus::fdo::Error::AccessDenied("fixture".into()));
        }
        s.writes += 1;
        if s.effect {
            s.powered = want;
        }
        Ok(())
    }
}
struct Power;
#[zbus::interface(name = "org.freedesktop.UPower")]
impl Power {
    fn get_display_device(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(BATTERY_DEVICE).unwrap()
    }
}
struct BatteryDevice(Shared);
#[zbus::interface(name = "org.freedesktop.UPower.Device")]
impl BatteryDevice {
    #[zbus(property)]
    fn is_present(&self) -> bool {
        true
    }
    #[zbus(property, name = "Type")]
    fn device_type(&self) -> u32 {
        2
    }
    #[zbus(property)]
    fn percentage(&self) -> f64 {
        self.0.lock().unwrap().percentage
    }
    #[zbus(property)]
    fn state(&self) -> u32 {
        2
    }
    #[zbus(property)]
    fn time_to_empty(&self) -> i64 {
        600
    }
    #[zbus(property)]
    fn time_to_full(&self) -> i64 {
        0
    }
}
struct Manager;
#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl Manager {
    #[zbus(name = "GetSessionByPID")]
    fn session_by_pid(&self, pid: u32) -> zbus::fdo::Result<OwnedObjectPath> {
        if pid != std::process::id() {
            return Err(zbus::fdo::Error::InvalidArgs("fixture PID".into()));
        }
        Ok(OwnedObjectPath::try_from(SESSION).unwrap())
    }
}
struct LoginSession(Shared);
#[zbus::interface(name = "org.freedesktop.login1.Session")]
impl LoginSession {
    fn set_brightness(&self, subsystem: &str, device: &str, want: u32) -> zbus::fdo::Result<()> {
        let mut s = self.0.lock().unwrap();
        if s.deny {
            return Err(zbus::fdo::Error::AccessDenied("fixture".into()));
        }
        if subsystem != "backlight" || device != "panel" || want > 100 {
            return Err(zbus::fdo::Error::InvalidArgs("fixture brightness".into()));
        }
        s.writes += 1;
        if s.effect {
            std::fs::write(&s.brightness, want.to_string()).unwrap();
        }
        Ok(())
    }
}

// A single test owns the canonical names so parallel tests cannot share fakes.
#[test]
fn canonical_system_bus_contracts_and_identity_pinning() {
    let (temp, backlight) = crate::brightness::fixture();
    let state = Arc::new(Mutex::new(State {
        wifi: false,
        powered: false,
        deny: false,
        effect: true,
        address: "00:11:22:33:44:55".into(),
        percentage: 73.5,
        writes: 0,
        brightness: temp.path().join("devices/panel/actual_brightness"),
    }));
    let server = Builder::session()
        .unwrap()
        .name(NM)
        .unwrap()
        .name(BLUEZ)
        .unwrap()
        .name(UPOWER)
        .unwrap()
        .name(LOGIND)
        .unwrap()
        .serve_at(NM_PATH, Network(state.clone()))
        .unwrap()
        .serve_at(WIFI_DEVICE, Device)
        .unwrap()
        .serve_at("/", Objects(state.clone()))
        .unwrap()
        .serve_at(BT_DEVICE, Adapter(state.clone()))
        .unwrap()
        .serve_at(UPOWER_PATH, Power)
        .unwrap()
        .serve_at(BATTERY_DEVICE, BatteryDevice(state.clone()))
        .unwrap()
        .serve_at(LOGIN_PATH, Manager)
        .unwrap()
        .serve_at(SESSION, LoginSession(state.clone()))
        .unwrap()
        .build()
        .unwrap();
    let client = Client {
        conn: Builder::session()
            .unwrap()
            .method_timeout(Duration::from_secs(1))
            .build()
            .unwrap(),
    };
    let wifi = client.wifi().unwrap();
    assert!(!wifi.enabled && wifi.hardware_enabled && wifi.device_count == 1);
    assert!(client.set_wifi(&wifi, true).unwrap().enabled);
    let adapter = client.bluetooth().unwrap().remove(0);
    assert!(!adapter.powered);
    assert!(
        client
            .set_bluetooth_powered(&adapter, true)
            .unwrap()
            .powered
    );
    let battery = client.battery().unwrap();
    assert_eq!(battery.percentage, 73.5);
    assert_eq!(battery.status, BatteryStatus::Discharging);
    assert_eq!(battery.time_to_empty, Some(Duration::from_secs(600)));
    assert!(battery.time_to_full.is_none());
    assert_eq!(client.set_brightness(&backlight, 60).unwrap().current, 60);
    assert_eq!(
        client.set_brightness(&backlight, 101),
        Err(Error::InvalidInput)
    );

    state.lock().unwrap().deny = true;
    assert_eq!(client.set_wifi(&wifi, false), Err(Error::Denied));
    assert_eq!(
        client.set_bluetooth_powered(&adapter, false),
        Err(Error::Denied)
    );
    assert_eq!(client.set_brightness(&backlight, 20), Err(Error::Denied));
    state.lock().unwrap().deny = false;
    state.lock().unwrap().effect = false;
    assert_eq!(client.set_wifi(&wifi, false), Err(Error::NotConfirmed));
    assert_eq!(
        client.set_bluetooth_powered(&adapter, false),
        Err(Error::NotConfirmed)
    );
    assert_eq!(
        client.set_brightness(&backlight, 20),
        Err(Error::NotConfirmed)
    );

    let before = state.lock().unwrap().writes;
    state.lock().unwrap().address = "00:11:22:33:44:66".into();
    assert_eq!(
        client.set_bluetooth_powered(&adapter, false),
        Err(Error::Changed)
    );
    assert_eq!(state.lock().unwrap().writes, before);
    state.lock().unwrap().percentage = f64::NAN;
    assert_eq!(client.battery(), Err(Error::InvalidResponse));

    server.release_name(NM).unwrap();
    let replacement = Builder::session()
        .unwrap()
        .name(NM)
        .unwrap()
        .serve_at(NM_PATH, Network(state.clone()))
        .unwrap()
        .serve_at(WIFI_DEVICE, Device)
        .unwrap()
        .build()
        .unwrap();
    let before = state.lock().unwrap().writes;
    assert_eq!(client.set_wifi(&wifi, false), Err(Error::Changed));
    assert_eq!(state.lock().unwrap().writes, before);
    drop(replacement);
}
