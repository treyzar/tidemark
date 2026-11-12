//! Scratch note and typed reminders, backed by `tidemark_core::notes`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use tidemark_core::notes::{self, Reminder};

use crate::model;
use crate::ui::{card, icon_button, muted, page_header, section_title, text_button, Shared};

pub struct NotesView {
    pub root: gtk::Box,
    shared: Shared,
    buffer: gtk::TextBuffer,
    reminders: gtk::Box,
    entry: gtk::Entry,
    status: gtk::Label,
    debounce: Rc<RefCell<Option<glib::SourceId>>>,
}

impl NotesView {
    pub fn build(shared: Shared) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("tidemark-page");
        root.append(&page_header(
            "Заметки",
            Some("Черновик и напоминания сохраняются на диск"),
        ));

        let buffer = gtk::TextBuffer::new(None);
        buffer.set_text(&shared.notes.borrow().note);
        let view = gtk::TextView::with_buffer(&buffer);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_top_margin(8);
        view.set_bottom_margin(8);
        view.set_left_margin(8);
        view.set_right_margin(8);
        view.set_size_request(-1, 150);
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scrolled.set_child(Some(&view));
        root.append(&scrolled);

        let hint = muted("Заметка сохраняется автоматически, пока остров открыт.");
        root.append(&hint);

        let reminders_card = card();
        reminders_card.append(&section_title("Напоминания"));
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some(model::REMINDER_HINT));
        reminders_card.append(&entry);
        let add = text_button("Добавить напоминание");
        reminders_card.append(&add);
        let reminders = gtk::Box::new(gtk::Orientation::Vertical, 2);
        reminders_card.append(&reminders);
        let status = gtk::Label::new(Some(""));
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.add_css_class("tiny");
        reminders_card.append(&status);
        root.append(&reminders_card);

        let debounce = Rc::new(RefCell::new(None));
        {
            let shared = shared.clone();
            let debounce = debounce.clone();
            buffer.connect_changed(move |buffer| {
                if let Some(source) = debounce.borrow_mut().take() {
                    source.remove();
                }
                let shared = shared.clone();
                let buffer = buffer.clone();
                let id = glib::timeout_add_local_once(Duration::from_millis(600), move || {
                    let text = buffer
                        .text(&buffer.start_iter(), &buffer.end_iter(), false)
                        .to_string();
                    shared.notes.borrow_mut().note = text;
                    shared.notes.borrow().save();
                });
                *debounce.borrow_mut() = Some(id);
            });
        }

        {
            let shared = shared.clone();
            let entry = entry.clone();
            let reminders = reminders.clone();
            let status = status.clone();
            add.connect_clicked(move |_| {
                add_reminder(&shared, &entry, &reminders, &status);
            });
            entry.connect_activate(move |entry| {
                add_reminder(&shared, entry, &reminders, &status);
            });
        }

        let view = Self {
            root,
            shared,
            buffer,
            reminders,
            entry,
            status,
            debounce,
        };
        view.render_reminders();
        view
    }

    pub fn refresh(&self) {
        self.render_reminders();
    }

    pub fn save_now(&self) {
        if let Some(source) = self.debounce.borrow_mut().take() {
            source.remove();
        }
        self.shared.notes.borrow_mut().note = self
            .buffer
            .text(&self.buffer.start_iter(), &self.buffer.end_iter(), false)
            .to_string();
        self.shared.notes.borrow().save();
    }

    /// Removes due reminders, persists the change and returns them so the app
    /// can raise one desktop notification per reminder.
    pub fn take_due(&self) -> Vec<Reminder> {
        let due = {
            let mut store = self.shared.notes.borrow_mut();
            let due = store.take_due(Shared::now_secs());
            if !due.is_empty() {
                store.save();
            }
            due
        };
        if !due.is_empty() {
            self.render_reminders();
        }
        due
    }

    fn render_reminders(&self) {
        while let Some(child) = self.reminders.first_child() {
            self.reminders.remove(&child);
        }
        let now = Shared::now_secs();
        let since = local_seconds_since_midnight();
        let reminders = self.shared.notes.borrow().reminders.clone();
        if reminders.is_empty() {
            self.reminders.append(&muted("Напоминаний нет."));
            return;
        }
        for (index, reminder) in reminders.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.add_css_class("tidemark-row");
            let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
            texts.set_hexpand(true);
            let when = gtk::Label::new(Some(&notes::when_label(reminder.at, now, since)));
            when.set_xalign(0.0);
            when.add_css_class("tiny");
            let text = gtk::Label::new(Some(if reminder.text.is_empty() {
                "Напоминание"
            } else {
                &reminder.text
            }));
            text.set_xalign(0.0);
            text.set_wrap(true);
            texts.append(&when);
            texts.append(&text);
            row.append(&texts);

            let remove = icon_button("user-trash-symbolic", "Удалить напоминание");
            let shared = self.shared.clone();
            let reminders = self.reminders.clone();
            remove.connect_clicked(move |_| {
                let mut store = shared.notes.borrow_mut();
                if index < store.reminders.len() {
                    store.reminders.remove(index);
                    store.save();
                }
                drop(store);
                render_rows(&shared, &reminders);
            });
            row.append(&remove);
            self.reminders.append(&row);
        }
    }

    pub fn focus_entry(&self) {
        self.entry.grab_focus();
    }

    pub fn set_status_text(&self, text: &str, error: bool) {
        self.status.remove_css_class("danger");
        if error {
            self.status.add_css_class("danger");
        }
        self.status.set_text(text);
    }
}

fn render_rows(shared: &Shared, widget: &gtk::Box) {
    // Delegating through a temporary view would need the widget tree; the
    // small duplicated traversal keeps delete handlers independent of `self`.
    while let Some(child) = widget.first_child() {
        widget.remove(&child);
    }
    let now = Shared::now_secs();
    let since = local_seconds_since_midnight();
    let reminders = shared.notes.borrow().reminders.clone();
    if reminders.is_empty() {
        widget.append(&muted("Напоминаний нет."));
        return;
    }
    for (index, reminder) in reminders.iter().enumerate() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.add_css_class("tidemark-row");
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
        texts.set_hexpand(true);
        let when = gtk::Label::new(Some(&notes::when_label(reminder.at, now, since)));
        when.set_xalign(0.0);
        when.add_css_class("tiny");
        let text = gtk::Label::new(Some(&reminder.text));
        text.set_xalign(0.0);
        text.set_wrap(true);
        texts.append(&when);
        texts.append(&text);
        row.append(&texts);
        let remove = icon_button("user-trash-symbolic", "Удалить напоминание");
        let shared = shared.clone();
        let widget = widget.clone();
        remove.connect_clicked(move |_| {
            let mut store = shared.notes.borrow_mut();
            if index < store.reminders.len() {
                store.reminders.remove(index);
                store.save();
            }
            drop(store);
            render_rows(&shared, &widget);
        });
        row.append(&remove);
        widget.append(&row);
    }
}

fn add_reminder(shared: &Shared, entry: &gtk::Entry, list: &gtk::Box, status: &gtk::Label) {
    let text = entry.text().to_string();
    if text.trim().is_empty() {
        show(status, "Введите фразу с временем.", true);
        return;
    }
    let now = Shared::now_secs();
    let since = local_seconds_since_midnight();
    match notes::parse(&text, now, since) {
        Some((at, reminder_text)) => {
            shared.notes.borrow_mut().add(Reminder {
                at,
                text: reminder_text,
            });
            shared.notes.borrow().save();
            entry.set_text("");
            render_rows(shared, list);
            show(status, "Напоминание добавлено.", false);
        }
        None => show(status, model::REMINDER_EXAMPLES, true),
    }
}

fn show(label: &gtk::Label, text: &str, error: bool) {
    label.remove_css_class("danger");
    if error {
        label.add_css_class("danger");
    }
    label.set_text(text);
}

fn local_seconds_since_midnight() -> u64 {
    glib::DateTime::now_local()
        .map(|dt| (dt.hour().max(0) * 3600 + dt.minute().max(0) * 60 + dt.second().max(0)) as u64)
        .unwrap_or(0)
}
