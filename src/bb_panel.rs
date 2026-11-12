//! Native bb panel, on its own UI thread. No network calls in window procedures.
use super::*;
use std::{cell::RefCell, sync::atomic::AtomicBool};
use windows::Win32::{
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
thread_local! {static PANEL:RefCell<Option<Panel>>=const{RefCell::new(None)};}
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
                    &format!(
                        "{}  ·  {}",
                        t["title"].as_str().unwrap_or(""),
                        t["status"].as_str().unwrap_or("")
                    ),
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
                let mut detail = format!(
                    "Вопрос {} из {}\r\n\r\n{}\r\n\r\n{}",
                    self.question + 1,
                    p["questions"].as_array().map_or(0, Vec::len),
                    q["prompt"].as_str().unwrap_or(""),
                    if q["multiSelect"] == true {
                        "Можно выбрать несколько вариантов (Ctrl + клик)."
                    } else {
                        "Выберите один вариант или введите свой ответ, если это разрешено."
                    }
                );
                if let Some(options) = q["options"].as_array() {
                    for o in options {
                        let label = o["label"].as_str().unwrap_or("");
                        let value: Vec<u16> = label.encode_utf16().chain(Some(0)).collect();
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
                        detail.push_str(&format!(
                            "\r\n\r\n{}: {}",
                            label,
                            o["description"].as_str().unwrap_or("")
                        ));
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
                URL, TOKEN, SAVE, OFF, THREAD, ITEM, OPTIONS, TEXT, SEND, DENY, REFRESH,
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
        let width = 760;
        let x = (GetSystemMetrics(SM_CXSCREEN) - width) / 2;
        if let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST,
            w!("TidemarkBbPanel"),
            w!("Tidemark · bb"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            x,
            70,
            width,
            770,
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
unsafe fn control(
    hwnd: HWND,
    class: PCWSTR,
    label: &str,
    id: i32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    style: WINDOW_STYLE,
) {
    if let Ok(h) = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        &HSTRING::from(label),
        WS_CHILD | WS_VISIBLE | style,
        x,
        y,
        width,
        height,
        Some(hwnd),
        Some(HMENU(id as *mut _)),
        Some(GetModuleHandleW(None).unwrap_or_default().into()),
        None,
    ) {
        use windows::Win32::Graphics::Gdi::*;
        send(
            h,
            WM_SETFONT,
            WPARAM(GetStockObject(DEFAULT_GUI_FONT).0 as usize),
            LPARAM(1),
        );
    }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let edit = WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32);
            let button = WS_TABSTOP;
            let combo = WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32);
            control(
                hwnd,
                w!("STATIC"),
                "Адрес сервера bb",
                0,
                16,
                12,
                200,
                20,
                WINDOW_STYLE(0),
            );
            control(hwnd, w!("EDIT"), "", URL, 16, 34, 540, 26, edit);
            control(
                hwnd,
                w!("BUTTON"),
                "Подключить",
                SAVE,
                568,
                34,
                160,
                26,
                button,
            );
            control(
                hwnd,
                w!("STATIC"),
                "Токен: bb plugin token tidemark",
                0,
                16,
                66,
                450,
                20,
                WINDOW_STYLE(0),
            );
            control(
                hwnd,
                w!("EDIT"),
                "",
                TOKEN,
                16,
                88,
                540,
                26,
                edit | WINDOW_STYLE(ES_PASSWORD as u32),
            );
            control(
                hwnd,
                w!("BUTTON"),
                "Отключить",
                OFF,
                568,
                88,
                160,
                26,
                button,
            );
            control(
                hwnd,
                w!("STATIC"),
                "Тред",
                0,
                16,
                125,
                160,
                20,
                WINDOW_STYLE(0),
            );
            control(hwnd, w!("COMBOBOX"), "", THREAD, 16, 147, 540, 300, combo);
            control(
                hwnd,
                w!("BUTTON"),
                "Обновить",
                REFRESH,
                568,
                147,
                160,
                26,
                button,
            );
            control(hwnd, w!("COMBOBOX"), "", ITEM, 16, 185, 712, 300, combo);
            control(
                hwnd,
                w!("EDIT"),
                "",
                DETAIL,
                16,
                225,
                712,
                185,
                WS_BORDER
                    | WS_VSCROLL
                    | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY) as u32),
            );
            control(
                hwnd,
                w!("LISTBOX"),
                "",
                OPTIONS,
                16,
                420,
                712,
                90,
                WS_BORDER
                    | WS_TABSTOP
                    | WS_VSCROLL
                    | WINDOW_STYLE((LBS_EXTENDEDSEL | LBS_NOINTEGRALHEIGHT) as u32),
            );
            control(
                hwnd,
                w!("STATIC"),
                "Ваш ответ / дополнение",
                0,
                16,
                519,
                350,
                20,
                WINDOW_STYLE(0),
            );
            control(
                hwnd,
                w!("EDIT"),
                "",
                TEXT,
                16,
                543,
                712,
                95,
                WS_BORDER
                    | WS_TABSTOP
                    | WS_VSCROLL
                    | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32),
            );
            control(
                hwnd,
                w!("BUTTON"),
                "Отправить",
                SEND,
                16,
                650,
                300,
                30,
                button,
            );
            control(
                hwnd,
                w!("BUTTON"),
                "Отклонить",
                DENY,
                328,
                650,
                140,
                30,
                button,
            );
            control(
                hwnd,
                w!("STATIC"),
                "",
                STATUS,
                16,
                688,
                712,
                45,
                WINDOW_STYLE(0),
            );
            send(child(hwnd, TEXT), EM_SETLIMITTEXT, WPARAM(32000), LPARAM(0));
            send(
                child(hwnd, DETAIL),
                EM_SETLIMITTEXT,
                WPARAM(2 * 1024 * 1024),
                LPARAM(0),
            );
            let c = connection();
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
            let mut p = Panel {
                hwnd,
                data: Value::Null,
                question: 0,
                answers: serde_json::Map::new(),
                result: Arc::new(Mutex::new(None)),
                busy: false,
                loaded: false,
                selected_thread: 0,
                selected_item: 0,
            };
            p.refresh();
            PANEL.with(|cell| *cell.borrow_mut() = Some(p));
            SetTimer(Some(hwnd), 1, 500, None);
            return LRESULT(0);
        }
        WM_COMMAND => {
            let id = (wp.0 & 0xffff) as i32;
            let notification = (wp.0 >> 16) as u32;
            if !matches!(id, SAVE | OFF | THREAD | ITEM | SEND | DENY | REFRESH) {
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
