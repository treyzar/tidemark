//! Clipboard history tab. Background capture belongs to the GNOME adapter;
//! this page can always add the text currently in the clipboard while the
//! island has focus, and it never pretends that a failed operation worked.

use crate::services::shell::state::{SaveQueue, SaveSnapshot};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use gtk::gdk;
use gtk::glib;
use gtk::glib::prelude::ToValue;
use gtk::prelude::*;
use tidemark_core::clip_history::{self, Entry, Glyph};

use crate::services::shell::{Command, Peer, ReadKey};
use crate::services::{clipboard as clipboard_service, file_manager};
use crate::ui::{card, icon_button, muted, page_header, scroll, text_button, Shared};
use tidemark_core::shell::{Capability, ShellError};

pub struct ClipboardView {
    pub root: gtk::Box,
    shared: Shared,
    list: gtk::Box,
    status: gtk::Label,
    add: gtk::Button,
    clear: gtk::Button,
    mode: gtk::Label,
    offer: RefCell<Option<ReadKey>>,
    write_serial: Cell<u64>,
    pending_write: RefCell<Option<(u64, Peer)>>,
    me: RefCell<Option<Weak<Self>>>,
    saves: Option<Arc<SaveQueue>>,
    save_worker: RefCell<Option<JoinHandle<()>>>,
    save_serial: Cell<u64>,
    saving: Cell<bool>,
}

impl Drop for ClipboardView {
    fn drop(&mut self) {
        if let Some(queue) = &self.saves {
            queue.close();
        }
        if let Some(worker) = self.save_worker.get_mut().take() {
            let _ = worker.join();
        }
    }
}

impl ClipboardView {
    pub fn build(shared: Shared) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("tidemark-page");
        root.append(&page_header(
            "Буфер обмена",
            Some("Текст, картинки и файлы, сохранённые локально"),
        ));

        let mode =
            muted("Подключение фоновой истории GNOME… Ручное добавление доступно при фокусе.");
        root.append(&mode);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let add = text_button("Добавить текущий текст");
        let clear = text_button("Очистить");
        actions.append(&add);
        actions.append(&clear);
        root.append(&actions);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let scrolled = scroll(&list);
        root.append(&scrolled);

        let status = gtk::Label::new(Some(""));
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.add_css_class("tiny");
        root.append(&status);

        let queue = Arc::new(SaveQueue::default());
        let worker_queue = queue.clone();
        let worker = if shared.smoke {
            None
        } else {
            std::thread::Builder::new()
                .name("clipboard-history-save".into())
                .spawn(move || {
                    let mut deferred_cleanup = Vec::new();
                    while let Some(job) = worker_queue.take() {
                        for name in &job.dropped {
                            if !deferred_cleanup.contains(name) {
                                deferred_cleanup.push(name.clone());
                            }
                        }
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let result = tidemark_core::paths::write_atomic(
                                &clip_history::dir().join("history.json"),
                                job.json.as_bytes(),
                            )
                            .map_err(|error| error.to_string());
                            if result.is_ok() {
                                // A failed earlier write must not lose cleanup, and a
                                // restored image must not be removed by an older snapshot.
                                let history: clip_history::History =
                                    serde_json::from_str(&job.json)
                                        .map_err(|error| error.to_string())?;
                                deferred_cleanup.retain(|name| {
                                    !history.items.iter().any(|item|
                                    matches!(&item.entry, Entry::Image { file, .. } if file == name)
                                )
                                });
                                delete_images(&deferred_cleanup);
                                deferred_cleanup.clear();
                            }
                            result
                        }))
                        .unwrap_or_else(|_| Err("Поток сохранения прерван".into()));
                        worker_queue.complete(job.serial, result);
                    }
                })
                .ok()
        };
        let saves = worker.as_ref().map(|_| queue);
        let view = Rc::new(Self {
            root,
            shared,
            list,
            status,
            add,
            clear,
            mode,
            offer: RefCell::new(None),
            write_serial: Cell::new(0),
            pending_write: RefCell::new(None),
            me: RefCell::new(None),
            saves,
            save_worker: RefCell::new(worker),
            save_serial: Cell::new(0),
            saving: Cell::new(false),
        });
        view.me.replace(Some(Rc::downgrade(&view)));

        let weak = Rc::downgrade(&view);
        view.add.clone().connect_clicked(move |_| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            let target = view.clone();
            clipboard_service::read_text(move |text| target.add_current(text));
        });

        let weak = Rc::downgrade(&view);
        view.clear.clone().connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.clear_unpinned();
            }
        });

        view.rebuild();
        view
    }

    pub fn refresh(&self) {
        self.update_mode();
        self.rebuild();
    }
    fn update_mode(&self) {
        self.mode
            .set_text(if !self.shared.config.borrow().clipboard_history {
                "Фоновая история выключена. Текст можно добавить вручную при фокусе."
            } else if self.shared.shell_peer.borrow().is_some()
                && self
                    .shared
                    .shell_caps
                    .borrow()
                    .contains(&Capability::ClipboardRead)
            {
                "История текста и списков локальных файлов подключена к GNOME. Секретные данные не читаются."
            } else {
                "GNOME не подключён: только ручное добавление текста при фокусе."
            });
    }
    pub fn context_changed(&self) {
        self.offer.borrow_mut().take();
        self.pending_write.borrow_mut().take();
        self.list.set_sensitive(true);
        self.update_mode();
    }
    pub fn offer_changed(&self, key: Option<ReadKey>, sensitive: bool) {
        self.offer.replace(key);
        self.update_mode();
        if sensitive {
            self.set_status("Секретное содержимое не читается и не сохраняется.", false);
        }
    }
    pub fn receive_text(&self, key: ReadKey, text: String) {
        self.receive_entry(key, Entry::Text { text });
    }
    pub fn receive_files(&self, key: ReadKey, paths: Vec<String>) {
        self.receive_entry(key, Entry::Files { paths });
    }
    fn receive_entry(&self, key: ReadKey, entry: Entry) {
        if self.shared.smoke
            || !self.shared.config.borrow().clipboard_history
            || !self.shared.shell.borrow().accepts_read(&key)
            || !self
                .shared
                .shell_caps
                .borrow()
                .contains(&Capability::ClipboardRead)
            || self.offer.borrow().as_ref() != Some(&key)
            || self.shared.shell_peer.borrow().as_ref() != Some(&key.peer)
        {
            return;
        }
        let dropped = self
            .shared
            .clip
            .borrow_mut()
            .push(entry, Shared::now_secs());
        self.rebuild();
        self.persist(dropped);
    }
    /// Persistence fence: invoke before a synchronous Shared::save or handover.
    /// No GTK access occurs on the save worker; this drains all earlier writes.
    pub fn flush_persistence(&self) {
        if let Some(queue) = &self.saves {
            queue.flush();
        }
    }

    fn persist(&self, dropped: Vec<String>) {
        let json = match serde_json::to_string(&*self.shared.clip.borrow()) {
            Ok(json) => json,
            Err(error) => {
                self.set_status(&format!("История только в памяти: {error}"), true);
                return;
            }
        };
        let serial = self.save_serial.get().wrapping_add(1);
        self.save_serial.set(serial);
        let sent = self.saves.as_ref().is_some_and(|queue| {
            queue.submit(SaveSnapshot {
                serial,
                json,
                dropped,
            })
        });
        if !sent {
            self.set_status("История только в памяти: сохранение недоступно", true);
            return;
        }
        self.set_status("Изменение в памяти; сохраняем историю…", false);
        if self.saving.replace(true) {
            return;
        }
        let Some(view) = self.weak().upgrade() else {
            return;
        };
        let hold = gtk::gio::Application::default().map(|app| app.hold());
        glib::timeout_add_local(Duration::from_millis(50), move || {
            let _keep_alive = &hold;
            while let Some((finished, result)) =
                view.saves.as_ref().and_then(|queue| queue.result())
            {
                if finished == view.save_serial.get() {
                    view.saving.set(false);
                    match result {
                        Ok(()) => view.set_status("История сохранена локально.", false),
                        Err(error) => {
                            view.set_status(&format!("История только в памяти: {error}"), true)
                        }
                    }
                    return glib::ControlFlow::Break;
                }
            }
            glib::ControlFlow::Continue
        });
    }
    pub fn written(&self, request: u64, peer: Peer, result: Result<(), ShellError>) {
        if self.pending_write.borrow().as_ref() != Some(&(request, peer.clone())) {
            return;
        }
        self.pending_write.borrow_mut().take();
        self.list.set_sensitive(true);
        if self.shared.shell_peer.borrow().as_ref() != Some(&peer) {
            return;
        }
        match result {
            Ok(()) => self.set_status("GNOME подтвердил запись текста в буфер.", false),
            Err(error) => self.set_status(&format!("Запись не подтверждена: {error}"), true),
        }
    }

    fn add_current(&self, text: Option<String>) {
        let Some(text) = text else {
            self.set_status("Не удалось прочитать буфер обмена.", true);
            return;
        };
        if text.chars().count() > clip_history::MAX_TEXT_CHARS {
            self.set_status("Текст превышает допустимый размер истории.", true);
            return;
        }
        if text.trim().is_empty() {
            self.set_status("В буфере нет текста.", true);
            return;
        }
        let dropped = {
            let mut history = self.shared.clip.borrow_mut();
            history.push(Entry::Text { text }, Shared::now_secs())
        };
        self.rebuild();
        self.persist(dropped);
    }

    fn clear_unpinned(&self) {
        let dropped = {
            let mut history = self.shared.clip.borrow_mut();
            if history.items.iter().all(|item| item.pinned) {
                None
            } else {
                Some(history.clear())
            }
        };
        let Some(dropped) = dropped else {
            self.set_status("Нет незакреплённых записей.", false);
            return;
        };
        self.rebuild();
        self.persist(dropped);
    }

    fn rebuild(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let now = Shared::now_secs();
        let items = self.shared.clip.borrow().items.clone();
        if items.is_empty() {
            self.list.append(&muted(
                "История пуста. Скопируйте текст и нажмите «Добавить текущий текст».",
            ));
            return;
        }
        for (index, item) in items.iter().enumerate() {
            self.list.append(&self.row(index, item, now));
        }
    }

    fn row(&self, _index: usize, item: &clip_history::Item, now: u64) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("tidemark-row");

        let (glyph, label) = clip_history::label(&item.entry);
        let icon = gtk::Image::from_icon_name(match glyph {
            Glyph::Text => "text-x-generic-symbolic",
            Glyph::Link => "web-browser-symbolic",
            Glyph::Image => "image-x-generic-symbolic",
            Glyph::Files => "folder-symbolic",
        });
        icon.set_pixel_size(20);
        row.append(&icon);

        let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
        texts.set_hexpand(true);
        let main = gtk::Label::new(Some(&label));
        main.set_xalign(0.0);
        main.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let mut detail = clip_history::ago(item.copied_at, now);
        if item.pinned {
            detail.push_str(" · закреплено");
        }
        if let Entry::Files { paths } = &item.entry {
            detail.push_str(&format!(" · {}", paths.join(" · ")));
        }
        let sub = gtk::Label::new(Some(&detail));
        sub.set_xalign(0.0);
        sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
        sub.add_css_class("muted");
        texts.append(&main);
        texts.append(&sub);
        row.append(&texts);

        let copy = icon_button("edit-copy-symbolic", "Скопировать");
        let weak = self.weak();
        let entry = item.entry.clone();
        copy.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                if let Some(index) = view.index_of(&entry) {
                    view.copy(index);
                }
            }
        });
        row.append(&copy);

        let pin = icon_button(
            if item.pinned {
                "starred-symbolic"
            } else {
                "non-starred-symbolic"
            },
            if item.pinned {
                "Снять закрепление"
            } else {
                "Закрепить"
            },
        );
        let weak = self.weak();
        let entry = item.entry.clone();
        pin.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                if let Some(index) = view.index_of(&entry) {
                    view.toggle_pin(index);
                }
            }
        });
        row.append(&pin);

        if matches!(item.entry, Entry::Files { .. }) {
            let reveal = icon_button("folder-open-symbolic", "Показать в папке");
            let weak = self.weak();
            let entry = item.entry.clone();
            reveal.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    if let Some(index) = view.index_of(&entry) {
                        view.reveal(index);
                    }
                }
            });
            row.append(&reveal);
        }

        let remove = icon_button("user-trash-symbolic", "Удалить запись");
        let weak = self.weak();
        let entry = item.entry.clone();
        remove.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                if let Some(index) = view.index_of(&entry) {
                    view.remove(index);
                }
            }
        });
        row.append(&remove);

        row
    }

    fn index_of(&self, entry: &Entry) -> Option<usize> {
        self.shared
            .clip
            .borrow()
            .items
            .iter()
            .position(|item| &item.entry == entry)
    }
    fn weak(&self) -> Weak<Self> {
        self.me
            .borrow()
            .clone()
            .expect("view is initialized before rows are built")
    }

    fn copy(&self, index: usize) {
        if self.shared.smoke {
            return;
        }
        let entry = self
            .shared
            .clip
            .borrow()
            .items
            .get(index)
            .map(|item| item.entry.clone());
        let Some(entry) = entry else {
            self.set_status("Запись больше не существует.", true);
            return;
        };
        match entry {
            Entry::Text { text } => {
                if self.shared.smoke || self.pending_write.borrow().is_some() {
                    return;
                }
                if let Some(peer) = self.shared.shell_peer.borrow().clone().filter(|_| {
                    self.shared
                        .shell_caps
                        .borrow()
                        .contains(&Capability::ClipboardWrite)
                }) {
                    let request = self.write_serial.get().wrapping_add(1);
                    self.write_serial.set(request);
                    if self.shared.shell.borrow().send(Command::SetClipboard {
                        request,
                        peer: peer.clone(),
                        text,
                    }) {
                        self.pending_write.replace(Some((request, peer)));
                        self.list.set_sensitive(false);
                        self.set_status("Ожидаем подтверждения записи GNOME…", false);
                    } else {
                        self.set_status("Мост GNOME занят; текст не отправлен.", true);
                    }
                    return;
                }
                if clipboard_service::write_text(&text) {
                    self.set_status("Текст скопирован.", false);
                } else {
                    self.set_status("Нет доступного дисплея для буфера.", true);
                }
            }
            Entry::Files { paths } => match copy_files(&paths) {
                Ok(()) => self.set_status("Список файлов скопирован.", false),
                Err(error) => self.set_status(&error, true),
            },
            Entry::Image { file, .. } => match copy_image(&file) {
                Ok(()) => self.set_status("Картинка скопирована.", false),
                Err(error) => self.set_status(&error, true),
            },
        }
    }

    fn toggle_pin(&self, index: usize) {
        let changed = self.shared.clip.borrow_mut().toggle_pin(index);
        if changed {
            self.rebuild();
            self.persist(Vec::new());
        }
    }

    fn remove(&self, index: usize) {
        let dropped = {
            let mut history = self.shared.clip.borrow_mut();
            if index >= history.items.len() {
                None
            } else {
                let item = history.items.remove(index);
                match item.entry {
                    Entry::Image { file, .. } => Some(vec![file]),
                    _ => Some(Vec::new()),
                }
            }
        };
        if let Some(dropped) = dropped {
            self.rebuild();
            self.persist(dropped);
        }
    }

    fn reveal(&self, index: usize) {
        let paths = self
            .shared
            .clip
            .borrow()
            .items
            .get(index)
            .and_then(|item| match &item.entry {
                Entry::Files { paths } => Some(paths.clone()),
                _ => None,
            });
        match paths {
            Some(paths) => match file_manager::reveal(&paths) {
                Ok(()) => self.set_status("Папка открыта в файловом менеджере.", false),
                Err(error) => self.set_status(&error, true),
            },
            None => self.set_status("В записи нет путей к файлам.", true),
        }
    }

    fn set_status(&self, text: &str, error: bool) {
        self.status.remove_css_class("danger");
        if error {
            self.status.add_css_class("danger");
        }
        self.status.set_text(text);
    }
}

fn copy_image(file: &str) -> Result<(), String> {
    let path = image_path(file).ok_or_else(|| "Некорректное имя файла картинки.".to_string())?;
    let texture = gdk::Texture::from_filename(&path)
        .map_err(|error| format!("Не удалось открыть картинку: {error}"))?;
    let Some(display) = gdk::Display::default() else {
        return Err("Нет доступного дисплея для буфера.".into());
    };
    let provider = gdk::ContentProvider::for_value(&texture.to_value());
    display
        .clipboard()
        .set_content(Some(&provider))
        .map_err(|error| format!("Не удалось записать картинку в буфер: {error}"))
}

fn copy_files(paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("В записи нет путей к файлам.".into());
    }
    let Some(display) = gdk::Display::default() else {
        return Err("Нет доступного дисплея для буфера.".into());
    };
    let lines: Vec<String> = paths
        .iter()
        .map(|path| crate::model::file_uri(path))
        .collect();
    let provider = gdk::ContentProvider::for_bytes(
        "text/uri-list",
        &glib::Bytes::from(lines.join("\r\n").as_bytes()),
    );
    display
        .clipboard()
        .set_content(Some(&provider))
        .map_err(|error| format!("Не удалось записать список файлов в буфер: {error}"))
}

fn image_path(file: &str) -> Option<std::path::PathBuf> {
    if !crate::services::shell::state::owned_image_name(file) {
        return None;
    }
    let path = clip_history::dir().join(file);
    path.is_file().then_some(path)
}

fn delete_images(files: &[String]) {
    for file in files {
        if let Some(path) = image_path(file) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[allow(dead_code)]
fn empty_card() -> gtk::Box {
    card()
}
