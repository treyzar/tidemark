//! System-bus controls pinned to unique service owners. Run outside the UI thread.
use crate::{Error, Result};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};
use zbus::blocking::{Connection, Proxy};
use zvariant::{OwnedObjectPath, OwnedValue, Value};
const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const UPOWER: &str = "org.freedesktop.UPower";
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const LOGIND: &str = "org.freedesktop.login1";
const LOGIN_PATH: &str = "/org/freedesktop/login1";
type Props = HashMap<String, OwnedValue>;
pub(crate) struct Budget(Instant);
impl Budget {
    pub(crate) fn new() -> Self {
        Self(Instant::now() + Duration::from_secs(12))
    }
    fn check(&self) -> Result<()> {
        if Instant::now() >= self.0 {
            Err(Error::Timeout)
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiState {
    pub enabled: bool,
    pub hardware_enabled: bool,
    pub device_count: usize,
    owner: String,
    devices: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BluetoothAdapter {
    pub path: String,
    pub alias: Option<String>,
    pub powered: bool,
    owner: String,
    address: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryStatus {
    Unknown,
    Charging,
    Discharging,
    Empty,
    FullyCharged,
    PendingCharge,
    PendingDischarge,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Battery {
    pub percentage: f64,
    pub status: BatteryStatus,
    pub time_to_empty: Option<Duration>,
    pub time_to_full: Option<Duration>,
}
#[derive(Clone)]
pub struct Client {
    conn: Connection,
}
fn mapped(error: zbus::Error) -> Error {
    if let zbus::Error::MethodError(name, _, _) = &error {
        return match name.as_str() {
            "org.freedesktop.DBus.Error.AccessDenied"
            | "org.freedesktop.DBus.Error.AuthFailed"
            | "org.freedesktop.NetworkManager.PermissionDenied"
            | "org.freedesktop.NetworkManager.Settings.PermissionDenied"
            | "org.bluez.Error.NotAuthorized"
            | "org.bluez.Error.NotPermitted"
            | "org.freedesktop.login1.AccessDenied" => Error::Denied,
            "org.freedesktop.DBus.Error.NoReply"
            | "org.freedesktop.DBus.Error.Timeout"
            | "org.freedesktop.DBus.Error.TimedOut" => Error::Timeout,
            "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner"
            | "org.freedesktop.DBus.Error.UnknownObject"
            | "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.DBus.Error.UnknownInterface"
            | "org.freedesktop.DBus.Error.UnknownProperty"
            | "org.freedesktop.login1.NoSessionForPID" => Error::Unsupported,
            _ => Error::Transport,
        };
    }
    Error::Transport
}
impl Client {
    pub fn connect() -> Result<Self> {
        async_io::block_on(futures_lite::future::or(
            async {
                let conn = zbus::connection::Builder::system()
                    .map_err(mapped)?
                    .method_timeout(Duration::from_secs(1))
                    .build()
                    .await
                    .map_err(mapped)?;
                Ok(Self {
                    conn: Connection::from(conn),
                })
            },
            async {
                async_io::Timer::after(Duration::from_secs(2)).await;
                Err(Error::Timeout)
            },
        ))
    }
    fn proxy<'a>(
        &'a self,
        owner: &'a str,
        path: &'a str,
        iface: &'a str,
        b: &Budget,
    ) -> Result<Proxy<'a>> {
        b.check()?;
        Proxy::new(&self.conn, owner, path, iface).map_err(mapped)
    }
    fn owner(&self, service: &str, b: &Budget) -> Result<String> {
        let owner: String = self
            .proxy(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                b,
            )?
            .call("GetNameOwner", &(service,))
            .map_err(mapped)?;
        if !owner.starts_with(':') {
            return Err(Error::Changed);
        }
        Ok(owner)
    }
    fn verify(&self, service: &str, owner: &str, b: &Budget) -> Result<()> {
        if self.owner(service, b)? != owner {
            Err(Error::Changed)
        } else {
            Ok(())
        }
    }
    fn properties(&self, owner: &str, path: &str, iface: &str, b: &Budget) -> Result<Props> {
        let properties: Props = self
            .proxy(owner, path, "org.freedesktop.DBus.Properties", b)?
            .call("GetAll", &(iface,))
            .map_err(mapped)?;
        if properties.len() > 64 {
            return Err(Error::Limit);
        }
        Ok(properties)
    }
    fn set_bool(
        &self,
        owner: &str,
        path: &str,
        iface: &str,
        name: &str,
        value: bool,
        b: &Budget,
    ) -> Result<()> {
        self.proxy(owner, path, "org.freedesktop.DBus.Properties", b)?
            .call::<_, _, ()>("Set", &(iface, name, Value::from(value)))
            .map_err(mapped)
    }
    pub fn wifi(&self) -> Result<WifiState> {
        self.wifi_with(&Budget::new())
    }
    fn wifi_with(&self, b: &Budget) -> Result<WifiState> {
        let owner = self.owner(NM, b)?;
        let paths: Vec<OwnedObjectPath> = self
            .proxy(&owner, NM_PATH, NM, b)?
            .call("GetDevices", &())
            .map_err(mapped)?;
        if paths.len() > 64 {
            return Err(Error::Limit);
        }
        let mut seen = HashSet::new();
        let mut devices = Vec::new();
        for path in paths {
            if !seen.insert(path.to_string()) {
                return Err(Error::InvalidResponse);
            }
            let properties = self.properties(
                &owner,
                path.as_str(),
                "org.freedesktop.NetworkManager.Device",
                b,
            )?;
            if number(&properties, "DeviceType")? == 2 {
                devices.push(path.to_string());
            }
        }
        if devices.is_empty() {
            return Err(Error::Unsupported);
        }
        devices.sort();
        let properties = self.properties(&owner, NM_PATH, NM, b)?;
        let result = WifiState {
            enabled: boolean(&properties, "WirelessEnabled")?,
            hardware_enabled: boolean(&properties, "WirelessHardwareEnabled")?,
            device_count: devices.len(),
            owner: owner.clone(),
            devices,
        };
        self.verify(NM, &owner, b)?;
        Ok(result)
    }
    /// Explicit software radio state; hardware_enabled is reported independently.
    pub fn set_wifi(&self, target: &WifiState, want: bool) -> Result<WifiState> {
        let b = Budget::new();
        let before = self.wifi_with(&b)?;
        if before.owner != target.owner || before.devices != target.devices {
            return Err(Error::Changed);
        }
        self.verify(NM, &target.owner, &b)?;
        self.set_bool(&target.owner, NM_PATH, NM, "WirelessEnabled", want, &b)?;
        let until = Instant::now() + Duration::from_secs(1);
        loop {
            let actual = self.wifi_with(&b)?;
            if actual.owner != target.owner || actual.devices != target.devices {
                return Err(Error::Changed);
            }
            if actual.enabled == want {
                return Ok(actual);
            }
            if Instant::now() >= until {
                return Err(Error::NotConfirmed);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    pub fn bluetooth(&self) -> Result<Vec<BluetoothAdapter>> {
        self.bluetooth_with(&Budget::new())
    }
    fn bluetooth_with(&self, b: &Budget) -> Result<Vec<BluetoothAdapter>> {
        let owner = self.owner(BLUEZ, b)?;
        let objects: HashMap<OwnedObjectPath, HashMap<String, Props>> = self
            .proxy(&owner, "/", "org.freedesktop.DBus.ObjectManager", b)?
            .call("GetManagedObjects", &())
            .map_err(mapped)?;
        if objects.len() > 256 {
            return Err(Error::Limit);
        }
        let mut adapters = Vec::new();
        for (path, interfaces) in objects {
            if interfaces.len() > 32 {
                return Err(Error::Limit);
            }
            if let Some(props) = interfaces.get(ADAPTER) {
                if props.len() > 64 || adapters.len() >= 16 {
                    return Err(Error::Limit);
                }
                adapters.push(adapter(path.to_string(), owner.clone(), props)?);
            }
        }
        if adapters.is_empty() {
            return Err(Error::Unsupported);
        }
        adapters.sort_by(|a, b| a.path.cmp(&b.path));
        self.verify(BLUEZ, &owner, b)?;
        Ok(adapters)
    }
    pub fn set_bluetooth_powered(
        &self,
        target: &BluetoothAdapter,
        want: bool,
    ) -> Result<BluetoothAdapter> {
        let b = Budget::new();
        self.verify(BLUEZ, &target.owner, &b)?;
        let before = adapter(
            target.path.clone(),
            target.owner.clone(),
            &self.properties(&target.owner, &target.path, ADAPTER, &b)?,
        )?;
        if before.address != target.address {
            return Err(Error::Changed);
        }
        self.verify(BLUEZ, &target.owner, &b)?;
        self.set_bool(&target.owner, &target.path, ADAPTER, "Powered", want, &b)?;
        let until = Instant::now() + Duration::from_secs(1);
        loop {
            self.verify(BLUEZ, &target.owner, &b)?;
            let actual = adapter(
                target.path.clone(),
                target.owner.clone(),
                &self.properties(&target.owner, &target.path, ADAPTER, &b)?,
            )?;
            if actual.address != target.address {
                return Err(Error::Changed);
            }
            self.verify(BLUEZ, &target.owner, &b)?;
            if actual.powered == want {
                return Ok(actual);
            }
            if Instant::now() >= until {
                return Err(Error::NotConfirmed);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    pub fn battery(&self) -> Result<Battery> {
        let b = Budget::new();
        let owner = self.owner(UPOWER, &b)?;
        let path: OwnedObjectPath = self
            .proxy(&owner, UPOWER_PATH, UPOWER, &b)?
            .call("GetDisplayDevice", &())
            .map_err(mapped)?;
        if path.as_str() == "/" {
            return Err(Error::Unsupported);
        }
        let props = self.properties(&owner, path.as_str(), "org.freedesktop.UPower.Device", &b)?;
        if !boolean(&props, "IsPresent")? || !matches!(number(&props, "Type")?, 2 | 3) {
            return Err(Error::Unsupported);
        }
        let percentage = f64::try_from(props.get("Percentage").ok_or(Error::InvalidResponse)?)
            .map_err(|_| Error::InvalidResponse)?;
        if !percentage.is_finite() || !(0.0..=100.0).contains(&percentage) {
            return Err(Error::InvalidResponse);
        }
        let status = match number(&props, "State")? {
            0 => BatteryStatus::Unknown,
            1 => BatteryStatus::Charging,
            2 => BatteryStatus::Discharging,
            3 => BatteryStatus::Empty,
            4 => BatteryStatus::FullyCharged,
            5 => BatteryStatus::PendingCharge,
            6 => BatteryStatus::PendingDischarge,
            _ => return Err(Error::InvalidResponse),
        };
        let result = Battery {
            percentage,
            status,
            time_to_empty: seconds(&props, "TimeToEmpty")?,
            time_to_full: seconds(&props, "TimeToFull")?,
        };
        self.verify(UPOWER, &owner, &b)?;
        Ok(result)
    }
    /// Only logind writes brightness; never write privileged sysfs attributes.
    pub fn set_brightness(
        &self,
        target: &crate::brightness::Brightness,
        want: u32,
    ) -> Result<crate::brightness::Brightness> {
        if want > target.maximum {
            return Err(Error::InvalidInput);
        }
        let b = Budget::new();
        let before = target.refresh()?;
        if !target.same_device(&before) {
            return Err(Error::Changed);
        }
        let owner = self.owner(LOGIND, &b)?;
        let session: OwnedObjectPath = self
            .proxy(&owner, LOGIN_PATH, "org.freedesktop.login1.Manager", &b)?
            .call("GetSessionByPID", &(std::process::id(),))
            .map_err(mapped)?;
        if !session
            .as_str()
            .starts_with("/org/freedesktop/login1/session/")
        {
            return Err(Error::InvalidResponse);
        }
        self.verify(LOGIND, &owner, &b)?;
        let _: () = self
            .proxy(
                &owner,
                session.as_str(),
                "org.freedesktop.login1.Session",
                &b,
            )?
            .call("SetBrightness", &("backlight", target.name.as_str(), want))
            .map_err(mapped)?;
        let until = Instant::now() + Duration::from_secs(1);
        loop {
            self.verify(LOGIND, &owner, &b)?;
            let after = target.refresh()?;
            if !target.same_device(&after) {
                return Err(Error::Changed);
            }
            if after.current == want {
                return Ok(after);
            }
            if Instant::now() >= until {
                return Err(Error::NotConfirmed);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
fn boolean(props: &Props, key: &str) -> Result<bool> {
    bool::try_from(props.get(key).ok_or(Error::InvalidResponse)?)
        .map_err(|_| Error::InvalidResponse)
}
fn number(props: &Props, key: &str) -> Result<u32> {
    u32::try_from(props.get(key).ok_or(Error::InvalidResponse)?).map_err(|_| Error::InvalidResponse)
}
fn string(props: &Props, key: &str) -> Result<String> {
    let value = <&str>::try_from(props.get(key).ok_or(Error::InvalidResponse)?)
        .map_err(|_| Error::InvalidResponse)?;
    if value.len() > 256 {
        return Err(Error::Limit);
    }
    Ok(value.into())
}
fn adapter(path: String, owner: String, props: &Props) -> Result<BluetoothAdapter> {
    Ok(BluetoothAdapter {
        path,
        owner,
        alias: if props.contains_key("Alias") {
            Some(string(props, "Alias")?)
        } else {
            None
        },
        powered: boolean(props, "Powered")?,
        address: string(props, "Address")?,
    })
}
fn seconds(props: &Props, key: &str) -> Result<Option<Duration>> {
    let Some(value) = props.get(key) else {
        return Ok(None);
    };
    let value = i64::try_from(value).map_err(|_| Error::InvalidResponse)?;
    if value < 0 {
        return Err(Error::InvalidResponse);
    }
    Ok((value > 0).then(|| Duration::from_secs(value as u64)))
}
#[cfg(test)]
#[path = "fixtures.rs"]
mod fixtures;
