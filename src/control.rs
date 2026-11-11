//! Control center: Wi-Fi networks (WlanAPI), Bluetooth devices
//! (BluetoothAPIs), Windows VPN connections (RAS) and power actions.
//! Slow calls run on a worker thread that sleeps until asked; results come
//! back to the island with a posted message.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{w, GUID, HSTRING, PCWSTR};
use windows::Win32::Devices::Bluetooth::*;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LUID, WPARAM};
use windows::Win32::NetworkManagement::Rras::*;
use windows::Win32::NetworkManagement::WiFi::*;
use windows::Win32::Security::{AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY};
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::System::Shutdown::*;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_CONTROL: u32 = WM_APP + 17;

// ---- Model ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Network {
    pub ssid: String,
    /// 0..100
    pub signal: u32,
    pub secured: bool,
    pub connected: bool,
    /// Windows already has a saved profile (no password needed).
    pub saved: bool,
    pub profile: String,
    pub auth: Auth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    Open,
    Wpa2,
    Wpa3,
    Wpa,
    /// Enterprise and others: left to Windows settings.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Audio,
    Input,
    Phone,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub name: String,
    pub address: u64,
    pub kind: DeviceKind,
    pub connected: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Vpn {
    pub name: String,
    pub connected: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub wifi_present: bool,
    pub networks: Vec<Network>,
    pub devices: Vec<Device>,
    pub vpns: Vec<Vpn>,
    pub throne: crate::throne::Status,
    /// One-line outcome of the last action ("Подключено к …").
    pub message: String,
    pub busy: bool,
}

/// Signal 0..100 -> 0..4 bars.
pub fn bars(signal: u32) -> u32 {
    match signal {
        0..=19 => 1,
        20..=44 => 2,
        45..=69 => 3,
        _ => 4,
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// WLAN profile for a personal network; None when Windows settings should
/// handle it (enterprise logins).
pub fn profile_xml(ssid: &str, auth: Auth, password: &str) -> Option<String> {
    let (authentication, encryption) = match auth {
        Auth::Open => ("open", "none"),
        Auth::Wpa2 => ("WPA2PSK", "AES"),
        Auth::Wpa3 => ("WPA3SAE", "AES"),
        Auth::Wpa => ("WPAPSK", "TKIP"),
        Auth::Other => return None,
    };
    let name = xml_escape(ssid);
    let key = if auth == Auth::Open {
        String::new()
    } else {
        format!(
            "<sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{}</keyMaterial></sharedKey>",
            xml_escape(password)
        )
    };
    Some(format!(
        "<?xml version=\"1.0\"?><WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\"><name>{name}</name><SSIDConfig><SSID><name>{name}</name></SSID></SSIDConfig><connectionType>ESS</connectionType><connectionMode>auto</connectionMode><MSM><security><authEncryption><authentication>{authentication}</authentication><encryption>{encryption}</encryption><useOneX>false</useOneX></authEncryption>{key}</security></MSM></WLANProfile>"
    ))
}

pub fn auth_of(algo: DOT11_AUTH_ALGORITHM, secured: bool) -> Auth {
    match algo {
        DOT11_AUTH_ALGO_80211_OPEN if !secured => Auth::Open,
        DOT11_AUTH_ALGO_RSNA_PSK => Auth::Wpa2,
        DOT11_AUTH_ALGO_WPA3_SAE => Auth::Wpa3,
        DOT11_AUTH_ALGO_WPA_PSK => Auth::Wpa,
        _ => Auth::Other,
    }
}

/// Major device class from a Bluetooth class-of-device value.
pub fn kind_of(class: u32) -> DeviceKind {
    match (class >> 8) & 0x1F {
        0x04 => DeviceKind::Audio,
        0x05 => DeviceKind::Input,
        0x02 => DeviceKind::Phone,
        _ => DeviceKind::Other,
    }
}

// ---- Wi-Fi ----------------------------------------------------------------

struct Wlan {
    handle: HANDLE,
    iface: Option<GUID>,
}

impl Wlan {
    fn open() -> Option<Self> {
        let mut version = 0;
        let mut handle = HANDLE::default();
        if unsafe { WlanOpenHandle(2, None, &mut version, &mut handle) } != 0 {
            return None;
        }
        let mut iface = None;
        unsafe {
            let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
            if WlanEnumInterfaces(handle, None, &mut list) == 0 && !list.is_null() {
                if (*list).dwNumberOfItems > 0 {
                    iface = Some((*list).InterfaceInfo[0].InterfaceGuid);
                }
                WlanFreeMemory(list as *const _);
            }
        }
        Some(Self { handle, iface })
    }

    fn scan(&self) {
        if let Some(g) = self.iface {
            unsafe {
                WlanScan(self.handle, &g, None, None, None);
            }
        }
    }

    fn networks(&self) -> Vec<Network> {
        let Some(g) = self.iface else { return Vec::new() };
        let mut out: Vec<Network> = Vec::new();
        unsafe {
            let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
            if WlanGetAvailableNetworkList(self.handle, &g, 0, None, &mut list) != 0 || list.is_null() {
                return out;
            }
            let n = (*list).dwNumberOfItems as usize;
            let items = std::slice::from_raw_parts((*list).Network.as_ptr(), n);
            for it in items {
                let len = (it.dot11Ssid.uSSIDLength as usize).min(32);
                let ssid = String::from_utf8_lossy(&it.dot11Ssid.ucSSID[..len]).into_owned();
                if ssid.is_empty() {
                    continue; // Hidden networks.
                }
                let plen = it.strProfileName.iter().position(|&c| c == 0).unwrap_or(0);
                let profile = String::from_utf16_lossy(&it.strProfileName[..plen]);
                let secured = it.bSecurityEnabled.as_bool();
                let net = Network {
                    connected: it.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0,
                    saved: it.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE != 0,
                    signal: it.wlanSignalQuality,
                    auth: auth_of(it.dot11DefaultAuthAlgorithm, secured),
                    ssid,
                    secured,
                    profile,
                };
                // One row per SSID: keep the connected or strongest entry.
                match out.iter_mut().find(|n| n.ssid == net.ssid) {
                    Some(e) => {
                        if net.connected || (!e.connected && (net.saved && !e.saved || net.signal > e.signal)) {
                            *e = net;
                        }
                    }
                    None => out.push(net),
                }
            }
            WlanFreeMemory(list as *const _);
        }
        out.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.saved.cmp(&a.saved)).then(b.signal.cmp(&a.signal)));
        out
    }

    fn connect_profile(&self, profile: &str) -> bool {
        let Some(g) = self.iface else { return false };
        let name = HSTRING::from(profile);
        let params = WLAN_CONNECTION_PARAMETERS {
            wlanConnectionMode: wlan_connection_mode_profile,
            strProfile: PCWSTR(name.as_ptr()),
            pDot11Ssid: std::ptr::null_mut(),
            pDesiredBssidList: std::ptr::null_mut(),
            dot11BssType: dot11_BSS_type_infrastructure,
            dwFlags: 0,
        };
        unsafe { WlanConnect(self.handle, &g, &params, None) == 0 }
    }

    fn set_profile(&self, xml: &str) -> bool {
        let Some(g) = self.iface else { return false };
        let mut reason = 0u32;
        unsafe { WlanSetProfile(self.handle, &g, 0, &HSTRING::from(xml), PCWSTR::null(), true, None, &mut reason) == 0 }
    }

    fn delete_profile(&self, name: &str) {
        if let Some(g) = self.iface {
            unsafe {
                WlanDeleteProfile(self.handle, &g, &HSTRING::from(name), None);
            }
        }
    }

    fn disconnect(&self) {
        if let Some(g) = self.iface {
            unsafe {
                WlanDisconnect(self.handle, &g, None);
            }
        }
    }

    /// Waits (polling twice a second, only during a connect) for `ssid`.
    fn wait_connected(&self, ssid: &str, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(500));
            if self.networks().iter().any(|n| n.ssid == ssid && n.connected) {
                return true;
            }
        }
        false
    }
}

impl Drop for Wlan {
    fn drop(&mut self) {
        unsafe {
            WlanCloseHandle(self.handle, None);
        }
    }
}

// ---- Bluetooth ------------------------------------------------------------

const AUDIO_SINK: GUID = GUID::from_u128(0x0000110b_0000_1000_8000_00805f9b34fb);
const HANDSFREE: GUID = GUID::from_u128(0x0000111e_0000_1000_8000_00805f9b34fb);

fn bt_devices() -> Vec<(Device, BLUETOOTH_DEVICE_INFO)> {
    let mut out: Vec<(Device, BLUETOOTH_DEVICE_INFO)> = Vec::new();
    let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
        dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
        fReturnAuthenticated: true.into(),
        fReturnRemembered: true.into(),
        fReturnUnknown: false.into(),
        fReturnConnected: true.into(),
        fIssueInquiry: false.into(),
        cTimeoutMultiplier: 0,
        hRadio: HANDLE::default(),
    };
    let mut info = BLUETOOTH_DEVICE_INFO { dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32, ..Default::default() };
    unsafe {
        let Ok(find) = BluetoothFindFirstDevice(&params, &mut info) else { return out };
        loop {
            let len = info.szName.iter().position(|&c| c == 0).unwrap_or(0);
            let name = String::from_utf16_lossy(&info.szName[..len]);
            let address = info.Address.Anonymous.ullLong;
            if !name.is_empty() && !out.iter().any(|(d, _)| d.address == address) {
                out.push((Device { name, address, kind: kind_of(info.ulClassofDevice), connected: info.fConnected.as_bool() }, info));
            }
            info = BLUETOOTH_DEVICE_INFO { dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32, ..Default::default() };
            if BluetoothFindNextDevice(find, &mut info).is_err() {
                break;
            }
        }
        let _ = BluetoothFindDeviceClose(find);
    }
    out.sort_by(|a, b| b.0.connected.cmp(&a.0.connected).then((a.0.kind != DeviceKind::Audio).cmp(&(b.0.kind != DeviceKind::Audio))));
    out
}

/// Connects or disconnects headphones/speakers by enabling or disabling
/// their audio services (A2DP sink and hands-free).
fn bt_set(address: u64, connect: bool) -> bool {
    let Some((_, info)) = bt_devices().into_iter().find(|(d, _)| d.address == address) else { return false };
    let flag = if connect { BLUETOOTH_SERVICE_ENABLE } else { BLUETOOTH_SERVICE_DISABLE };
    let mut ok = false;
    for service in [AUDIO_SINK, HANDSFREE] {
        ok |= unsafe { BluetoothSetServiceState(None, &info, &service, flag) } == 0;
    }
    ok
}

// ---- VPN (Windows built-in, RAS) --------------------------------------------

fn vpn_entries() -> Vec<String> {
    unsafe {
        let mut size = std::mem::size_of::<RASENTRYNAMEW>() as u32;
        let mut count = 0u32;
        let mut buf = vec![RASENTRYNAMEW { dwSize: std::mem::size_of::<RASENTRYNAMEW>() as u32, ..Default::default() }; 16];
        size *= buf.len() as u32;
        if RasEnumEntriesW(PCWSTR::null(), PCWSTR::null(), Some(buf.as_mut_ptr()), &mut size, &mut count) != 0 {
            return Vec::new();
        }
        buf[..count as usize]
            .iter()
            .map(|e| {
                let len = e.szEntryName.iter().position(|&c| c == 0).unwrap_or(0);
                String::from_utf16_lossy(&e.szEntryName[..len])
            })
            .collect()
    }
}

fn vpn_connections() -> Vec<(String, HRASCONN)> {
    unsafe {
        let mut buf = vec![RASCONNW { dwSize: std::mem::size_of::<RASCONNW>() as u32, ..Default::default() }; 8];
        let mut size = (std::mem::size_of::<RASCONNW>() * buf.len()) as u32;
        let mut count = 0u32;
        if RasEnumConnectionsW(Some(buf.as_mut_ptr()), &mut size, &mut count) != 0 {
            return Vec::new();
        }
        buf[..count as usize]
            .iter()
            .map(|c| {
                let len = c.szEntryName.iter().position(|&x| x == 0).unwrap_or(0);
                (String::from_utf16_lossy(&c.szEntryName[..len]), c.hrasconn)
            })
            .collect()
    }
}

fn vpns() -> Vec<Vpn> {
    let live = vpn_connections();
    vpn_entries().into_iter().map(|name| Vpn { connected: live.iter().any(|(n, _)| *n == name), name }).collect()
}

/// Dials a VPN with the credentials Windows saved for it.
fn vpn_connect(name: &str) -> bool {
    unsafe {
        let mut params = RASDIALPARAMSW { dwSize: std::mem::size_of::<RASDIALPARAMSW>() as u32, ..Default::default() };
        for (dst, src) in params.szEntryName.iter_mut().zip(name.encode_utf16()) {
            *dst = src;
        }
        let mut has_password = windows::core::BOOL(0);
        if RasGetEntryDialParamsW(PCWSTR::null(), &mut params, &mut has_password) != 0 {
            return false;
        }
        let mut conn = HRASCONN::default();
        let r = RasDialW(None, PCWSTR::null(), &params, 0, None, &mut conn);
        if r != 0 && !conn.is_invalid() {
            RasHangUpW(conn);
        }
        r == 0
    }
}

fn vpn_disconnect(name: &str) {
    for (n, h) in vpn_connections() {
        if n == name {
            unsafe {
                RasHangUpW(h);
            }
        }
    }
}

// ---- Power --------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    Lock,
    Sleep,
    Restart,
    Shutdown,
}

pub const POWER: [Power; 4] = [Power::Lock, Power::Sleep, Power::Restart, Power::Shutdown];

impl Power {
    pub fn label(self) -> &'static str {
        match self {
            Power::Lock => "Блокировка",
            Power::Sleep => "Сон",
            Power::Restart => "Перезагрузка",
            Power::Shutdown => "Выключение",
        }
    }

    /// Everything but locking asks for a second click.
    pub fn needs_confirm(self) -> bool {
        self != Power::Lock
    }
}

fn enable_shutdown_privilege() {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token).is_err() {
            return;
        }
        let mut luid = LUID::default();
        if LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid).is_ok() {
            let tp = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }] };
            let _ = AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None);
        }
        let _ = CloseHandle(token);
    }
}

pub fn power(action: Power) {
    unsafe {
        match action {
            Power::Lock => {
                let _ = LockWorkStation();
            }
            Power::Sleep => {
                enable_shutdown_privilege();
                let _ = SetSuspendState(false, false, false);
            }
            Power::Restart | Power::Shutdown => {
                enable_shutdown_privilege();
                let flags = if action == Power::Restart { SHUTDOWN_RESTART } else { SHUTDOWN_POWEROFF };
                InitiateShutdownW(PCWSTR::null(), PCWSTR::null(), 0, flags, SHTDN_REASON_FLAG_PLANNED);
            }
        }
    }
}

pub fn open_settings(page: &str) {
    crate::dock::launch(page);
}

#[allow(dead_code)]
const _SETTINGS: PCWSTR = w!("ms-settings:network-wifi");

// ---- Worker ---------------------------------------------------------------------

pub enum Command {
    Refresh,
    WifiConnect { ssid: String, password: Option<String> },
    WifiDisconnect,
    Bluetooth { address: u64, connect: bool },
    VpnConnect(String),
    VpnDisconnect(String),
    ThroneOpen,
    /// Turn the Windows system proxy on/off.
    Proxy(bool),
}

pub struct Service {
    tx: Sender<Command>,
    pub state: Arc<Mutex<State>>,
}

impl Service {
    pub fn start(hwnd: HWND) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<Command>();
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let hwnd_raw = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("control".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let post = || unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd_raw as *mut _)), WM_CONTROL, WPARAM(0), LPARAM(0));
                };
                let set_msg = |m: &str, busy: bool| {
                    let mut s = shared.lock().unwrap();
                    s.message = m.to_string();
                    s.busy = busy;
                };
                while let Ok(cmd) = rx.recv() {
                    let wlan = Wlan::open();
                    match cmd {
                        Command::Refresh => {
                            // Show what is known now, then again once the
                            // scan (a few seconds) has found more.
                            if let Some(w) = &wlan {
                                w.scan();
                            }
                            {
                                let mut s = shared.lock().unwrap();
                                s.wifi_present = wlan.as_ref().is_some_and(|w| w.iface.is_some());
                                s.networks = wlan.as_ref().map(|w| w.networks()).unwrap_or_default();
                                s.devices = bt_devices().into_iter().map(|(d, _)| d).collect();
                                s.vpns = vpns();
                                s.throne = crate::throne::status();
                            }
                            post();
                            std::thread::sleep(Duration::from_millis(3500));
                        }
                        Command::WifiConnect { ssid, password } => {
                            set_msg(&format!("Подключаюсь к «{ssid}»…"), true);
                            post();
                            let ok = wlan.as_ref().is_some_and(|w| {
                                let net = w.networks().into_iter().find(|n| n.ssid == ssid);
                                let fresh = password.is_some();
                                let started = match (&net, &password) {
                                    (Some(n), None) if n.saved => w.connect_profile(&n.profile),
                                    (Some(n), pw) => match profile_xml(&ssid, n.auth, pw.as_deref().unwrap_or("")) {
                                        Some(xml) => w.set_profile(&xml) && w.connect_profile(&ssid),
                                        None => false,
                                    },
                                    _ => false,
                                };
                                let ok = started && w.wait_connected(&ssid, Duration::from_secs(15));
                                if !ok && fresh {
                                    // A wrong password must not stay saved.
                                    w.delete_profile(&ssid);
                                }
                                ok
                            });
                            set_msg(&if ok { format!("Подключено к «{ssid}»") } else { format!("Не удалось подключиться к «{ssid}» — проверьте пароль") }, false);
                        }
                        Command::WifiDisconnect => {
                            if let Some(w) = &wlan {
                                w.disconnect();
                            }
                            std::thread::sleep(Duration::from_millis(800));
                            set_msg("Wi‑Fi отключён", false);
                        }
                        Command::Bluetooth { address, connect } => {
                            set_msg(if connect { "Подключаю…" } else { "Отключаю…" }, true);
                            post();
                            let ok = bt_set(address, connect);
                            std::thread::sleep(Duration::from_millis(1500));
                            set_msg(
                                match (ok, connect) {
                                    (true, true) => "Подключение запрошено",
                                    (true, false) => "Отключено",
                                    _ => "Не получилось — устройство рядом и включено?",
                                },
                                false,
                            );
                        }
                        Command::VpnConnect(name) => {
                            set_msg(&format!("Подключаю VPN «{name}»…"), true);
                            post();
                            let ok = vpn_connect(&name);
                            set_msg(&if ok { format!("VPN «{name}» подключён") } else { format!("VPN «{name}»: нужен вход в параметрах Windows") }, false);
                        }
                        Command::ThroneOpen => crate::throne::open(),
                        Command::Proxy(on) => {
                            let ok = crate::throne::set_proxy(on);
                            set_msg(
                                match (ok, on) {
                                    (true, true) => "Системный прокси включён",
                                    (true, false) => "Системный прокси выключен",
                                    _ => "Не удалось изменить системный прокси",
                                },
                                false,
                            );
                        }
                        Command::VpnDisconnect(name) => {
                            vpn_disconnect(&name);
                            set_msg(&format!("VPN «{name}» отключён"), false);
                        }
                    }
                    // Fresh lists after every command.
                    {
                        let mut s = shared.lock().unwrap();
                        s.wifi_present = wlan.as_ref().is_some_and(|w| w.iface.is_some());
                        s.networks = wlan.as_ref().map(|w| w.networks()).unwrap_or_default();
                        s.devices = bt_devices().into_iter().map(|(d, _)| d).collect();
                        s.vpns = vpns();
                        s.throne = crate::throne::status();
                    }
                    post();
                }
            })
            .ok()?;
        Some(Self { tx, state })
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn get(&self) -> State {
        self.state.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_bars() {
        assert_eq!([bars(5), bars(30), bars(60), bars(95)], [1, 2, 3, 4]);
    }

    #[test]
    fn profiles_escape_and_pick_security() {
        let xml = profile_xml("Кафе & <Wi-Fi>", Auth::Wpa2, "p\"a'ss").unwrap();
        assert!(xml.contains("<name>Кафе &amp; &lt;Wi-Fi&gt;</name>"));
        assert!(xml.contains("<authentication>WPA2PSK</authentication>"));
        assert!(xml.contains("<keyMaterial>p&quot;a&apos;ss</keyMaterial>"));
        let open = profile_xml("Free", Auth::Open, "").unwrap();
        assert!(open.contains("<authentication>open</authentication>") && !open.contains("sharedKey"));
        assert!(profile_xml("Corp", Auth::Other, "x").is_none());
        assert!(profile_xml("New", Auth::Wpa3, "x").unwrap().contains("WPA3SAE"));
    }

    #[test]
    fn device_classes() {
        assert_eq!(kind_of(0x240404), DeviceKind::Audio); // headset
        assert_eq!(kind_of(0x002580), DeviceKind::Input); // mouse
        assert_eq!(kind_of(0x5a020c), DeviceKind::Phone);
        assert_eq!(kind_of(0x000000), DeviceKind::Other);
    }

    /// Live: `cargo test --release control::tests::live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live() {
        let t = Instant::now();
        let w = Wlan::open().expect("wlan");
        w.scan();
        std::thread::sleep(Duration::from_secs(4));
        let nets = w.networks();
        println!("wifi iface: {}, {} networks in {:?}; saved: {}, secured: {}", w.iface.is_some(), nets.len(), t.elapsed(), nets.iter().filter(|n| n.saved).count(), nets.iter().filter(|n| n.secured).count());
        let devs = bt_devices();
        println!("bluetooth: {} devices; audio {}, connected {}", devs.len(), devs.iter().filter(|(d, _)| d.kind == DeviceKind::Audio).count(), devs.iter().filter(|(d, _)| d.connected).count());
        println!("vpn entries: {}", vpns().len());
    }
}
