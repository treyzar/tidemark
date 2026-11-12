//! Clipboard history tab. Background capture belongs to the GNOME adapter;
//! this page can always add the text currently in the clipboard while the
//! island has focus, and it never pretends that a failed operation worked.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::gdk;
use gtk::glib;
use gtk::glib::prelude::ToValue;
use gtk::prelude::*;
use tidemark_core::clip_history::{self, Entry, Glyph};

use crate::services::{clipboard as clipboard_service, file_manager};
use crate::ui::{card, icon_button, muted, page_header, scroll, text_button, Shared};

pub struct ClipboardView {
    pub root: gtk::Box,
    shared: Shared,
    list: gtk::Box,
    status: gtk::Label,
    add: gtk::Button,
    clear: gtk::Button,
    me: RefCell<Option<Weak<Self>>>,
}

impl ClipboardView {
    pub fn build(shared: Shared) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("tidemark-page");
        root.append(&page_header(
            "Буфер обмена",
            Some("Текст, картинки и файлы, сохранённые локально"),
        ));

        let history_on = shared.config.borrow().clipboard_history;
        root.append(&muted(if history_on {
            "Фоновая история требует адаптера GNOME Shell. Пока остров в фокусе, текст \
             можно добавить вручную."
        } else {
            "История выключена в настройках. Добавлять текст вручную всё равно можно."
        }));

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

        let view = Rc::new(Self {
            root,
            shared,
            list,
            status,
            add,
            clear,
            me: RefCell::new(None),
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
        self.rebuild();
    }

    fn add_current(&self, text: Option<String>) {
        let Some(text) = text else {
            self.set_status("Не удалось прочитать буфер обмена.", true);
            return;
        };
        if text.trim().is_empty() {
            self.set_status("В буфере нет текста.", true);
            return;
        }
        let dropped = {
            let mut history = self.shared.clip.borrow_mut();
            history.push(Entry::Text { text }, Shared::now_secs())
        };
        delete_images(&dropped);
        self.shared.clip.borrow().save();
        self.rebuild();
        self.set_status("Текущий текст добавлен в историю.", false);
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
        delete_images(&dropped);
        self.shared.clip.borrow().save();
        self.rebuild();
        self.set_status("Незакреплённые записи удалены.", false);
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

    fn row(&self, index: usize, item: &clip_history::Item, now: u64) -> gtk::Box {
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
        copy.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.copy(index);
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
        pin.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.toggle_pin(index);
            }
        });
        row.append(&pin);

        if matches!(item.entry, Entry::Files { .. }) {
            let reveal = icon_button("folder-open-symbolic", "Показать в папке");
            let weak = self.weak();
            reveal.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.reveal(index);
                }
            });
            row.append(&reveal);
        }

        let remove = icon_button("user-trash-symbolic", "Удалить запись");
        let weak = self.weak();
        remove.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.remove(index);
            }
        });
        row.append(&remove);

        row
    }

    fn weak(&self) -> Weak<Self> {
        self.me
            .borrow()
            .clone()
            .expect("view is initialized before rows are built")
    }

    fn copy(&self, index: usize) {
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
            self.shared.clip.borrow().save();
            self.rebuild();
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
            delete_images(&dropped);
            self.shared.clip.borrow().save();
            self.rebuild();
            self.set_status("Запись удалена.", false);
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
    display.clipboard().set_content(Some(&provider));
    Ok(())
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
    display.clipboard().set_content(Some(&provider));
    Ok(())
}

fn image_path(file: &str) -> Option<std::path::PathBuf> {
    if file.is_empty() || file.contains(['/', '\\']) || file == ".." {
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
