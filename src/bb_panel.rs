//! Native bb panel, on its own UI thread. No network calls in window procedures.
use super::*;
use crate::{
    bb_layout::{layout, Rect},
    settings::skin,
};
#[path = "bb_skin.rs"]
mod visual;
use std::{cell::RefCell, sync::atomic::AtomicBool};
use windows::Win32::{
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::EnableWindow, WindowsAndMessaging::*},
};
static OPEN: AtomicBool = AtomicBool::new(false);
const URL: i32 = 101;
const TOKEN: i32 = 102;
const SAVE: i32 = 103;
const OFF: i32 = 104;
const THREAD: i32 = 105;
const ITEM: i32 = 106;
const DETAIL: i32 = 107;
const OPTIONS: i32 = 108;
const TEXT: i32 = 109;
const SEND: i32 = 110;
const REFRESH: i32 = 111;
const STATUS: i32 = 112;
const DENY: i32 = 113;
const CONNECTION: i32 = 114;
thread_local! {static PANEL:RefCell<Option<Panel>>=const{RefCell::new(None)};}
#[derive(Clone, PartialEq)]
struct Render {
    dpi: u32,
    fonts: [HFONT; 4],
    brush: HBRUSH,
    width: i32,
    height: i32,
    connection: bool,
    meta: String,
    choices: Vec<Value>,
    multi: bool,
    hovered: i32,
}
thread_local! {static RENDER:RefCell<Option<Render>>=const{RefCell::new(None)};}
fn publish_render() {
    let snapshot = PANEL.with(|p| {
        let p = p.try_borrow().ok()?;
        let p = p.as_ref()?;
        let (width, height) = p.dimensions();
        let item = p.item();
        let q = item
            .as_ref()
            .map(|i| &i["payload"]["questions"][p.question]);
        Some((
            p.hwnd,
            Render {
                dpi: p.dpi,
                fonts: p.fonts,
                brush: p.brush,
                width,
                height,
                connection: p.connection_page,
                meta: p.meta.clone(),
                choices: q
                    .and_then(|q| q["options"].as_array())
                    .cloned()
                    .unwrap_or_default(),
                multi: q.is_some_and(|q| q["multiSelect"] == true),
                hovered: p.hovered,
            },
        ))
    });
    if let Some((hwnd, next)) = snapshot {
        let changed = RENDER.with(|r| {
            let mut r = r.borrow_mut();
            if r.as_ref() == Some(&next) {
                false
            } else {
                *r = Some(next);
                true
            }
        });
        if changed {
            unsafe {
                let _ = RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ALLCHILDREN);
            }
        }
    }
}
struct Panel {
    hwnd: HWND,
    data: Value,
    question: usize,
    answers: serde_json::Map<String, Value>,
    result: Arc<Mutex<Option<Result<Value, String>>>>,
    busy: bool,
    loaded: bool,
    selected_thread: usize,
    selected_item: usize,
    connection_page: bool,
    dpi: u32,
    fonts: [HFONT; 4],
    brush: HBRUSH,
    meta: String,
    hovered: i32,
}
impl Drop for Panel {
    fn drop(&mut self) {
        unsafe {
            for f in self.fonts {
                let _ = DeleteObject(f.into());
            }
            let _ = DeleteObject(self.brush.into());
        }
    }
}
fn child(hwnd: HWND, id: i32) -> HWND {
    unsafe { GetDlgItem(Some(hwnd), id).unwrap_or_default() }
}
fn text(hwnd: HWND, id: i32) -> String {
    unsafe {
        let ctl = child(hwnd, id);
        let n = GetWindowTextLengthW(ctl).max(0) as usize;
        let mut b = vec![0; n + 1];
        let n = GetWindowTextW(ctl, &mut b);
        String::from_utf16_lossy(&b[..n as usize])
    }
}
fn set(hwnd: HWND, id: i32, s: &str) {
    if text(hwnd, id) == s {
        return;
    }
    unsafe {
        let _ = SetWindowTextW(child(hwnd, id), &HSTRING::from(s));
    }
}
fn combo_index(hwnd: HWND, id: i32) -> usize {
    unsafe {
        send(child(hwnd, id), CB_GETCURSEL, WPARAM(0), LPARAM(0))
            .0
            .max(0) as usize
    }
}
fn combo_add(hwnd: HWND, id: i32, s: &str) {
    let w: Vec<u16> = s.encode_utf16().chain(Some(0)).collect();
    unsafe {
        send(
            child(hwnd, id),
            CB_ADDSTRING,
            WPARAM(0),
            LPARAM(w.as_ptr() as isize),
        );
    }
}
impl Panel {
    fn status(&self, s: &str) {
        set(self.hwnd, STATUS, s);
    }
    fn choice_count(&self) -> usize {
        self.item()
            .and_then(|i| {
                i["payload"]["questions"][self.question]["options"]
                    .as_array()
                    .map(Vec::len)
            })
            .unwrap_or(0)
    }
    fn dimensions(&self) -> (i32, i32) {
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut rc);
            (
                rc.right * 96 / self.dpi as i32,
                rc.bottom * 96 / self.dpi as i32,
            )
        }
    }
    fn layout(&self) {
        unsafe {
            let (width, height) = self.dimensions();
            let move_to = |id, r: Rect| {
                let _ = MoveWindow(
                    child(self.hwnd, id),
                    skin::scale(r.x, self.dpi),
                    skin::scale(r.y, self.dpi),
                    skin::scale(r.w, self.dpi),
                    skin::scale(r.h, self.dpi),
                    false,
                );
            };
            let show = |id, on| {
                let _ = ShowWindow(child(self.hwnd, id), if on { SW_SHOW } else { SW_HIDE });
            };
            move_to(
                CONNECTION,
                Rect {
                    x: width - 188,
                    y: 26,
                    w: 160,
                    h: 36,
                },
            );
            set(
                self.hwnd,
                CONNECTION,
                if self.connection_page {
                    "К вопросам"
                } else {
                    "Подключение"
                },
            );
            move_to(
                REFRESH,
                Rect {
                    x: width - 298,
                    y: 26,
                    w: 98,
                    h: 36,
                },
            );
            for id in [THREAD, ITEM, DETAIL, TEXT, SEND, OPTIONS, DENY, REFRESH] {
                show(id, !self.connection_page);
            }
            for id in [URL, TOKEN, SAVE, OFF] {
                show(id, self.connection_page);
            }
            if self.connection_page {
                move_to(
                    URL,
                    Rect {
                        x: 44,
                        y: 205,
                        w: width - 88,
                        h: 28,
                    },
                );
                move_to(
                    TOKEN,
                    Rect {
                        x: 44,
                        y: 327,
                        w: width - 88,
                        h: 28,
                    },
                );
                move_to(
                    SAVE,
                    Rect {
                        x: width - 248,
                        y: height - 64,
                        w: 220,
                        h: 42,
                    },
                );
                move_to(
                    OFF,
                    Rect {
                        x: width - 398,
                        y: height - 64,
                        w: 136,
                        h: 42,
                    },
                );
            } else {
                let half = (width - 80) / 2;
                move_to(
                    THREAD,
                    Rect {
                        x: 28,
                        y: 128,
                        w: half,
                        h: 300,
                    },
                );
                move_to(
                    ITEM,
                    Rect {
                        x: width / 2 + 12,
                        y: 128,
                        w: half,
                        h: 300,
                    },
                );
                send(
                    child(self.hwnd, THREAD),
                    CB_SETITEMHEIGHT,
                    WPARAM(usize::MAX),
                    LPARAM(skin::scale(26, self.dpi) as isize),
                );
                send(
                    child(self.hwnd, ITEM),
                    CB_SETITEMHEIGHT,
                    WPARAM(usize::MAX),
                    LPARAM(skin::scale(26, self.dpi) as isize),
                );
                let l = layout(width, height, self.choice_count());
                move_to(
                    DETAIL,
                    Rect {
                        x: l.question.x + 20,
                        y: l.question.y + 42,
                        w: l.question.w - 40,
                        h: l.question.h - 56,
                    },
                );
                move_to(OPTIONS, l.options);
                show(OPTIONS, self.choice_count() > 0);
                move_to(
                    TEXT,
                    Rect {
                        x: l.reply.x + 16,
                        y: l.reply.y + 14,
                        w: l.reply.w - 32,
                        h: l.reply.h - 28,
                    },
                );
                move_to(
                    SEND,
                    Rect {
                        x: width - 248,
                        y: l.footer + 16,
                        w: 220,
                        h: 42,
                    },
                );
                move_to(
                    DENY,
                    Rect {
                        x: width - 398,
                        y: l.footer + 16,
                        w: 136,
                        h: 42,
                    },
                );
                let approval = self.item().is_some_and(|i| {
                    i["payload"]["kind"] == "approval"
                        && i["payload"]["subject"]["kind"] != "permission_grant"
                });
                show(DENY, approval);
            }
            move_to(
                STATUS,
                Rect {
                    x: 28,
                    y: height - 63,
                    w: (width - 450).max(220),
                    h: 46,
                },
            );
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }
    fn thread(&self) -> Option<&Value> {
        self.data["threads"]
            .as_array()?
            .get(combo_index(self.hwnd, THREAD))
    }
    fn interactions(&self) -> Vec<Value> {
        let id = self.thread().and_then(|t| t["id"].as_str()).unwrap_or("");
        self.data["interactions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i["threadId"] == id)
            .cloned()
            .collect()
    }
    fn item(&self) -> Option<Value> {
        let n = combo_index(self.hwnd, ITEM);
        if n == 0 {
            None
        } else {
            self.interactions().get(n - 1).cloned()
        }
    }
    fn refresh(&mut self) {
        let snapshot = state().lock().unwrap().clone();
        self.data = snapshot.data;
        unsafe {
            send(
                child(self.hwnd, THREAD),
                CB_RESETCONTENT,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Some(threads) = self.data["threads"].as_array() {
            for t in threads {
                combo_add(
                    self.hwnd,
                    THREAD,
                    t["title"].as_str().unwrap_or("Без названия"),
                );
            }
        }
        let first = self.data["threads"]
            .as_array()
            .and_then(|threads| {
                threads.iter().position(|t| {
                    self.data["interactions"]
                        .as_array()
                        .is_some_and(|items| items.iter().any(|i| i["threadId"] == t["id"]))
                })
            })
            .unwrap_or(0);
        unsafe {
            send(
                child(self.hwnd, THREAD),
                CB_SETCURSEL,
                WPARAM(first),
                LPARAM(0),
            );
        }
        self.select_thread();
        self.loaded = self.data["threads"].is_array();
        self.status(if !snapshot.error.is_empty() {
            &snapshot.error
        } else if self.data["incomplete"] == true {
            "Показана часть тредов. Выберите проект в настройках плагина bb."
        } else {
            "Подключено к bb. Выберите тред и вопрос или дополните задачу."
        });
    }
    fn select_thread(&mut self) {
        self.selected_thread = combo_index(self.hwnd, THREAD);
        unsafe {
            send(
                child(self.hwnd, ITEM),
                CB_RESETCONTENT,
                WPARAM(0),
                LPARAM(0),
            );
        }
        combo_add(self.hwnd, ITEM, "Дополнить задачу");
        let items = self.interactions();
        for item in &items {
            let p = &item["payload"];
            let label = p["questions"][0]["prompt"]
                .as_str()
                .or_else(|| p["reason"].as_str())
                .unwrap_or("Запрос агента");
            combo_add(self.hwnd, ITEM, label);
        }
        unsafe {
            send(
                child(self.hwnd, ITEM),
                CB_SETCURSEL,
                WPARAM(if items.is_empty() { 0 } else { 1 }),
                LPARAM(0),
            );
        }
        self.select_item();
    }
    fn select_item(&mut self) {
        self.selected_item = combo_index(self.hwnd, ITEM);
        self.question = 0;
        self.answers.clear();
        set(self.hwnd, TEXT, "");
        self.render_question();
    }
    fn has_draft(&self) -> bool {
        !self.answers.is_empty()
            || !text(self.hwnd, TEXT).trim().is_empty()
            || unsafe {
                send(
                    child(self.hwnd, OPTIONS),
                    LB_GETSELCOUNT,
                    WPARAM(0),
                    LPARAM(0),
                )
                .0 > 0
            }
    }
    fn discard_allowed(&self) -> bool {
        !self.has_draft()
            || unsafe {
                MessageBoxW(
                    Some(self.hwnd),
                    w!("Удалить текущий черновик и продолжить?"),
                    w!("Tidemark · bb"),
                    MB_YESNO | MB_ICONQUESTION,
                ) == IDYES
            }
    }
    fn render_question(&mut self) {
        self.layout();
        self.meta = "Дополнение к задаче".into();
        unsafe {
            send(
                child(self.hwnd, OPTIONS),
                LB_RESETCONTENT,
                WPARAM(0),
                LPARAM(0),
            );
            let _ = EnableWindow(child(self.hwnd, TEXT), true);
            let _ = EnableWindow(child(self.hwnd, SEND), true);
            let _ = EnableWindow(child(self.hwnd, DENY), false);
            let Some(item) = self.item() else {
                set(self.hwnd,DETAIL,"Дополнительные инструкции для выбранного треда.\r\nСообщение попадёт в этот тред; если агент ждёт ответа, bb поставит его в очередь.");
                set(self.hwnd, SEND, "Отправить сообщение");
                return;
            };
            let p = &item["payload"];
            if p["kind"] == "user_question" {
                let q = &p["questions"][self.question];
                self.meta = format!(
                    "Вопрос {} из {}",
                    self.question + 1,
                    p["questions"].as_array().map_or(0, Vec::len)
                );
                let detail = q["prompt"].as_str().unwrap_or("").to_string();
                if let Some(options) = q["options"].as_array() {
                    for o in options {
                        let label = o["label"].as_str().unwrap_or("");
                        let accessible =
                            format!("{} {}", label, o["description"].as_str().unwrap_or(""));
                        let value: Vec<u16> = accessible.encode_utf16().chain(Some(0)).collect();
                        send(
                            child(self.hwnd, OPTIONS),
                            LB_ADDSTRING,
                            WPARAM(0),
                            LPARAM(value.as_ptr() as isize),
                        );
                        if self
                            .answers
                            .get(q["id"].as_str().unwrap_or(""))
                            .and_then(|a| a["selected"].as_array())
                            .is_some_and(|selected| selected.contains(&o["value"]))
                        {
                            let index =
                                send(child(self.hwnd, OPTIONS), LB_GETCOUNT, WPARAM(0), LPARAM(0))
                                    .0
                                    - 1;
                            send(
                                child(self.hwnd, OPTIONS),
                                LB_SETSEL,
                                WPARAM(1),
                                LPARAM(index),
                            );
                        }
                    }
                }
                set(
                    self.hwnd,
                    DETAIL,
                    &detail.replace('\n', "\r\n").replace("\r\r\n", "\r\n"),
                );
                let _ = EnableWindow(child(self.hwnd, TEXT), q["allowFreeText"] == true);
                let count = p["questions"].as_array().map_or(0, Vec::len);
                set(
                    self.hwnd,
                    SEND,
                    if self.question + 1 < count {
                        "Следующий вопрос"
                    } else {
                        "Отправить ответ"
                    },
                );
            } else if p["kind"] == "approval" && p["subject"]["kind"] != "permission_grant" {
                self.meta = "Нужно ваше разрешение".into();
                set(
                    self.hwnd,
                    DETAIL,
                    &serde_json::to_string_pretty(p)
                        .unwrap_or_default()
                        .replace('\n', "\r\n"),
                );
                let _ = EnableWindow(child(self.hwnd, TEXT), false);
                let decisions = p["availableDecisions"].as_array();
                let _ = EnableWindow(
                    child(self.hwnd, SEND),
                    decisions.is_some_and(|a| a.iter().any(|x| x == "allow_once")),
                );
                let _ = EnableWindow(
                    child(self.hwnd, DENY),
                    decisions.is_some_and(|a| a.iter().any(|x| x == "deny")),
                );
                set(self.hwnd, SEND, "Разрешить один раз");
            } else {
                self.meta = "Запрос агента".into();
                set(
                    self.hwnd,
                    DETAIL,
                    &format!(
                        "Этот тип запроса нужно открыть в bb.\r\n\r\n{}",
                        serde_json::to_string_pretty(p).unwrap_or_default()
                    ),
                );
                let _ = EnableWindow(child(self.hwnd, TEXT), false);
                let _ = EnableWindow(child(self.hwnd, SEND), false);
            }
        }
    }
    fn submit(&mut self, deny: bool) {
        if self.busy {
            return;
        }
        let c = connection();
        let Some(thread) = self.thread() else {
            self.status("Выберите тред");
            return;
        };
        let thread_id = thread["id"].clone();
        let item = self.item();
        let (route, body) = if let Some(item) = item {
            let p = &item["payload"];
            let response = if p["kind"] == "approval" {
                serde_json::json!({"decision":if deny{"deny"}else{"allow_once"}})
            } else {
                let q = &p["questions"][self.question];
                let mut selected = Vec::new();
                if let Some(options) = q["options"].as_array() {
                    for (i, o) in options.iter().enumerate() {
                        if unsafe {
                            send(child(self.hwnd, OPTIONS), LB_GETSEL, WPARAM(i), LPARAM(0)).0
                        } > 0
                        {
                            selected.push(o["value"].clone());
                        }
                    }
                }
                let free = text(self.hwnd, TEXT);
                if q["multiSelect"] != true && selected.len() > 1 {
                    self.status("Выберите только один вариант");
                    return;
                }
                if q["required"] == true && selected.is_empty() && free.trim().is_empty() {
                    self.status("Выберите вариант или напишите ответ");
                    return;
                }
                let mut a = serde_json::json!({"selected":selected});
                if q["allowFreeText"] == true && !free.trim().is_empty() {
                    a["freeText"] = free.into();
                }
                let Some(qid) = q["id"].as_str() else { return };
                self.answers.insert(qid.to_string(), a);
                if self.question + 1 < p["questions"].as_array().map_or(0, Vec::len) {
                    self.question += 1;
                    set(self.hwnd, TEXT, "");
                    self.render_question();
                    return;
                }
                serde_json::json!({"answers":self.answers})
            };
            (
                "answer",
                serde_json::json!({"threadId":thread_id,"interactionId":item["id"],"response":response}),
            )
        } else {
            let message = text(self.hwnd, TEXT);
            if message.trim().is_empty() {
                self.status("Введите сообщение");
                return;
            }
            (
                "send",
                serde_json::json!({"threadId":thread_id,"text":message}),
            )
        };
        self.busy = true;
        self.status("Отправка…");
        self.enable_controls(false);
        let result = self.result.clone();
        let generation = state().lock().unwrap().generation;
        std::thread::spawn(move || {
            let sent = request(&c, route, Some(&body));
            if sent.is_ok() {
                if let Ok(data) = request(&c, "state", None) {
                    let shared = state();
                    let mut s = shared.lock().unwrap();
                    if s.generation == generation {
                        s.data = data;
                        s.revision += 1;
                    }
                }
            }
            *result.lock().unwrap() = Some(sent);
        });
    }
    fn enable_controls(&self, enabled: bool) {
        unsafe {
            for id in [
                URL, TOKEN, SAVE, OFF, THREAD, ITEM, OPTIONS, TEXT, SEND, DENY, REFRESH, CONNECTION,
            ] {
                let _ = EnableWindow(child(self.hwnd, id), enabled);
            }
        }
    }
    fn tick(&mut self) {
        let result = self.result.lock().unwrap().take();
        if let Some(result) = result {
            self.busy = false;
            self.enable_controls(true);
            match result {
                Ok(value) => {
                    set(self.hwnd, TEXT, "");
                    self.answers.clear();
                    self.question = 0;
                    self.refresh();
                    self.status(if value["delivery"] == "queued" {
                        "Сообщение принято bb и поставлено в очередь."
                    } else {
                        "Отправлено в bb."
                    });
                }
                Err(error) => {
                    self.render_question();
                    self.status(&format!(
                        "{error}. Текст сохранён. Перед повтором проверьте тред в bb."
                    ));
                }
            }
        }
        if !self.loaded && !state().lock().unwrap().data.is_null() {
            self.refresh();
        } else if !self.loaded {
            let snapshot = state().lock().unwrap().clone();
            if !snapshot.error.is_empty() {
                self.status(&snapshot.error);
            }
        }
    }
}
pub fn open() {
    if OPEN.swap(true, Ordering::SeqCst) {
        unsafe {
            if let Ok(h) = FindWindowW(w!("TidemarkBbPanel"), None) {
                let _ = ShowWindow(h, SW_RESTORE);
                let _ = SetForegroundWindow(h);
            }
        }
        return;
    }
    std::thread::spawn(|| unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: w!("TidemarkBbPanel"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(
                (windows::Win32::Graphics::Gdi::COLOR_WINDOW.0 + 1) as *mut _,
            ),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let dpi = ui_dpi();
        let height = ((GetSystemMetrics(SM_CYSCREEN) - 96) * 96 / dpi as i32 - 40).clamp(620, 740);
        let mut bounds = RECT {
            left: 0,
            top: 0,
            right: skin::scale(760, dpi),
            bottom: skin::scale(height, dpi),
        };
        let _ = AdjustWindowRectExForDpi(
            &mut bounds,
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
            false,
            WS_EX_CONTROLPARENT,
            GetDpiForSystem(),
        );
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        let x = (GetSystemMetrics(SM_CXSCREEN) - width) / 2;
        if let Ok(hwnd) = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            w!("TidemarkBbPanel"),
            w!("Tidemark · bb"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
            x,
            ((GetSystemMetrics(SM_CYSCREEN) - height) / 2).max(0),
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        ) {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if !IsDialogMessageW(hwnd, &msg).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
        PANEL.with(|p| *p.borrow_mut() = None);
        OPEN.store(false, Ordering::SeqCst);
    });
}
unsafe fn control(hwnd: HWND, class: PCWSTR, label: &str, id: i32, style: WINDOW_STYLE) {
    let dpi = PANEL.with(|p| p.borrow().as_ref().map_or(96, |p| p.dpi));
    if let Ok(h) = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        &HSTRING::from(label),
        WS_CHILD | WS_VISIBLE | style,
        0,
        0,
        skin::scale(120, dpi),
        skin::scale(32, dpi),
        Some(hwnd),
        Some(HMENU(id as *mut _)),
        Some(GetModuleHandleW(None).unwrap_or_default().into()),
        None,
    ) {
        let font = PANEL
            .with(|p| {
                p.borrow().as_ref().map(|p| {
                    p.fonts[if id == STATUS {
                        1
                    } else if id == DETAIL {
                        3
                    } else {
                        0
                    }]
                })
            })
            .unwrap_or_default();
        send(h, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
        if matches!(
            id,
            SEND | SAVE | DENY | OFF | REFRESH | CONNECTION | TEXT | OPTIONS
        ) {
            let _ = windows::Win32::UI::Shell::SetWindowSubclass(h, Some(control_proc), 1, 0);
        }
    }
}
fn ui_dpi() -> u32 {
    unsafe {
        GetDpiForSystem()
            .max(96)
            .min((((GetSystemMetrics(SM_CYSCREEN) - 96) * 96) / 660).max(96) as u32)
    }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let dpi = ui_dpi();
            let c = connection();
            let p = Panel {
                hwnd,
                data: Value::Null,
                question: 0,
                answers: serde_json::Map::new(),
                result: Arc::new(Mutex::new(None)),
                busy: false,
                loaded: false,
                selected_thread: 0,
                selected_item: 0,
                connection_page: c.token.is_empty(),
                dpi,
                fonts: [
                    skin::font(15, 400, dpi),
                    skin::font(12, 400, dpi),
                    skin::font(25, 600, dpi),
                    skin::font(18, 400, dpi),
                ],
                brush: CreateSolidBrush(skin::color(skin::BG)),
                meta: String::new(),
                hovered: 0,
            };
            PANEL.with(|cell| *cell.borrow_mut() = Some(p));
            publish_render();
            let edit = WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32);
            let button = WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32);
            let combo = WS_TABSTOP
                | WS_VSCROLL
                | WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_OWNERDRAWFIXED | CBS_HASSTRINGS) as u32);
            control(hwnd, w!("EDIT"), "", URL, edit);
            control(
                hwnd,
                w!("EDIT"),
                "",
                TOKEN,
                edit | WINDOW_STYLE(ES_PASSWORD as u32),
            );
            control(hwnd, w!("BUTTON"), "Подключить", SAVE, button);
            control(hwnd, w!("BUTTON"), "Отключить", OFF, button);
            control(hwnd, w!("BUTTON"), "Подключение", CONNECTION, button);
            control(hwnd, w!("BUTTON"), "Обновить", REFRESH, button);
            control(hwnd, w!("COMBOBOX"), "", THREAD, combo);
            control(hwnd, w!("COMBOBOX"), "", ITEM, combo);
            control(
                hwnd,
                w!("EDIT"),
                "",
                DETAIL,
                WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY) as u32),
            );
            control(
                hwnd,
                w!("LISTBOX"),
                "",
                OPTIONS,
                WS_TABSTOP
                    | WS_VSCROLL
                    | WINDOW_STYLE(
                        (LBS_OWNERDRAWFIXED
                            | LBS_HASSTRINGS
                            | LBS_NOTIFY
                            | LBS_MULTIPLESEL
                            | LBS_NOINTEGRALHEIGHT) as u32,
                    ),
            );
            send(
                child(hwnd, OPTIONS),
                LB_SETITEMHEIGHT,
                WPARAM(0),
                LPARAM(skin::scale(64, dpi) as isize),
            );
            control(
                hwnd,
                w!("EDIT"),
                "",
                TEXT,
                WS_TABSTOP
                    | WS_VSCROLL
                    | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32),
            );
            control(hwnd, w!("BUTTON"), "Отправить", SEND, button);
            control(hwnd, w!("BUTTON"), "Отклонить", DENY, button);
            control(hwnd, w!("STATIC"), "", STATUS, WINDOW_STYLE(0));
            send(child(hwnd, TEXT), EM_SETLIMITTEXT, WPARAM(32000), LPARAM(0));
            send(
                child(hwnd, DETAIL),
                EM_SETLIMITTEXT,
                WPARAM(2 * 1024 * 1024),
                LPARAM(0),
            );
            send(
                child(hwnd, TEXT),
                EM_SETCUEBANNER,
                WPARAM(0),
                LPARAM(w!("Напишите ответ или дополнение к задаче…").as_ptr() as isize),
            );
            set(
                hwnd,
                URL,
                if c.url.is_empty() {
                    "http://127.0.0.1:38886"
                } else {
                    &c.url
                },
            );
            set(hwnd, TOKEN, &c.token);
            PANEL.with(|p| {
                if let Some(p) = p.borrow_mut().as_mut() {
                    p.refresh();
                    p.layout();
                }
            });
            publish_render();
            SetTimer(Some(hwnd), 1, 500, None);
            return LRESULT(0);
        }
        WM_COMMAND => {
            let id = (wp.0 & 0xffff) as i32;
            let notification = (wp.0 >> 16) as u32;
            if !matches!(
                id,
                SAVE | OFF | THREAD | ITEM | SEND | DENY | REFRESH | CONNECTION | OPTIONS
            ) {
                return LRESULT(0);
            }
            PANEL.with(|cell| {
                let Ok(mut borrow) = cell.try_borrow_mut() else {
                    return;
                };
                let Some(p) = borrow.as_mut() else { return };
                if p.busy {
                    return;
                }
                match id {
                    CONNECTION => {
                        p.connection_page = !p.connection_page;
                        p.layout();
                    }
                    OPTIONS if notification == LBN_SELCHANGE => {
                        if p.item().is_some_and(|i| {
                            i["payload"]["questions"][p.question]["multiSelect"] != true
                        }) {
                            let index =
                                send(child(hwnd, OPTIONS), LB_GETCARETINDEX, WPARAM(0), LPARAM(0))
                                    .0;
                            let selected = send(
                                child(hwnd, OPTIONS),
                                LB_GETSEL,
                                WPARAM(index.max(0) as usize),
                                LPARAM(0),
                            )
                            .0;
                            send(child(hwnd, OPTIONS), LB_SETSEL, WPARAM(0), LPARAM(-1));
                            if selected > 0 {
                                send(child(hwnd, OPTIONS), LB_SETSEL, WPARAM(1), LPARAM(index));
                            }
                        }
                    }
                    SAVE => {
                        if !p.discard_allowed() {
                            return;
                        }
                        let c = Connection {
                            url: text(hwnd, URL).trim().to_string(),
                            token: text(hwnd, TOKEN).trim().to_string(),
                        };
                        match save(&c) {
                            Ok(()) => {
                                p.connection_page = false;
                                p.refresh();
                                p.loaded = false;
                                p.status("Подключение…");
                            }
                            Err(e) => p.status(&e),
                        }
                    }
                    OFF => {
                        if !p.discard_allowed() {
                            return;
                        }
                        disconnect();
                        set(hwnd, TOKEN, "");
                        p.connection_page = true;
                        p.refresh();
                    }
                    THREAD if notification == CBN_SELCHANGE => {
                        if p.discard_allowed() {
                            p.select_thread();
                        } else {
                            send(
                                child(hwnd, THREAD),
                                CB_SETCURSEL,
                                WPARAM(p.selected_thread),
                                LPARAM(0),
                            );
                        }
                    }
                    ITEM if notification == CBN_SELCHANGE => {
                        if p.discard_allowed() {
                            p.select_item();
                        } else {
                            send(
                                child(hwnd, ITEM),
                                CB_SETCURSEL,
                                WPARAM(p.selected_item),
                                LPARAM(0),
                            );
                        }
                    }
                    SEND => p.submit(false),
                    DENY => p.submit(true),
                    REFRESH => {
                        if p.has_draft()
                            && MessageBoxW(
                                Some(hwnd),
                                w!("Обновление очистит текущий черновик. Продолжить?"),
                                w!("Tidemark · bb"),
                                MB_YESNO | MB_ICONQUESTION,
                            ) != IDYES
                        {
                            return;
                        }
                        p.refresh();
                    }
                    _ => {}
                }
            });
            publish_render();
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            paint_window(hwnd, None);
            return LRESULT(0);
        }
        WM_PRINTCLIENT => {
            paint_window(hwnd, Some(HDC(wp.0 as *mut _)));
            return LRESULT(0);
        }
        WM_DRAWITEM => {
            draw_control(&*(lp.0 as *const DRAWITEMSTRUCT));
            return LRESULT(1);
        }
        WM_MEASUREITEM => {
            let item = &mut *(lp.0 as *mut MEASUREITEMSTRUCT);
            item.itemHeight =
                skin::scale(if item.CtlID == OPTIONS as u32 { 64 } else { 26 }, ui_dpi()) as u32;
            return LRESULT(1);
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC | WM_CTLCOLORLISTBOX => {
            let dc = HDC(wp.0 as *mut _);
            let id = GetDlgCtrlID(HWND(lp.0 as *mut _));
            SetTextColor(
                dc,
                skin::color(if id == STATUS { skin::MUTED } else { skin::INK }),
            );
            let white = matches!(id, URL | TOKEN | TEXT | DETAIL);
            SetBkColor(
                dc,
                skin::color(if white { visual::WHITE } else { skin::BG }),
            );
            if white {
                return LRESULT(GetStockObject(WHITE_BRUSH).0 as isize);
            }
            return LRESULT(
                RENDER
                    .with(|r| r.borrow().as_ref().map(|r| r.brush))
                    .unwrap_or_default()
                    .0 as isize,
            );
        }
        WM_SIZE => {
            PANEL.with(|p| {
                if let Ok(p) = p.try_borrow() {
                    if let Some(p) = p.as_ref() {
                        p.layout();
                    }
                }
            });
            publish_render();
            return LRESULT(0);
        }
        WM_TIMER => {
            PANEL.with(|p| {
                if let Ok(mut p) = p.try_borrow_mut() {
                    if let Some(p) = p.as_mut() {
                        p.tick();
                    }
                }
            });
            publish_render();
            return LRESULT(0);
        }
        WM_CLOSE => {
            let busy = PANEL.with(|p| p.borrow().as_ref().is_some_and(|p| p.busy));
            if busy {
                return LRESULT(0);
            }
            if PANEL.with(|p| p.borrow().as_ref().is_some_and(Panel::has_draft))
                && MessageBoxW(
                    Some(hwnd),
                    w!("Закрыть панель и удалить черновик?"),
                    w!("Tidemark · bb"),
                    MB_YESNO | MB_ICONQUESTION,
                ) != IDYES
            {
                return LRESULT(0);
            }
            let _ = DestroyWindow(hwnd);
            return LRESULT(0);
        }
        WM_DESTROY => {
            RENDER.with(|r| *r.borrow_mut() = None);
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe fn send(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    SendMessageW(hwnd, msg, Some(wp), Some(lp))
}
unsafe fn paint_window(hwnd: HWND, target: Option<HDC>) {
    let mut ps = PAINTSTRUCT::default();
    let dc = target.unwrap_or_else(|| BeginPaint(hwnd, &mut ps));
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let memory = CreateCompatibleDC(Some(dc));
    let bitmap = CreateCompatibleBitmap(dc, rc.right.max(1), rc.bottom.max(1));
    let old = SelectObject(memory, bitmap.into());
    let fallback = CreateSolidBrush(skin::color(skin::BG));
    FillRect(memory, &rc, fallback);
    let _ = DeleteObject(fallback.into());
    let snapshot = RENDER.with(|r| r.borrow().clone());
    if let Some(r) = snapshot {
        let canvas = skin::Canvas {
            dc: memory,
            dpi: r.dpi,
            fonts: r.fonts,
        };
        visual::paint(
            &canvas,
            r.width,
            r.height,
            r.connection,
            &r.meta,
            r.choices.len(),
            windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() == child(hwnd, TEXT),
        );
    }
    let _ = BitBlt(dc, 0, 0, rc.right, rc.bottom, Some(memory), 0, 0, SRCCOPY);
    SelectObject(memory, old);
    let _ = DeleteObject(bitmap.into());
    let _ = DeleteDC(memory);
    if target.is_none() {
        let _ = EndPaint(hwnd, &ps);
    }
}
unsafe fn draw_control(d: &DRAWITEMSTRUCT) {
    let saved = SaveDC(d.hDC);
    let _ = SetViewportOrgEx(d.hDC, d.rcItem.left, d.rcItem.top, None);
    let snapshot = RENDER.with(|r| r.borrow().clone());
    if let Some(r) = snapshot {
        let width = (d.rcItem.right - d.rcItem.left) * 96 / r.dpi as i32;
        let height = (d.rcItem.bottom - d.rcItem.top) * 96 / r.dpi as i32;
        let canvas = skin::Canvas {
            dc: d.hDC,
            dpi: r.dpi,
            fonts: r.fonts,
        };
        let focus = d.itemState.0 & ODS_FOCUS.0 != 0;
        if d.CtlID == OPTIONS as u32 {
            if let Some(o) = r.choices.get(d.itemID as usize) {
                visual::choice(
                    &canvas,
                    width,
                    height,
                    o["label"].as_str().unwrap_or(""),
                    o["description"].as_str().unwrap_or(""),
                    d.itemState.0 & ODS_SELECTED.0 != 0,
                    r.multi,
                    focus,
                );
            } else {
                canvas.fill(0, 0, width, height, skin::BG);
            }
        } else if matches!(d.CtlID as i32, THREAD | ITEM) {
            canvas.fill(
                0,
                0,
                width,
                height,
                if d.itemState.0 & ODS_SELECTED.0 != 0 {
                    skin::SELECT
                } else {
                    visual::WHITE
                },
            );
            let length = send(
                d.hwndItem,
                CB_GETLBTEXTLEN,
                WPARAM(d.itemID as usize),
                LPARAM(0),
            )
            .0;
            if (0..32000).contains(&length) {
                let mut text = vec![0u16; length as usize + 1];
                send(
                    d.hwndItem,
                    CB_GETLBTEXT,
                    WPARAM(d.itemID as usize),
                    LPARAM(text.as_mut_ptr() as isize),
                );
                canvas.text(
                    &String::from_utf16_lossy(&text[..length as usize]),
                    10,
                    0,
                    width - 20,
                    height,
                    0,
                    skin::INK,
                );
            }
        } else {
            visual::button(
                &canvas,
                width,
                height,
                &skin::caption(d.hwndItem),
                matches!(d.CtlID as i32, SEND | SAVE),
                d.itemState.0 & ODS_SELECTED.0 != 0,
                focus,
                d.itemState.0 & ODS_DISABLED.0 == 0,
                r.hovered == d.CtlID as i32,
            );
        }
    }
    let _ = RestoreDC(d.hDC, saved);
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
    let parent = GetParent(h).unwrap_or_default();
    if matches!(
        msg,
        WM_MOUSEMOVE | WM_MOUSELEAVE | WM_SETFOCUS | WM_KILLFOCUS
    ) {
        PANEL.with(|cell| {
            if let Ok(mut p) = cell.try_borrow_mut() {
                if let Some(p) = p.as_mut() {
                    if msg == WM_MOUSEMOVE {
                        p.hovered = id;
                        let mut track =
                            windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT {
                                cbSize: std::mem::size_of::<
                                    windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT,
                                >() as u32,
                                dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::TME_LEAVE,
                                hwndTrack: h,
                                ..Default::default()
                            };
                        let _ = windows::Win32::UI::Input::KeyboardAndMouse::TrackMouseEvent(
                            &mut track,
                        );
                    }
                    if msg == WM_MOUSELEAVE && p.hovered == id {
                        p.hovered = 0;
                    }
                }
            }
        });
        publish_render();
        let _ = InvalidateRect(Some(h), None, false);
        let _ = InvalidateRect(Some(parent), None, false);
    }
    if msg == WM_SETCURSOR && id != TEXT {
        let _ = SetCursor(LoadCursorW(None, IDC_HAND).ok());
        return LRESULT(1);
    }
    windows::Win32::UI::Shell::DefSubclassProc(h, msg, wp, lp)
}

#[cfg(test)]
mod render_tests {
    use super::*;
    #[test]
    fn owner_draw_works_during_a_mutable_controller_borrow() {
        unsafe {
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: 320,
                    biHeight: -64,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp =
                CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            let old = SelectObject(dc, bmp.into());
            let fonts = [
                skin::font(15, 400, 96),
                skin::font(12, 400, 96),
                skin::font(25, 600, 96),
                skin::font(18, 400, 96),
            ];
            let brush = CreateSolidBrush(skin::color(skin::BG));
            PANEL.with(|p| {
                *p.borrow_mut() = Some(Panel {
                    hwnd: HWND::default(),
                    data: Value::Null,
                    question: 0,
                    answers: serde_json::Map::new(),
                    result: Arc::new(Mutex::new(None)),
                    busy: false,
                    loaded: false,
                    selected_thread: 0,
                    selected_item: 0,
                    connection_page: false,
                    dpi: 96,
                    fonts,
                    brush,
                    meta: String::new(),
                    hovered: 0,
                })
            });
            RENDER.with(|r| {
                *r.borrow_mut() = Some(Render {
                    dpi: 96,
                    fonts,
                    brush,
                    width: 320,
                    height: 64,
                    connection: false,
                    meta: String::new(),
                    choices: vec![serde_json::json!({"label":"Option","description":"Details"})],
                    multi: false,
                    hovered: 0,
                })
            });
            PANEL.with(|p| {
                let _guard = p.borrow_mut();
                draw_control(&DRAWITEMSTRUCT {
                    CtlID: OPTIONS as u32,
                    itemID: 0,
                    hDC: dc,
                    rcItem: RECT {
                        left: 0,
                        top: 0,
                        right: 320,
                        bottom: 64,
                    },
                    ..Default::default()
                });
            });
            assert_eq!(GetPixel(dc, 280, 20), skin::color(visual::WHITE));
            RENDER.with(|r| *r.borrow_mut() = None);
            PANEL.with(|p| *p.borrow_mut() = None);
            SelectObject(dc, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(dc);
        }
    }
}
