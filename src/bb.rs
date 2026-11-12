//! Independent, opt-in bb connection. Network I/O runs outside the island UI.
#[path = "bb_panel.rs"]
mod panel;
use crate::{bb_endpoint, config::Config};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicIsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::*, Networking::WinHttp::*, Security::Cryptography::*,
        UI::WindowsAndMessaging::*,
    },
};
pub const WM_UPDATE: u32 = WM_APP + 42;
pub const WM_OPEN: u32 = WM_APP + 43;
static ISLAND: AtomicIsize = AtomicIsize::new(0);
static STATE: OnceLock<Arc<Mutex<State>>> = OnceLock::new();
#[derive(Clone, Default)]
pub struct Connection {
    pub url: String,
    pub token: String,
}
#[derive(Clone, Default)]
pub struct State {
    pub data: Value,
    pub error: String,
    pub revision: u64,
    pub generation: u64,
}
pub fn state() -> Arc<Mutex<State>> {
    STATE
        .get_or_init(|| Arc::new(Mutex::new(State::default())))
        .clone()
}
#[derive(Serialize, Deserialize)]
struct Saved {
    url: String,
    protected_token: Vec<u8>,
}
fn protect(bytes: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        let ok = if encrypt {
            CryptProtectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        ok.ok()?;
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData as *mut c_void)));
        Some(result)
    }
}
pub fn connection() -> Connection {
    let loaded = (|| {
        let saved: Saved =
            serde_json::from_slice(&std::fs::read(Config::dir().join("bb-connection.json")).ok()?)
                .ok()?;
        Some(Connection {
            url: saved.url,
            token: String::from_utf8(protect(&saved.protected_token, false)?).ok()?,
        })
    })();
    loaded.unwrap_or_default()
}
pub fn save(c: &Connection) -> Result<(), String> {
    bb_endpoint::parse(&c.url)?;
    if c.token.is_empty()
        || c.token.len() > 4096
        || !c.token.bytes().all(|b| (0x21..=0x7e).contains(&b))
    {
        return Err("Введите токен плагина bb".into());
    }
    let protected_token =
        protect(c.token.as_bytes(), true).ok_or("Не удалось защитить токен Windows DPAPI")?;
    std::fs::create_dir_all(Config::dir()).map_err(|_| "Не удалось создать каталог настроек")?;
    let bytes = serde_json::to_vec(&Saved {
        url: c.url.clone(),
        protected_token,
    })
    .map_err(|_| "Ошибка настроек")?;
    std::fs::write(Config::dir().join("bb-connection.json"), bytes)
        .map_err(|_| "Не удалось сохранить подключение")?;
    let state = state();
    let mut s = state.lock().unwrap();
    s.generation += 1;
    s.data = Value::Null;
    s.error = "Подключение…".into();
    s.revision += 1;
    Ok(())
}
pub fn disconnect() {
    let _ = std::fs::remove_file(Config::dir().join("bb-connection.json"));
    let state = state();
    let mut s = state.lock().unwrap();
    s.generation += 1;
    s.data = Value::Null;
    s.error = "bb отключён".into();
    s.revision += 1;
    unsafe {
        let _ = PostMessageW(
            Some(HWND(ISLAND.load(Ordering::Relaxed) as *mut _)),
            WM_UPDATE,
            WPARAM(0),
            LPARAM(0),
        );
    }
}
struct Handle(*mut c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}
fn handle(p: *mut c_void) -> Result<Handle, String> {
    if p.is_null() {
        Err("Не удалось открыть соединение bb".into())
    } else {
        Ok(Handle(p))
    }
}
fn net_error(_: windows::core::Error) -> String {
    "bb недоступен: проверьте адрес, сеть и сертификат".into()
}
pub fn request(c: &Connection, route: &str, body: Option<&Value>) -> Result<Value, String> {
    let e = bb_endpoint::parse(&c.url)?;
    if c.token.contains(['\r', '\n']) {
        return Err("Некорректный токен".into());
    }
    unsafe {
        let session = handle(WinHttpOpen(
            w!("Tidemark-bb/1.4"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, 3000, 3000, 5000, 8000).map_err(net_error)?;
        let connect = handle(WinHttpConnect(
            session.0,
            &HSTRING::from(&e.host),
            e.port,
            0,
        ))?;
        let path = format!("{}/api/v1/plugins/tidemark/http/{route}", e.prefix);
        let verb = if body.is_some() {
            w!("POST")
        } else {
            w!("GET")
        };
        let req = handle(WinHttpOpenRequest(
            connect.0,
            verb,
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            if e.secure {
                WINHTTP_FLAG_SECURE
            } else {
                WINHTTP_OPEN_REQUEST_FLAGS(0)
            },
        ))?;
        // Never forward a plugin token to a redirected host.
        let redirect = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        WinHttpSetOption(
            Some(req.0),
            WINHTTP_OPTION_REDIRECT_POLICY,
            Some(&redirect.to_ne_bytes()),
        )
        .map_err(net_error)?;
        let headers: Vec<u16> = format!(
            "x-bb-plugin-token: {}\r\nContent-Type: application/json\r\n",
            c.token
        )
        .encode_utf16()
        .collect();
        let content = body.map(Value::to_string).unwrap_or_default();
        WinHttpSendRequest(
            req.0,
            Some(&headers),
            if content.is_empty() {
                None
            } else {
                Some(content.as_ptr() as *const c_void)
            },
            content.len() as u32,
            content.len() as u32,
            0,
        )
        .map_err(net_error)?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(net_error)?;
        let mut status = 0u32;
        let mut len = 4u32;
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut _ as *mut c_void),
            &mut len,
            std::ptr::null_mut(),
        )
        .map_err(net_error)?;
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0u8; 8192];
            let mut read = 0;
            WinHttpReadData(
                req.0,
                chunk.as_mut_ptr() as *mut c_void,
                chunk.len() as u32,
                &mut read,
            )
            .map_err(net_error)?;
            if read == 0 {
                break;
            }
            if bytes.len() + read as usize > 2 * 1024 * 1024 {
                return Err(
                    "Слишком большой ответ bb; выберите проект в настройках плагина".into(),
                );
            }
            bytes.extend_from_slice(&chunk[..read as usize]);
        }
        let data: Value = serde_json::from_slice(&bytes)
            .map_err(|_| format!("bb вернул HTTP {status}; проверьте адрес и установку плагина"))?;
        if status != 200 {
            return Err(if status == 401 || status == 403 {
                "bb отклонил токен подключения".into()
            } else {
                data["error"]
                    .as_str()
                    .unwrap_or("bb отклонил запрос")
                    .to_string()
            });
        }
        Ok(data)
    }
}
pub fn start(hwnd: HWND) {
    ISLAND.store(hwnd.0 as isize, Ordering::Relaxed);
    std::thread::spawn(|| {
        let mut last = Vec::<String>::new();
        let mut generation = 0;
        loop {
            let current = state().lock().unwrap().generation;
            let c = connection();
            if current != generation {
                generation = current;
                last.clear();
            }
            if !c.url.is_empty() {
                let result = request(&c, "state", None);
                let shared = state();
                let mut s = shared.lock().unwrap();
                if s.generation == generation {
                    match result {
                        Ok(data)
                            if data["version"] == 1
                                && data["threads"].is_array()
                                && data["interactions"].is_array() =>
                        {
                            let ids: Vec<String> = data["interactions"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .filter_map(|x| x["id"].as_str().map(str::to_string))
                                .collect();
                            let notify = ids.iter().any(|id| !last.contains(id))
                                || (!last.is_empty() && ids.is_empty());
                            last = ids;
                            s.data = data;
                            s.error.clear();
                            s.revision += 1;
                            if notify {
                                unsafe {
                                    let _ = PostMessageW(
                                        Some(HWND(ISLAND.load(Ordering::Relaxed) as *mut _)),
                                        WM_UPDATE,
                                        WPARAM(0),
                                        LPARAM(0),
                                    );
                                }
                            }
                        }
                        Ok(_) => {
                            s.error = "Несовместимая версия плагина bb".into();
                            s.revision += 1;
                        }
                        Err(e) => {
                            s.error = e;
                            s.revision += 1;
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(3));
        }
    });
}
pub fn pending_title() -> Option<String> {
    state().lock().ok()?.data["interactions"]
        .as_array()?
        .first()
        .map(|x| {
            x["threadTitle"]
                .as_str()
                .unwrap_or("Агент bb")
                .chars()
                .take(90)
                .collect()
        })
}
pub fn open(_owner: HWND) {
    panel::open();
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dpapi_roundtrip() {
        let token = b"test-token-not-a-real-credential";
        let encrypted = protect(token, true).unwrap();
        assert_ne!(encrypted, token);
        assert_eq!(protect(&encrypted, false).unwrap(), token);
    }
    #[test]
    #[ignore = "requires a configured local bb and the connector plugin"]
    fn live_configured_bb_state() {
        let c = connection();
        assert!(!c.token.is_empty(), "bb connection not configured");
        let data = request(&c, "state", None).expect("native WinHTTP connection to bb");
        assert_eq!(data["version"], 1);
        assert!(data["threads"].is_array());
        assert!(data["interactions"].is_array());
    }
}
