//! Tidemark settings: native Win32 controls with a DPI-aware GDI skin.
//! Edits config.toml and tells the island to reload it.

use crate::autostart;
use crate::config::{parse_hex, Config};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;
use windows::core::{w, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    IsWindowEnabled, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::*;

use tidemark_core::settings_motion as motion;
#[path = "settings_skin.rs"]
mod skin;
use motion::Tween;
const TIMER_MOTION: usize = 0x5449;
#[derive(Clone, Copy)]
struct ControlMotion {
    hover: Tween,
    toggle: Tween,
}
impl Default for ControlMotion {
    fn default() -> Self {
        Self {
            hover: Tween::new(0.0),
            toggle: Tween::new(0.0),
        }
    }
}
const ID_NAV: i32 = 400;
const ID_THEME_CHOICE: i32 = 410;

/// Posted to the island window after the config file was rewritten.
pub const WM_SETTINGS_APPLIED: u32 = WM_APP + 20;

/// Posted to the island window by a second `win-island --settings` launch.
pub const WM_OPEN_SETTINGS: u32 = WM_APP + 21;

const TBM_GETPOS_MSG: u32 = WM_USER;

const ID_OK: i32 = 1;
const ID_CANCEL: i32 = 2;
const ID_APPLY: i32 = 100;
const ID_THEME: i32 = 101;
const ID_ACCENT: i32 = 102;
const ID_ACCENT_PICK: i32 = 103;
const ID_BACKGROUND: i32 = 104;
const ID_BACKGROUND_PICK: i32 = 105;
const ID_OPACITY: i32 = 106;
const ID_OPACITY_LABEL: i32 = 107;
const ID_HOVER: i32 = 108;
const ID_CITY: i32 = 109;
const ID_SEARCH: i32 = 110;
const ID_AUTOSTART: i32 = 111;
const ID_OPEN_FILE: i32 = 112;
const ID_RESET: i32 = 113;
const ID_TILE_BASE: i32 = 200;
const ID_FLAG_BASE: i32 = 300;

const THEMES: [(&str, &str); 3] = [
    ("system", "Как в Windows"),
    ("dark", "Тёмная"),
    ("light", "Светлая"),
];

const TILES: [(&str, &str); 6] = [
    ("media", "Музыка"),
    ("clipboard", "Буфер обмена"),
    ("window", "Окно"),
    ("tools", "Инструменты"),
    ("weather", "Погода"),
    ("apps", "Приложения"),
];

type FlagGet = fn(&Config) -> bool;
type FlagSet = fn(&mut Config, bool);

const FLAGS: [(&str, FlagGet, FlagSet); 9] = [
    (
        "Прятать в полноэкранном режиме",
        |c| c.hide_in_fullscreen,
        |c, v| c.hide_in_fullscreen = v,
    ),
    (
        "Сжиматься над развёрнутыми окнами",
        |c| c.shrink_under_windows,
        |c, v| c.shrink_under_windows = v,
    ),
    (
        "Показывать активное приложение",
        |c| c.show_active_app,
        |c, v| c.show_active_app = v,
    ),
    (
        "Громкость, яркость, раскладка, Caps Lock",
        |c| c.osd,
        |c, v| c.osd = v,
    ),
    (
        "Индикатор камеры и микрофона",
        |c| c.privacy_indicator,
        |c, v| c.privacy_indicator = v,
    ),
    ("Поиск по Alt+Space", |c| c.launcher, |c, v| c.launcher = v),
    (
        "Край острова пульсирует в такт музыке",
        |c| c.beat_edge,
        |c, v| c.beat_edge = v,
    ),
    (
        "История буфера обмена",
        |c| c.clipboard_history,
        |c, v| c.clipboard_history = v,
    ),
    (
        "Рендер на видеокарте (+50 МБ памяти)",
        |c| c.gpu,
        |c, v| c.gpu = v,
    ),
];

struct State {
    hwnd: HWND,
    owner: HWND,
    fonts: [HFONT; 4],
    brush: HBRUSH,
    images: [HBITMAP; 2],
    page: usize,
    controls: Vec<(HWND, usize)>,
    status: String,
    dpi: u32,
    motions: HashMap<i32, ControlMotion>,
    epoch: Instant,
    animate: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Opens the window, or brings the open one to the front.
pub fn open(owner: HWND) {
    if let Some(existing) = STATE.with(|s| s.borrow().as_ref().map(|s| s.hwnd)) {
        unsafe {
            let _ = ShowWindow(existing, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(existing);
        }
        return;
    }
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else {
            return;
        };
        let instance: HINSTANCE = module.into();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH::default(),
            lpszClassName: w!("WinIslandSettings"),
            hIcon: crate::brand::icon(),
            ..Default::default()
        };
        RegisterClassExW(&class);
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);

        let Ok(hwnd) = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            w!("WinIslandSettings"),
            w!("Tidemark — настройки"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            100,
            100,
            None,
            None,
            Some(instance),
            None,
        ) else {
            let _ = std::fs::write(
                std::env::temp_dir().join("settings-err.txt"),
                format!("{:?}", windows::core::Error::from_win32()),
            );
            return;
        };

        let dpi = GetDpiForWindow(hwnd).max(96);
        let fonts = [
            skin::font(15, 400, dpi),
            skin::font(12, 400, dpi),
            skin::font(23, 600, dpi),
            skin::font(30, 600, dpi),
        ];
        let font = fonts[0];
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                hwnd,
                owner,
                fonts,
                brush: CreateSolidBrush(skin::color(skin::BG)),
                images: [skin::load_bitmap(201), skin::load_bitmap(202)],
                page: 0,
                controls: Vec::new(),
                status: "Изменения применяются кнопкой «Применить»".into(),
                dpi,
                motions: HashMap::new(),
                epoch: Instant::now(),
                animate: animations_enabled(),
            })
        });
        build(hwnd, dpi, font);
        fill(hwnd, &Config::load());
        select_page(hwnd, 0);

        let (cw, ch) = (px(CLIENT_W, dpi), px(CLIENT_H, dpi));
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: cw,
            bottom: ch,
        };
        let _ = AdjustWindowRectExForDpi(
            &mut rc,
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            false,
            WS_EX_CONTROLPARENT,
            dpi,
        );
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        let x = (GetSystemMetrics(SM_CXSCREEN) - w) / 2;
        let y = ((GetSystemMetrics(SM_CYSCREEN) - h) / 2).max(0);
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER);
        if animations_enabled() {
            let _ = AnimateWindow(hwnd, 160, AW_BLEND);
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Lets Tab/Enter/Esc work inside the window; call for every message.
pub fn is_dialog_message(msg: &MSG) -> bool {
    let hwnd = STATE.with(|s| s.borrow().as_ref().map(|s| s.hwnd));
    match hwnd {
        Some(h) => unsafe { IsDialogMessageW(h, msg).as_bool() },
        None => false,
    }
}

const CLIENT_W: i32 = 1000;
const CLIENT_H: i32 = 688;

fn px(v: i32, dpi: u32) -> i32 {
    (v as i64 * dpi as i64 / 96) as i32
}

struct Builder {
    parent: HWND,
    instance: HINSTANCE,
    dpi: u32,
    font: HFONT,
    page: usize,
}

impl Builder {
    fn add(
        &self,
        class: PCWSTR,
        text: &str,
        style: WINDOW_STYLE,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        id: i32,
    ) -> HWND {
        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                &HSTRING::from(text),
                WS_CHILD | WS_VISIBLE | style,
                px(x, self.dpi),
                px(y, self.dpi),
                px(w, self.dpi),
                px(h, self.dpi),
                Some(self.parent),
                Some(HMENU(id as usize as *mut _)),
                Some(self.instance),
                None,
            )
            .unwrap_or_default();
            SendMessageW(
                hwnd,
                WM_SETFONT,
                Some(WPARAM(self.font.0 as usize)),
                Some(LPARAM(1)),
            );
            let _ = SetWindowSubclass(hwnd, Some(control_proc), 2, 0);
            STATE.with(|s| {
                if let Some(s) = s.borrow_mut().as_mut() {
                    s.controls.push((hwnd, self.page));
                }
            });
            hwnd
        }
    }

    fn check(&self, text: &str, x: i32, y: i32, w: i32, id: i32) {
        let h = self.add(
            w!("BUTTON"),
            text,
            WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
            x,
            y,
            w,
            32,
            id,
        );
        unsafe {
            let _ = SetWindowSubclass(h, Some(toggle_proc), 1, 0);
        }
    }

    fn edit(&self, x: i32, y: i32, w: i32, id: i32, number: bool) {
        let style = WS_TABSTOP
            | WINDOW_STYLE(ES_AUTOHSCROLL as u32 | if number { ES_NUMBER as u32 } else { 0 });
        let hwnd = self.add(w!("EDIT"), "", style, x, y, w, 32, id);
        unsafe {
            SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & !(WS_EX_CLIENTEDGE.0 as isize),
            );
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
        }
    }

    fn button(&self, text: &str, x: i32, y: i32, w: i32, id: i32, _default: bool) {
        let style = WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32);
        self.add(w!("BUTTON"), text, style, x, y, w, 36, id);
    }
}

unsafe fn build(hwnd: HWND, dpi: u32, font: HFONT) {
    let instance: HINSTANCE = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
    let mut b = Builder {
        parent: hwnd,
        instance,
        dpi,
        font,
        page: usize::MAX,
    };
    for (i, name) in skin::TITLES.iter().enumerate() {
        b.button(name, 16, 156 + i as i32 * 44, 200, ID_NAV + i as i32, false);
    }
    b.button("Закрыть", 712, 640, 104, ID_CANCEL, false);
    b.button("Применить", 828, 640, 140, ID_APPLY, true);
    b.page = 0;
    // A hidden standard combo remains the single source of the theme value.
    let theme = b.add(
        w!("COMBOBOX"),
        "",
        WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
        700,
        352,
        268,
        180,
        ID_THEME,
    );
    for (_, name) in THEMES {
        SendMessageW(
            theme,
            CB_ADDSTRING,
            None,
            Some(LPARAM(HSTRING::from(name).as_ptr() as isize)),
        );
    }
    for (i, name) in ["Windows", "Тёмная", "Светлая"].iter().enumerate() {
        b.button(
            name,
            700 + i as i32 * 92,
            352,
            84,
            ID_THEME_CHOICE + i as i32,
            false,
        );
    }
    b.edit(700, 412, 176, ID_ACCENT, false);
    b.button("Выбрать", 888, 410, 80, ID_ACCENT_PICK, false);
    b.edit(700, 464, 176, ID_BACKGROUND, false);
    b.button("Выбрать", 888, 462, 80, ID_BACKGROUND_PICK, false);
    let slider = b.add(
        w!("msctls_trackbar32"),
        "",
        WS_TABSTOP | WINDOW_STYLE(TBS_HORZ | TBS_NOTICKS),
        696,
        516,
        208,
        32,
        ID_OPACITY,
    );
    let _ = SetWindowSubclass(slider, Some(toggle_proc), 1, 0);
    SendMessageW(
        slider,
        TBM_SETRANGE,
        Some(WPARAM(1)),
        Some(LPARAM((30 | (100 << 16)) as isize)),
    );
    b.add(
        w!("STATIC"),
        "100%",
        WINDOW_STYLE(0),
        916,
        520,
        52,
        24,
        ID_OPACITY_LABEL,
    );
    b.edit(864, 568, 104, ID_HOVER, true);
    b.page = 1;
    for (i, (_, name)) in TILES.iter().enumerate() {
        b.check(name, 912, 136 + i as i32 * 76, 56, ID_TILE_BASE + i as i32);
    }
    b.page = 2;
    for (i, (name, _, _)) in FLAGS.iter().enumerate() {
        b.check(name, 912, 124 + i as i32 * 54, 56, ID_FLAG_BASE + i as i32);
    }
    b.page = 3;
    b.check("Запускать вместе с Windows", 912, 140, 56, ID_AUTOSTART);
    b.edit(264, 296, 704, ID_CITY, false);
    b.edit(264, 416, 704, ID_SEARCH, false);
    b.button("Открыть config.toml", 264, 564, 220, ID_OPEN_FILE, false);
    b.button("Сбросить настройки", 500, 564, 220, ID_RESET, false);
    cue(hwnd, ID_BACKGROUND, "По теме");
    cue(hwnd, ID_CITY, "Например, Москва");
    cue(hwnd, ID_SEARCH, "https://www.google.com/search?q={q}");
}

unsafe fn select_page(hwnd: HWND, page: usize) {
    let controls = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let s = s.as_mut().unwrap();
        s.page = page;
        s.controls.clone()
    });
    for (h, p) in controls {
        let visible = (p == page || p == usize::MAX) && GetDlgCtrlID(h) != ID_THEME;
        let _ = ShowWindow(h, if visible { SW_SHOW } else { SW_HIDE });
        let _ = InvalidateRect(Some(h), None, false);
    }
    let _ = InvalidateRect(Some(hwnd), None, false);
}

unsafe fn repaint(hwnd: HWND) {
    let _ = InvalidateRect(Some(hwnd), None, false);
    let controls = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|s| s.controls.clone())
            .unwrap_or_default()
    });
    for (h, _) in controls {
        let _ = InvalidateRect(Some(h), None, false);
    }
}

unsafe fn cue(hwnd: HWND, id: i32, hint: &str) {
    SendMessageW(
        item(hwnd, id),
        EM_SETCUEBANNER,
        Some(WPARAM(0)),
        Some(LPARAM(HSTRING::from(hint).as_ptr() as isize)),
    );
}

unsafe fn item(hwnd: HWND, id: i32) -> HWND {
    GetDlgItem(Some(hwnd), id).unwrap_or_default()
}

unsafe fn text(hwnd: HWND, id: i32) -> String {
    let h = item(hwnd, id);
    let len = GetWindowTextLengthW(h).max(0) as usize;
    let mut buf = vec![0u16; len + 1];
    let n = GetWindowTextW(h, &mut buf).max(0) as usize;
    String::from_utf16_lossy(&buf[..n]).trim().to_string()
}

unsafe fn set_text(hwnd: HWND, id: i32, s: &str) {
    let _ = SetWindowTextW(item(hwnd, id), &HSTRING::from(s));
}

unsafe fn checked(hwnd: HWND, id: i32) -> bool {
    SendMessageW(item(hwnd, id), BM_GETCHECK, None, None).0 == BST_CHECKED.0 as isize
}

unsafe fn set_checked(hwnd: HWND, id: i32, on: bool) {
    STATE.with(|s| {
        if let Some(s) = s.borrow_mut().as_mut() {
            s.motions.entry(id).or_default().toggle = Tween::new(if on { 1.0 } else { 0.0 });
        }
    });
    SendMessageW(
        item(hwnd, id),
        BM_SETCHECK,
        Some(WPARAM(
            if on { BST_CHECKED.0 } else { BST_UNCHECKED.0 } as usize
        )),
        None,
    );
    let _ = InvalidateRect(Some(item(hwnd, id)), None, false);
}

unsafe fn set_opacity(hwnd: HWND, percent: i32) {
    let p = percent.clamp(30, 100);
    SendMessageW(
        item(hwnd, ID_OPACITY),
        TBM_SETPOS,
        Some(WPARAM(1)),
        Some(LPARAM(p as isize)),
    );
    set_text(hwnd, ID_OPACITY_LABEL, &format!("{p}%"));
}

/// Puts a config into the controls. Autostart is read from the registry.
unsafe fn fill(hwnd: HWND, cfg: &Config) {
    let theme = THEMES
        .iter()
        .position(|(k, _)| *k == cfg.theme)
        .unwrap_or(0);
    SendMessageW(
        item(hwnd, ID_THEME),
        CB_SETCURSEL,
        Some(WPARAM(theme)),
        None,
    );
    set_text(hwnd, ID_ACCENT, &cfg.accent);
    set_text(hwnd, ID_BACKGROUND, &cfg.background);
    set_opacity(hwnd, (cfg.opacity * 100.0).round() as i32);
    set_text(hwnd, ID_HOVER, &cfg.hover_delay_ms.to_string());
    for (i, (id, _)) in TILES.iter().enumerate() {
        set_checked(
            hwnd,
            ID_TILE_BASE + i as i32,
            cfg.home_tiles.iter().any(|t| t == id),
        );
    }
    for (i, (_, get, _)) in FLAGS.iter().enumerate() {
        set_checked(hwnd, ID_FLAG_BASE + i as i32, get(cfg));
    }
    set_text(hwnd, ID_CITY, &cfg.weather_city);
    set_text(hwnd, ID_SEARCH, &cfg.search_url);
    set_checked(hwnd, ID_AUTOSTART, autostart::is_enabled());
}

fn warn(hwnd: HWND, msg: &str) {
    unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(msg),
            w!("Tidemark"),
            MB_OK | MB_ICONWARNING,
        );
    }
}

/// Reads the controls over the file's current contents, so settings that
/// have no control here (sizes, dock apps) are kept.
unsafe fn collect(hwnd: HWND) -> Option<Config> {
    let mut cfg = Config::load();

    let sel = SendMessageW(item(hwnd, ID_THEME), CB_GETCURSEL, None, None).0;
    cfg.theme = THEMES
        .get(sel.max(0) as usize)
        .map_or("system", |t| t.0)
        .into();

    let accent = text(hwnd, ID_ACCENT);
    if parse_hex(&accent).is_none() {
        warn(
            hwnd,
            "Акцентный цвет должен быть в виде #RRGGBB, например #7C6CF2.",
        );
        return None;
    }
    cfg.accent = format!("#{}", accent.trim_start_matches('#').to_uppercase());
    let bg = text(hwnd, ID_BACKGROUND);
    if !bg.is_empty() && parse_hex(&bg).is_none() {
        warn(hwnd, "Фон должен быть в виде #RRGGBB или пустым.");
        return None;
    }
    cfg.background = if bg.is_empty() {
        bg
    } else {
        format!("#{}", bg.trim_start_matches('#').to_uppercase())
    };

    let pos = SendMessageW(item(hwnd, ID_OPACITY), TBM_GETPOS_MSG, None, None).0 as i32;
    cfg.opacity = pos.clamp(30, 100) as f32 / 100.0;
    cfg.hover_delay_ms = text(hwnd, ID_HOVER)
        .parse::<u32>()
        .unwrap_or(cfg.hover_delay_ms)
        .min(5000);

    // Keep the order of tiles that stay; new ones go to the end.
    let wanted: Vec<&str> = TILES
        .iter()
        .enumerate()
        .filter(|(i, _)| checked(hwnd, ID_TILE_BASE + *i as i32))
        .map(|(_, t)| t.0)
        .collect();
    let mut tiles: Vec<String> = cfg
        .home_tiles
        .iter()
        .filter(|t| wanted.contains(&t.as_str()))
        .cloned()
        .collect();
    for t in wanted {
        if !tiles.iter().any(|x| x == t) {
            tiles.push(t.into());
        }
    }
    cfg.home_tiles = tiles;

    for (i, (_, _, set)) in FLAGS.iter().enumerate() {
        set(&mut cfg, checked(hwnd, ID_FLAG_BASE + i as i32));
    }
    cfg.weather_city = text(hwnd, ID_CITY);
    let search = text(hwnd, ID_SEARCH);
    cfg.search_url = if search.is_empty() {
        Config::default().search_url
    } else {
        search
    };
    Some(cfg)
}

unsafe fn apply(hwnd: HWND) -> bool {
    let Some(cfg) = collect(hwnd) else {
        return false;
    };
    let result = toml::to_string(&cfg)
        .map_err(|e| e.to_string())
        .and_then(|data| {
            std::fs::create_dir_all(Config::dir())
                .and_then(|_| std::fs::write(Config::path(), data))
                .map_err(|e| e.to_string())
        });
    if let Err(error) = result {
        warn(hwnd, &format!("Не удалось сохранить настройки: {error}"));
        return false;
    }
    let want = checked(hwnd, ID_AUTOSTART);
    if want != autostart::is_enabled() {
        autostart::set_enabled(want);
    }
    if let Some(owner) = STATE.with(|s| s.borrow().as_ref().map(|s| s.owner)) {
        let _ = PostMessageW(Some(owner), WM_SETTINGS_APPLIED, WPARAM(0), LPARAM(0));
    }
    STATE.with(|s| {
        if let Some(s) = s.borrow_mut().as_mut() {
            s.status = "Настройки применены".into();
        }
    });
    repaint(hwnd);
    true
}

unsafe fn pick_color(hwnd: HWND, id: i32) {
    let current = parse_hex(&text(hwnd, id)).unwrap_or([0.486, 0.424, 0.949, 1.0]);
    let to_u8 = |v: f32| (v * 255.0).round() as u32;
    let mut custom = [COLORREF(0x00FF_FFFF); 16];
    let mut cc = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: hwnd,
        rgbResult: COLORREF(
            to_u8(current[0]) | (to_u8(current[1]) << 8) | (to_u8(current[2]) << 16),
        ),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_RGBINIT | CC_FULLOPEN,
        ..Default::default()
    };
    if ChooseColorW(&mut cc).as_bool() {
        let c = cc.rgbResult.0;
        set_text(
            hwnd,
            id,
            &format!(
                "#{:02X}{:02X}{:02X}",
                c & 0xFF,
                (c >> 8) & 0xFF,
                (c >> 16) & 0xFF
            ),
        );
    }
}

unsafe fn open_file() {
    let path = Config::path();
    if !path.exists() {
        Config::default().save();
    }
    ShellExecuteW(
        None,
        w!("open"),
        w!("notepad.exe"),
        &HSTRING::from(path.as_os_str()),
        None,
        SW_SHOWNORMAL,
    );
}

unsafe fn change_dpi(hwnd: HWND, dpi: u32, suggested: &RECT) {
    let fonts = [
        skin::font(15, 400, dpi),
        skin::font(12, 400, dpi),
        skin::font(23, 600, dpi),
        skin::font(30, 600, dpi),
    ];
    let previous = STATE.with(|s| {
        let mut state = s.borrow_mut();
        let s = state.as_mut()?;
        let previous = (s.dpi, s.fonts, s.controls.clone());
        s.dpi = dpi;
        s.fonts = fonts;
        Some(previous)
    });
    if let Some((old_dpi, old_fonts, controls)) = previous {
        for (h, _) in controls {
            let mut rc = RECT::default();
            let _ = GetWindowRect(h, &mut rc);
            let mut pt = POINT {
                x: rc.left,
                y: rc.top,
            };
            let _ = ScreenToClient(hwnd, &mut pt);
            let n = |x: i32| (x as i64 * dpi as i64 / old_dpi as i64) as i32;
            let _ = SetWindowPos(
                h,
                None,
                n(pt.x),
                n(pt.y),
                n(rc.right - rc.left),
                n(rc.bottom - rc.top),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            SendMessageW(
                h,
                WM_SETFONT,
                Some(WPARAM(fonts[0].0 as usize)),
                Some(LPARAM(1)),
            );
        }
        for font in old_fonts {
            let _ = DeleteObject(font.into());
        }
    }
    let _ = SetWindowPos(
        hwnd,
        None,
        suggested.left,
        suggested.top,
        suggested.right - suggested.left,
        suggested.bottom - suggested.top,
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
    repaint(hwnd);
}

unsafe fn paint_window(hwnd: HWND, target: Option<HDC>) {
    let mut ps = PAINTSTRUCT::default();
    let dc = target.unwrap_or_else(|| BeginPaint(hwnd, &mut ps));
    let state = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|s| (s.page, s.fonts, s.images, s.status.clone()))
    });
    if let Some((page, fonts, images, status)) = state {
        let theme = SendMessageW(item(hwnd, ID_THEME), CB_GETCURSEL, None, None).0;
        let light = match theme {
            2 => true,
            1 => false,
            _ => crate::theme::Theme::resolve(&Config::default()).light,
        };
        let rgb = |v: [f32; 4]| {
            ((v[0] * 255.0) as u32) << 16 | ((v[1] * 255.0) as u32) << 8 | (v[2] * 255.0) as u32
        };
        let accent = parse_hex(&text(hwnd, ID_ACCENT))
            .map(rgb)
            .unwrap_or(skin::ACCENT);
        let bg = parse_hex(&text(hwnd, ID_BACKGROUND))
            .map(rgb)
            .unwrap_or(if light { 0xF5F5F3 } else { 0x202423 });
        let opacity = SendMessageW(item(hwnd, ID_OPACITY), TBM_GETPOS_MSG, None, None).0 as i32;
        let c = skin::Canvas {
            dc,
            dpi: GetDpiForWindow(hwnd).max(96),
            fonts,
        };
        skin::paint(
            &c,
            page,
            images[if light { 0 } else { 1 }],
            light,
            accent,
            bg,
            opacity,
            &status,
        );
    }
    if target.is_none() {
        let _ = EndPaint(hwnd, &ps);
    }
}

fn is_switch(id: i32) -> bool {
    id == ID_AUTOSTART
        || (ID_TILE_BASE..ID_TILE_BASE + 6).contains(&id)
        || (ID_FLAG_BASE..ID_FLAG_BASE + 9).contains(&id)
}
fn is_button(id: i32) -> bool {
    is_switch(id)
        || (ID_NAV..ID_NAV + 4).contains(&id)
        || (ID_THEME_CHOICE..ID_THEME_CHOICE + 3).contains(&id)
        || matches!(
            id,
            ID_OK
                | ID_CANCEL
                | ID_APPLY
                | ID_ACCENT_PICK
                | ID_BACKGROUND_PICK
                | ID_OPEN_FILE
                | ID_RESET
        )
}
unsafe fn animations_enabled() -> bool {
    let mut enabled = BOOL(1);
    let _ = SystemParametersInfoW(
        SPI_GETCLIENTAREAANIMATION,
        0,
        Some(&mut enabled as *mut _ as *mut _),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    enabled.as_bool()
}
unsafe fn retarget(hwnd: HWND, id: i32, hover: Option<bool>, toggle: Option<bool>) {
    let active = STATE.with(|s| {
        let mut state = s.borrow_mut();
        let Some(s) = state.as_mut() else {
            return false;
        };
        let now = s.epoch.elapsed().as_secs_f32();
        let duration = if s.animate { 0.16 } else { 0.0 };
        let m = s.motions.entry(id).or_default();
        if let Some(v) = hover {
            m.hover.target(if v { 1.0 } else { 0.0 }, now, duration);
        }
        if let Some(v) = toggle {
            m.toggle.target(if v { 1.0 } else { 0.0 }, now, duration);
        }
        m.hover.running(now) || m.toggle.running(now)
    });
    if active {
        let _ = SetTimer(Some(hwnd), TIMER_MOTION, 16, None);
    }
    let _ = InvalidateRect(Some(item(hwnd, id)), None, false);
}

unsafe extern "system" fn control_proc(
    h: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let id = GetDlgCtrlID(h);
    if is_button(id) || id == ID_OPACITY {
        match msg {
            WM_SETCURSOR if (lp.0 as u32 & 0xFFFF) == HTCLIENT => {
                if IsWindowEnabled(h).as_bool() {
                    SetCursor(LoadCursorW(None, IDC_HAND).ok());
                    return LRESULT(1);
                }
            }
            WM_MOUSEMOVE => {
                let changed = STATE.with(|s| {
                    s.borrow()
                        .as_ref()
                        .and_then(|s| s.motions.get(&id))
                        .is_none_or(|m| m.hover.value(f32::MAX) == 0.0)
                });
                if changed {
                    let mut tracking = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: h,
                        ..Default::default()
                    };
                    let _ = TrackMouseEvent(&mut tracking);
                    retarget(GetParent(h).unwrap_or_default(), id, Some(true), None);
                }
            }
            WM_MOUSELEAVE => retarget(GetParent(h).unwrap_or_default(), id, Some(false), None),
            _ => {}
        }
    }
    DefSubclassProc(h, msg, wp, lp)
}

// Keep native checkbox keyboard behavior and accessibility state.
unsafe extern "system" fn toggle_proc(
    h: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_ERASEBKGND {
        return LRESULT(1);
    }
    if msg == WM_PAINT || msg == WM_PRINTCLIENT {
        let mut ps = PAINTSTRUCT::default();
        let dc = if msg == WM_PAINT {
            BeginPaint(h, &mut ps)
        } else {
            HDC(wp.0 as *mut _)
        };
        let mut rc = RECT::default();
        let _ = GetClientRect(h, &mut rc);
        let d = DRAWITEMSTRUCT {
            CtlID: GetDlgCtrlID(h) as u32,
            hwndItem: h,
            hDC: dc,
            rcItem: rc,
            itemState: if windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() == h {
                ODS_FOCUS
            } else {
                ODS_FLAGS(0)
            },
            ..Default::default()
        };
        draw_control(GetParent(h).unwrap_or_default(), &d);
        if msg == WM_PAINT {
            let _ = EndPaint(h, &ps);
        }
        return LRESULT(0);
    }
    DefSubclassProc(h, msg, wp, lp)
}

unsafe fn draw_control(hwnd: HWND, d: &DRAWITEMSTRUCT) {
    let Some((fonts, page)) = STATE.with(|s| s.borrow().as_ref().map(|s| (s.fonts, s.page))) else {
        return;
    };
    let id = d.CtlID as i32;
    let (hover, toggle) = STATE.with(|s| {
        let state = s.borrow();
        let s = state.as_ref().unwrap();
        let now = s.epoch.elapsed().as_secs_f32();
        s.motions
            .get(&id)
            .map(|m| (m.hover.value(now), m.toggle.value(now)))
            .unwrap_or((0.0, 0.0))
    });
    let dpi = GetDpiForWindow(hwnd).max(96);
    let width = (d.rcItem.right - d.rcItem.left) * 96 / dpi as i32;
    let height = (d.rcItem.bottom - d.rcItem.top) * 96 / dpi as i32;
    let saved = SaveDC(d.hDC);
    let _ = SetViewportOrgEx(d.hDC, d.rcItem.left, d.rcItem.top, None);
    let c = skin::Canvas {
        dc: d.hDC,
        dpi,
        fonts,
    };
    let nav = (ID_NAV..ID_NAV + 4).contains(&id);
    c.fill(0, 0, width, height, if nav { skin::SIDE } else { skin::BG });
    let pressed = d.itemState.0 & ODS_SELECTED.0 != 0;
    let focus = d.itemState.0 & ODS_FOCUS.0 != 0;
    if id == ID_OPACITY {
        let pos = SendMessageW(d.hwndItem, TBM_GETPOS_MSG, None, None).0 as i32;
        let x = 12 + (width - 24) * (pos.clamp(30, 100) - 30) / 70;
        c.round(12, 14, width - 24, 4, 2, 0xCDD1CB);
        c.round(12, 14, (x - 12).max(1), 4, 2, skin::ACCENT);
        c.round(x - 7, 9, 14, 14, 7, skin::ACCENT);
    } else if id == ID_AUTOSTART
        || (ID_TILE_BASE..ID_TILE_BASE + 6).contains(&id)
        || (ID_FLAG_BASE..ID_FLAG_BASE + 9).contains(&id)
    {
        c.round(4, 4, 44, 24, 12, skin::mix(0xC6CBC6, skin::ACCENT, toggle));
        c.round((8.0 + 20.0 * toggle).round() as i32, 8, 16, 16, 8, 0xFFFFFF);
    } else {
        let choice = (ID_THEME_CHOICE..ID_THEME_CHOICE + 3).contains(&id);
        let theme = SendMessageW(item(hwnd, ID_THEME), CB_GETCURSEL, None, None).0;
        let selected = (nav && page as i32 == id - ID_NAV)
            || (choice && theme == (id - ID_THEME_CHOICE) as isize);
        let primary = id == ID_APPLY;
        let fill = if primary {
            if pressed {
                0x853A28
            } else {
                skin::ACCENT
            }
        } else if selected || pressed {
            skin::SELECT
        } else if nav {
            skin::SIDE
        } else {
            0xECEDE8
        };
        let hovered = if primary { 0x853A28 } else { skin::SELECT };
        c.round(0, 0, width, height, 8, skin::mix(fill, hovered, hover));
        if nav && selected {
            c.round(0, 10, 3, height - 20, 1, skin::ACCENT);
        }
        let label = skin::caption(d.hwndItem);
        if nav {
            c.text(&label, 16, 0, width - 24, height, 0, skin::INK);
        } else {
            let old = SelectObject(d.hDC, fonts[if choice { 1 } else { 0 }].into());
            SetTextColor(
                d.hDC,
                skin::color(if primary { 0xFFFFFF } else { skin::INK }),
            );
            SetBkMode(d.hDC, TRANSPARENT);
            let mut buf: Vec<u16> = label.encode_utf16().collect();
            let mut rc = c.rect(4, 0, width - 8, height);
            DrawTextW(
                d.hDC,
                &mut buf,
                &mut rc,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            SelectObject(d.hDC, old);
        }
    }
    if focus {
        let rc = c.rect(2, 2, width - 4, height - 4);
        let _ = DrawFocusRect(d.hDC, &rc);
    }
    let _ = RestoreDC(d.hDC, saved);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wparam.0 == TIMER_MOTION => {
            let (active, controls) = STATE.with(|s| {
                let state = s.borrow();
                let Some(s) = state.as_ref() else {
                    return (false, Vec::new());
                };
                let now = s.epoch.elapsed().as_secs_f32();
                (
                    s.motions
                        .values()
                        .any(|m| m.hover.running(now) || m.toggle.running(now)),
                    s.controls.clone(),
                )
            });
            for (h, _) in controls {
                let _ = InvalidateRect(Some(h), None, false);
            }
            if !active {
                let _ = KillTimer(Some(hwnd), TIMER_MOTION);
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            let enabled = animations_enabled();
            STATE.with(|s| {
                if let Some(s) = s.borrow_mut().as_mut() {
                    s.animate = enabled;
                    if !enabled {
                        for m in s.motions.values_mut() {
                            m.hover = Tween::new(m.hover.value(f32::MAX));
                            m.toggle = Tween::new(m.toggle.value(f32::MAX));
                        }
                    }
                }
            });
            if !enabled {
                let _ = KillTimer(Some(hwnd), TIMER_MOTION);
                repaint(hwnd);
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            change_dpi(
                hwnd,
                (wparam.0 & 0xFFFF) as u32,
                &*(lparam.0 as *const RECT),
            );
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint_window(hwnd, None);
            LRESULT(0)
        }
        WM_PRINTCLIENT => {
            paint_window(hwnd, Some(HDC(wparam.0 as *mut _)));
            LRESULT(0)
        }
        WM_DRAWITEM => {
            draw_control(hwnd, &*(lparam.0 as *const DRAWITEMSTRUCT));
            LRESULT(1)
        }
        WM_CTLCOLOREDIT => {
            let dc = HDC(wparam.0 as *mut _);
            SetTextColor(dc, skin::color(skin::INK));
            SetBkColor(dc, skin::color(0xFFFFFF));
            LRESULT(GetStockObject(WHITE_BRUSH).0 as isize)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let dc = HDC(wparam.0 as *mut _);
            SetTextColor(dc, skin::color(skin::INK));
            SetBkColor(dc, skin::color(skin::BG));
            let brush = STATE.with(|s| s.borrow().as_ref().map(|s| s.brush).unwrap_or_default());
            LRESULT(brush.0 as isize)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if code == BN_CLICKED {
                if is_switch(id) {
                    retarget(hwnd, id, None, Some(checked(hwnd, id)));
                }
                match id {
                    ID_OK => {
                        if apply(hwnd) {
                            let _ = DestroyWindow(hwnd);
                        }
                    }
                    ID_CANCEL => {
                        let _ = DestroyWindow(hwnd);
                    }
                    ID_APPLY => {
                        apply(hwnd);
                    }
                    ID_ACCENT_PICK => pick_color(hwnd, ID_ACCENT),
                    ID_BACKGROUND_PICK => pick_color(hwnd, ID_BACKGROUND),
                    ID_OPEN_FILE => open_file(),
                    ID_RESET => {
                        let was = checked(hwnd, ID_AUTOSTART);
                        fill(hwnd, &Config::default());
                        set_checked(hwnd, ID_AUTOSTART, was);
                    }
                    id if (ID_NAV..ID_NAV + 4).contains(&id) => {
                        select_page(hwnd, (id - ID_NAV) as usize)
                    }
                    id if (ID_THEME_CHOICE..ID_THEME_CHOICE + 3).contains(&id) => {
                        SendMessageW(
                            item(hwnd, ID_THEME),
                            CB_SETCURSEL,
                            Some(WPARAM((id - ID_THEME_CHOICE) as usize)),
                            None,
                        );
                        repaint(hwnd);
                    }
                    _ => {}
                }
            }
            if code == EN_CHANGE {
                repaint(hwnd);
            }
            LRESULT(0)
        }
        WM_HSCROLL => {
            let pos = SendMessageW(item(hwnd, ID_OPACITY), TBM_GETPOS_MSG, None, None).0 as i32;
            set_text(hwnd, ID_OPACITY_LABEL, &format!("{pos}%"));
            repaint(hwnd);
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(Some(hwnd), TIMER_MOTION);
            STATE.with(|s| {
                if let Some(state) = s.borrow_mut().take() {
                    for font in state.fonts {
                        let _ = DeleteObject(font.into());
                    }
                    for image in state.images {
                        let _ = DeleteObject(image.into());
                    }
                    let _ = DeleteObject(state.brush.into());
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_settings_button_exposes_a_hand_cursor() {
        unsafe {
            let instance = GetModuleHandleW(None).ok().map(Into::into);
            let parent = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("cursor-test"),
                WS_OVERLAPPED,
                0,
                0,
                100,
                100,
                None,
                None,
                instance,
                None,
            )
            .unwrap();
            let previous = GetCursor();
            let mut ids = vec![
                ID_CANCEL,
                ID_APPLY,
                ID_ACCENT_PICK,
                ID_BACKGROUND_PICK,
                ID_OPEN_FILE,
                ID_RESET,
                ID_AUTOSTART,
            ];
            ids.extend(ID_NAV..ID_NAV + 4);
            ids.extend(ID_THEME_CHOICE..ID_THEME_CHOICE + 3);
            ids.extend(ID_TILE_BASE..ID_TILE_BASE + 6);
            ids.extend(ID_FLAG_BASE..ID_FLAG_BASE + 9);
            for id in ids {
                let child = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("BUTTON"),
                    w!("test"),
                    WS_CHILD,
                    0,
                    0,
                    40,
                    20,
                    Some(parent),
                    Some(HMENU(id as usize as *mut _)),
                    instance,
                    None,
                )
                .unwrap();
                assert!(SetWindowSubclass(child, Some(control_proc), 2, 0).as_bool());
                assert_eq!(
                    SendMessageW(
                        child,
                        WM_SETCURSOR,
                        Some(WPARAM(child.0 as usize)),
                        Some(LPARAM(HTCLIENT as isize))
                    )
                    .0,
                    1,
                    "button {id}"
                );
                assert_eq!(GetCursor(), LoadCursorW(None, IDC_HAND).unwrap());
                let _ = DestroyWindow(child);
            }
            SetCursor(Some(previous));
            let _ = DestroyWindow(parent);
        }
    }
}
