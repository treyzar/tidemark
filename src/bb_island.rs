//! State and geometry for the bb page rendered inside the real island.
use crate::{bb, layout::Rect, textinput::TextField};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
pub const HEIGHT: f32 = 500.0;
#[derive(Default)]
struct Draft {
    question: usize,
    fields: HashMap<String, TextField>,
    selected: HashMap<String, Vec<String>>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    pub description: String,
    pub selected: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub thread: String,
    pub meta: String,
    pub prompt: String,
    pub choices: Vec<Choice>,
    pub field: TextField,
    pub focus: bool,
    pub allow_text: bool,
    pub multi: bool,
    pub busy: bool,
    pub status: String,
    pub send: String,
    pub picker: bool,
    pub threads: Vec<(String, String)>,
    pub pending: usize,
    pub can_back: bool,
    pub scroll: f32,
    pub offset: usize,
    pub approval: bool,
}
pub struct Client {
    data: Value,
    pub thread_id: String,
    interaction: Option<String>,
    drafts: HashMap<String, Draft>,
    pub picker: bool,
    filter: TextField,
    pub offset: usize,
    pub scroll: f32,
    pub busy: bool,
    status: String,
    last_error: String,
    result: Arc<Mutex<Option<Result<Value, String>>>>,
}
impl Default for Client {
    fn default() -> Self {
        Self {
            data: Value::Null,
            thread_id: String::new(),
            interaction: None,
            drafts: HashMap::new(),
            picker: false,
            filter: TextField::new(false),
            offset: 0,
            scroll: 0.0,
            busy: false,
            status: String::new(),
            last_error: String::new(),
            result: Arc::new(Mutex::new(None)),
        }
    }
}
impl Client {
    fn items(&self) -> Vec<Value> {
        self.data["interactions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i["threadId"] == self.thread_id)
            .cloned()
            .collect()
    }
    fn item(&self) -> Option<Value> {
        self.items()
            .into_iter()
            .find(|i| Some(i["id"].as_str().unwrap_or("")) == self.interaction.as_deref())
    }
    fn key(&self) -> String {
        self.interaction
            .clone()
            .unwrap_or_else(|| format!("compose:{}", self.thread_id))
    }
    fn q(&self) -> Value {
        let n = self.drafts.get(&self.key()).map_or(0, |d| d.question);
        self.item()
            .map(|i| i["payload"]["questions"][n].clone())
            .unwrap_or(Value::Null)
    }
    fn field_key(&self) -> String {
        self.q()["id"].as_str().unwrap_or("message").to_string()
    }
    pub fn input(&mut self) -> &mut TextField {
        if self.picker {
            return &mut self.filter;
        }
        let key = self.key();
        let q = self.field_key();
        self.drafts
            .entry(key)
            .or_default()
            .fields
            .entry(q)
            .or_insert_with(|| TextField::new(true))
    }
    pub fn edited(&mut self) {
        if self.picker {
            self.offset = 0
        } else {
            self.status.clear();
        }
    }
    pub fn sync(&mut self, prefer_question: bool) {
        let snapshot = bb::state().lock().unwrap().clone();
        self.absorb(snapshot.data, snapshot.error, prefer_question);
    }
    fn absorb(&mut self, data: Value, error: String, prefer_question: bool) {
        let result = self.result.lock().unwrap().take();
        if self.busy && result.is_none() {
            return;
        }
        self.data = data;
        if !error.is_empty() {
            self.status = error.clone();
            self.last_error = error;
        } else if !self.last_error.is_empty() {
            if self.status == self.last_error {
                self.status.clear();
            }
            self.last_error.clear();
        }
        if let Some(result) = result {
            self.busy = false;
            match result {
                Ok(v) => {
                    let key = self.key();
                    self.drafts.remove(&key);
                    if self.interaction.is_some() {
                        self.interaction = None;
                    }
                    self.status = if v["delivery"] == "queued" {
                        "Сообщение в очереди"
                    } else {
                        "Отправлено"
                    }
                    .into();
                }
                Err(e) => self.status = format!("{e}. Черновик сохранён."),
            }
        }
        let threads = self.data["threads"].as_array();
        if prefer_question && !self.busy {
            if let Some(id) = self.data["interactions"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|i| i["threadId"].as_str())
            {
                self.thread_id = id.to_string();
            }
        }
        if !self.busy && !threads.is_some_and(|ts| ts.iter().any(|t| t["id"] == self.thread_id)) {
            self.thread_id = self.data["interactions"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|i| i["threadId"].as_str())
                .or_else(|| {
                    threads
                        .and_then(|a| a.first())
                        .and_then(|t| t["id"].as_str())
                })
                .unwrap_or("")
                .into();
            self.interaction = None;
        }
        if !self.busy && (prefer_question || self.interaction.is_some() && self.item().is_none()) {
            self.interaction = self
                .items()
                .first()
                .and_then(|i| i["id"].as_str())
                .map(str::to_string);
            self.scroll = 0.0;
            self.offset = 0;
        }
    }
    pub fn toggle_picker(&mut self) {
        if !self.busy {
            self.picker = !self.picker;
            self.offset = 0;
            self.filter = TextField::new(false);
        }
    }
    fn filtered(&self) -> Vec<Value> {
        let query = self.filter.text.to_lowercase();
        self.data["threads"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| {
                query.is_empty()
                    || t["title"]
                        .as_str()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
            })
            .cloned()
            .collect()
    }
    pub fn choose_thread(&mut self, index: usize) {
        if self.busy {
            return;
        }
        if let Some(t) = self.filtered().get(self.offset + index) {
            self.thread_id = t["id"].as_str().unwrap_or("").into();
            self.interaction = self
                .items()
                .first()
                .and_then(|i| i["id"].as_str())
                .map(str::to_string);
            self.picker = false;
            self.scroll = 0.0;
            self.offset = 0;
            self.status.clear();
        }
    }
    pub fn compose(&mut self) {
        if self.busy {
            return;
        }
        self.interaction = if self.interaction.is_some() {
            None
        } else {
            self.items()
                .first()
                .and_then(|i| i["id"].as_str())
                .map(str::to_string)
        };
        self.offset = 0;
        self.scroll = 0.0;
        self.status.clear();
    }
    pub fn next_request(&mut self) {
        if self.busy {
            return;
        }
        let items = self.items();
        if items.len() > 1 {
            let n = items
                .iter()
                .position(|i| i["id"].as_str() == self.interaction.as_deref())
                .unwrap_or(0);
            self.interaction = items[(n + 1) % items.len()]["id"]
                .as_str()
                .map(str::to_string);
            self.offset = 0;
            self.scroll = 0.0;
        }
    }
    pub fn back(&mut self) {
        if self.busy {
            return;
        }
        let key = self.key();
        if let Some(d) = self.drafts.get_mut(&key) {
            d.question = d.question.saturating_sub(1);
        }
        self.offset = 0;
        self.scroll = 0.0;
    }
    pub fn choose(&mut self, index: usize) {
        if self.busy {
            return;
        }
        let q = self.q();
        let Some(v) = q["options"]
            .get(self.offset + index)
            .and_then(|o| o["value"].as_str())
        else {
            return;
        };
        let key = self.key();
        let qid = q["id"].as_str().unwrap_or("").to_string();
        let selected = self
            .drafts
            .entry(key)
            .or_default()
            .selected
            .entry(qid)
            .or_default();
        if let Some(i) = selected.iter().position(|s| s == v) {
            selected.remove(i);
        } else {
            if q["multiSelect"] != true {
                selected.clear();
            }
            selected.push(v.to_string());
        }
        self.status.clear();
    }
    pub fn scroll_list(&mut self, down: bool, body: bool) {
        if body {
            self.scroll = (self.scroll + if down { 36.0 } else { -36.0 }).max(0.0);
            return;
        }
        let count = if self.picker {
            self.filtered().len()
        } else {
            self.q()["options"].as_array().map_or(0, Vec::len)
        };
        let visible = if self.picker { 5 } else { 3 };
        if down {
            self.offset = (self.offset + 1).min(count.saturating_sub(visible));
        } else {
            self.offset = self.offset.saturating_sub(1);
        }
    }
    pub fn view(&self, focus: bool) -> View {
        let item = self.item();
        let q = self.q();
        let key = self.key();
        let draft = self.drafts.get(&key);
        let field_key = self.field_key();
        let field = if self.picker {
            self.filter.clone()
        } else {
            draft
                .and_then(|d| d.fields.get(&field_key))
                .cloned()
                .unwrap_or_else(|| TextField::new(true))
        };
        let n = draft.map_or(0, |d| d.question);
        let count = item
            .as_ref()
            .and_then(|i| i["payload"]["questions"].as_array())
            .map_or(0, Vec::len);
        let selected = draft.and_then(|d| q["id"].as_str().and_then(|id| d.selected.get(id)));
        let choices = q["options"]
            .as_array()
            .into_iter()
            .flatten()
            .skip(self.offset)
            .take(3)
            .map(|o| Choice {
                label: o["label"].as_str().unwrap_or("").into(),
                description: o["description"].as_str().unwrap_or("").into(),
                selected: selected
                    .is_some_and(|s| s.iter().any(|s| Some(s.as_str()) == o["value"].as_str())),
            })
            .collect();
        let approval = item
            .as_ref()
            .is_some_and(|i| i["payload"]["kind"] == "approval");
        let prompt = if self.picker {
            "Выберите разговор".into()
        } else if let Some(item) = &item {
            if item["payload"]["kind"] == "user_question" {
                q["prompt"].as_str().unwrap_or("").into()
            } else if approval {
                item["payload"]["reason"]
                    .as_str()
                    .unwrap_or("Агент запрашивает разрешение")
                    .to_string()
                    + "\n"
                    + &serde_json::to_string_pretty(&item["payload"]["subject"]).unwrap_or_default()
            } else {
                "Откройте этот запрос в bb".into()
            }
        } else if self.thread_id.is_empty() {
            "Подключите bb, чтобы отвечать агентам".into()
        } else {
            "Что передать агенту?".into()
        };
        let thread = self.data["threads"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| t["id"] == self.thread_id)
            .and_then(|t| t["title"].as_str())
            .unwrap_or("Выбрать разговор")
            .to_string();
        View {
            thread,
            meta: if self.picker {
                "Разговоры".into()
            } else if count > 0 {
                format!("Вопрос {} из {count}", n + 1)
            } else if approval {
                "Нужно разрешение".into()
            } else {
                "Новое сообщение".into()
            },
            prompt,
            choices,
            field,
            focus,
            allow_text: self.interaction.is_none() || q["allowFreeText"] == true,
            multi: q["multiSelect"] == true,
            busy: self.busy,
            status: self.status.clone(),
            send: if self.picker {
                "Готово"
            } else if self.busy {
                "Отправляю…"
            } else if n + 1 < count {
                "Далее"
            } else if approval {
                "Разрешить"
            } else {
                "Отправить"
            }
            .into(),
            picker: self.picker,
            threads: self
                .filtered()
                .iter()
                .skip(self.offset)
                .take(5)
                .map(|t| {
                    (
                        t["title"].as_str().unwrap_or("").into(),
                        if self.data["interactions"]
                            .as_array()
                            .is_some_and(|items| items.iter().any(|i| i["threadId"] == t["id"]))
                        {
                            "Ждёт ответа"
                        } else {
                            match t["status"].as_str() {
                                Some("active" | "starting") => "В работе",
                                _ => "",
                            }
                        }
                        .into(),
                    )
                })
                .collect(),
            pending: self.items().len(),
            can_back: n > 0,
            scroll: self.scroll,
            offset: self.offset,
            approval,
        }
    }
    pub fn submit(&mut self, hwnd: isize, deny: bool) {
        if self.busy {
            return;
        }
        if self.picker {
            self.picker = false;
            return;
        }
        if self.thread_id.is_empty() {
            self.status = "Сначала подключите bb".into();
            return;
        }
        let item = self.item();
        let key = self.key();
        let draft = self.drafts.entry(key).or_default();
        let (route, body) = if let Some(item) = item {
            let p = &item["payload"];
            let response = if p["kind"] == "approval" {
                if p["subject"]["kind"] == "permission_grant" {
                    self.status = "Этот запрос нужно открыть в bb".into();
                    return;
                }
                json!({"decision":if deny{"deny"}else{"allow_once"}})
            } else if p["kind"] == "user_question" {
                let q = &p["questions"][draft.question];
                let id = q["id"].as_str().unwrap_or("");
                let selections = draft.selected.get(id).cloned().unwrap_or_default();
                let free = draft.fields.get(id).map(|f| f.text.as_str()).unwrap_or("");
                if q["required"] == true && selections.is_empty() && free.trim().is_empty() {
                    self.status = "Выберите вариант или напишите ответ".into();
                    return;
                }
                if draft.question + 1 < p["questions"].as_array().map_or(0, Vec::len) {
                    draft.question += 1;
                    self.offset = 0;
                    self.scroll = 0.0;
                    return;
                }
                let mut answers = serde_json::Map::new();
                for q in p["questions"].as_array().unwrap() {
                    let id = q["id"].as_str().unwrap_or("");
                    let mut a =
                        json!({"selected":draft.selected.get(id).cloned().unwrap_or_default()});
                    if q["allowFreeText"] == true {
                        if let Some(f) = draft.fields.get(id).filter(|f| !f.text.trim().is_empty())
                        {
                            a["freeText"] = f.text.clone().into();
                        }
                    }
                    answers.insert(id.into(), a);
                }
                json!({"answers":answers})
            } else {
                self.status = "Этот запрос нужно открыть в bb".into();
                return;
            };
            (
                "answer",
                json!({"threadId":self.thread_id,"interactionId":item["id"],"response":response}),
            )
        } else {
            let text = draft
                .fields
                .get("message")
                .map(|f| f.text.clone())
                .unwrap_or_default();
            if text.trim().is_empty() {
                self.status = "Введите сообщение".into();
                return;
            }
            ("send", json!({"threadId":self.thread_id,"text":text}))
        };
        self.busy = true;
        self.status = "Отправка…".into();
        let c = bb::connection();
        let result = self.result.clone();
        let generation = bb::state().lock().unwrap().generation;
        std::thread::spawn(move || {
            let sent = bb::request(&c, route, Some(&body));
            if sent.is_ok() {
                if let Ok(data) = bb::request(&c, "state", None) {
                    let shared = bb::state();
                    let mut s = shared.lock().unwrap();
                    if s.generation == generation {
                        s.data = data;
                        s.revision += 1;
                    }
                }
            }
            *result.lock().unwrap() = Some(sent);
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(windows::Win32::Foundation::HWND(hwnd as *mut _)),
                    bb::WM_RESULT,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                );
            }
        });
    }
}
pub struct Regions {
    pub thread: Rect,
    pub window: Rect,
    pub settings: Rect,
    pub mode: Rect,
    pub next: Rect,
    pub prompt: Rect,
    pub choices: Vec<Rect>,
    pub field: Rect,
    pub send: Rect,
    pub back: Rect,
    pub status: Rect,
    pub rows: Vec<Rect>,
}
pub fn regions(width: f32, height: f32, view: &View) -> Regions {
    let inner = width - 40.0;
    let prompt = Rect {
        x: 24.0,
        y: 132.0,
        w: inner - 8.0,
        h: 70.0,
    };
    let choices: Vec<Rect> = (0..view.choices.len())
        .map(|i| Rect {
            x: 20.0,
            y: 212.0 + i as f32 * 48.0,
            w: inner,
            h: 42.0,
        })
        .collect();
    let input_top = if view.picker {
        110.0
    } else if view.choices.is_empty() {
        206.0
    } else {
        212.0 + view.choices.len() as f32 * 48.0 + 8.0
    };
    Regions {
        thread: Rect {
            x: 20.0,
            y: 56.0,
            w: inner - 88.0,
            h: 36.0,
        },
        window: Rect {
            x: width - 98.0,
            y: 56.0,
            w: 34.0,
            h: 36.0,
        },
        settings: Rect {
            x: width - 58.0,
            y: 56.0,
            w: 38.0,
            h: 36.0,
        },
        mode: Rect {
            x: width - 148.0,
            y: 102.0,
            w: 128.0,
            h: 26.0,
        },
        next: Rect {
            x: width - 184.0,
            y: 102.0,
            w: 30.0,
            h: 26.0,
        },
        prompt,
        choices,
        field: Rect {
            x: 20.0,
            y: input_top,
            w: inner,
            h: if view.picker {
                36.0
            } else {
                (height - 76.0 - input_top).max(48.0)
            },
        },
        send: Rect {
            x: width - 146.0,
            y: height - 54.0,
            w: 126.0,
            h: 34.0,
        },
        back: Rect {
            x: 20.0,
            y: height - 54.0,
            w: 68.0,
            h: 34.0,
        },
        status: Rect {
            x: if view.can_back || view.approval {
                100.0
            } else {
                24.0
            },
            y: height - 54.0,
            w: width - 270.0,
            h: 34.0,
        },
        rows: (0..view.threads.len())
            .map(|i| Rect {
                x: 20.0,
                y: 158.0 + i as f32 * 52.0,
                w: inner,
                h: 46.0,
            })
            .collect(),
    }
}

pub fn theme() -> crate::theme::Theme {
    crate::theme::Theme {
        light: false,
        background: [0.025, 0.03, 0.04, 1.0],
        tile: [1.0, 1.0, 1.0, 0.06],
        tile_hover: [1.0, 1.0, 1.0, 0.11],
        text: [0.95, 0.965, 0.985, 1.0],
        subtext: [0.56, 0.59, 0.65, 1.0],
        accent: [0.48, 0.69, 1.0, 1.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn data() -> Value {
        json!({"threads":[{"id":"a","title":"Alpha"},{"id":"b","title":"Beta"}],"interactions":[{"id":"i1","threadId":"a","payload":{"kind":"user_question","questions":[{"id":"q1","prompt":"Pick","allowFreeText":true,"required":true,"multiSelect":false,"options":[{"label":"One","value":"one"},{"label":"Two","value":"two"}]},{"id":"q2","prompt":"Explain","allowFreeText":true,"required":true}]}}]})
    }
    fn client() -> Client {
        let mut c = Client::default();
        c.absorb(data(), String::new(), true);
        c
    }
    #[test]
    fn actual_question_options_select_by_value() {
        let mut c = client();
        c.choose(0);
        c.choose(1);
        let v = c.view(false);
        assert!(!v.choices[0].selected);
        assert!(v.choices[1].selected);
    }
    #[test]
    fn multi_question_back_preserves_text_and_selection() {
        let mut c = client();
        c.choose(1);
        c.input().insert("Custom clarification");
        c.submit(0, false);
        assert_eq!(c.view(false).meta, "Вопрос 2 из 2");
        c.back();
        let v = c.view(false);
        assert_eq!(v.field.text, "Custom clarification");
        assert!(v.choices[1].selected);
        assert!(!c.busy);
    }
    #[test]
    fn compose_drafts_survive_changing_threads() {
        let mut c = client();
        c.compose();
        c.input().insert("For Alpha");
        c.picker = true;
        c.choose_thread(1);
        c.input().insert("For Beta");
        c.picker = true;
        c.choose_thread(0);
        c.compose();
        assert_eq!(c.view(false).field.text, "For Alpha");
        c.picker = true;
        c.choose_thread(1);
        assert_eq!(c.view(false).field.text, "For Beta");
    }
    #[test]
    fn poll_during_send_does_not_retarget_or_delete_another_draft() {
        let mut c = client();
        c.input().insert("reply");
        c.busy = true;
        let mut next = data();
        next["interactions"] = json!([]);
        c.absorb(next.clone(), String::new(), false);
        assert_eq!(c.interaction.as_deref(), Some("i1"));
        assert_eq!(c.view(false).field.text, "reply");
        *c.result.lock().unwrap() = Some(Ok(json!({"ok":true})));
        c.absorb(next, String::new(), false);
        assert!(!c.busy);
        assert!(c.interaction.is_none());
        assert!(!c.drafts.contains_key("i1"));
    }
    #[test]
    fn failed_answer_preserves_selected_option_and_text() {
        let mut c = client();
        c.choose(0);
        c.input().insert("Keep me");
        c.busy = true;
        *c.result.lock().unwrap() = Some(Err("Offline".into()));
        c.absorb(data(), String::new(), false);
        assert!(c.view(false).choices[0].selected);
        assert_eq!(c.view(false).field.text, "Keep me");
    }
    #[test]
    fn empty_answer_does_not_start_a_request() {
        let mut c = client();
        c.submit(0, false);
        assert!(!c.busy);
        assert!(c.view(false).status.contains("Выберите"));
    }
    #[test]
    fn picker_filter_and_scroll_use_the_visible_thread() {
        let mut c = client();
        c.toggle_picker();
        c.input().insert("Beta");
        c.edited();
        assert_eq!(c.view(true).threads.len(), 1);
        c.choose_thread(0);
        assert_eq!(c.thread_id, "b");
    }
    #[test]
    fn opening_an_alert_selects_the_pending_thread_without_losing_another_draft() {
        let mut c = client();
        c.choose_thread(1);
        c.input().insert("Beta draft");
        c.absorb(data(), String::new(), false);
        assert_eq!(c.thread_id, "b");
        c.absorb(data(), String::new(), true);
        assert_eq!(c.thread_id, "a");
        assert_eq!(c.view(false).choices.len(), 2);
        c.choose_thread(1);
        assert_eq!(c.view(false).field.text, "Beta draft");
    }
    #[test]
    fn recovered_connection_clears_its_error_without_discarding_a_reply() {
        let mut c = client();
        c.input().insert("Keep this");
        c.absorb(data(), "Connection unavailable".into(), false);
        assert_eq!(c.view(false).status, "Connection unavailable");
        c.absorb(data(), String::new(), false);
        assert!(c.view(false).status.is_empty());
        assert_eq!(c.view(false).field.text, "Keep this");
    }
}
