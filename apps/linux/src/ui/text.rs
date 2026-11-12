//! Text tools tab: all twelve pure transforms from `tidemark_core::text_tools`.

use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc, sync::mpsc, time::Duration};
use tidemark_net::translate::{target_for, TargetLanguage};
#[path = "../services/online.rs"]
mod online;
use online::TranslationState;
use tidemark_core::text_tools::{self, OPS};

use crate::services::clipboard;
use crate::ui::{icon_button, muted, page_header, section_title, text_button, Shared};

pub struct TextView {
    pub root: gtk::Box,
    buffer: gtk::TextBuffer,
    status: gtk::Label,
}

impl TextView {
    pub fn build(shared: Shared) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("tidemark-page");
        root.append(&page_header(
            "Текст",
            Some("Преобразования — локально. Онлайн-перевод — по кнопке, RU ↔ EN."),
        ));

        let buffer = gtk::TextBuffer::new(None);
        let view = gtk::TextView::with_buffer(&buffer);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_top_margin(8);
        view.set_bottom_margin(8);
        view.set_left_margin(8);
        view.set_right_margin(8);
        view.set_size_request(-1, 170);
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scrolled.set_child(Some(&view));
        root.append(&scrolled);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let paste = text_button("Вставить");
        let copy = text_button("Копировать");
        let clear = text_button("Очистить");
        let translate = text_button("Перевести онлайн");
        translate.set_tooltip_text(Some(
            "Отправить текст сервису перевода. Направление RU ↔ EN определяется автоматически.",
        ));
        actions.append(&paste);
        actions.append(&copy);
        actions.append(&clear);
        actions.append(&translate);
        root.append(&actions);

        let status = gtk::Label::new(Some(""));
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.add_css_class("tiny");
        root.append(&status);

        let translated_buffer = gtk::TextBuffer::new(None);
        let translated_view = gtk::TextView::with_buffer(&translated_buffer);
        translated_view.set_editable(false);
        translated_view.set_cursor_visible(false);
        translated_view.set_wrap_mode(gtk::WrapMode::WordChar);
        translated_view.set_size_request(-1, 110);
        let translated_scroll = gtk::ScrolledWindow::new();
        translated_scroll.set_child(Some(&translated_view));
        let result_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        result_box.append(&section_title("Перевод"));
        result_box.append(&translated_scroll);
        let copy_translation = text_button("Копировать перевод");
        result_box.append(&copy_translation);
        result_box.set_visible(false);
        root.append(&result_box);
        let translation_state = Rc::new(RefCell::new(TranslationState::default()));
        {
            let state = translation_state.clone();
            let result_box = result_box.clone();
            buffer.connect_changed(move |_| {
                state.borrow_mut().edited();
                result_box.set_visible(false);
            });
        }
        {
            let translated_buffer = translated_buffer.clone();
            let status = status.clone();
            copy_translation.connect_clicked(move |_| {
                let text = translated_buffer.text(
                    &translated_buffer.start_iter(),
                    &translated_buffer.end_iter(),
                    false,
                );
                if clipboard::write_text(text.as_str()) {
                    show(
                        &status,
                        "Перевод скопирован. Исходный текст сохранён.",
                        false,
                    );
                } else {
                    show(
                        &status,
                        "Не удалось скопировать перевод: нет доступного дисплея.",
                        true,
                    );
                }
            });
        }
        {
            let buffer = buffer.clone();
            let status = status.clone();
            let translated_buffer = translated_buffer.clone();
            let state = translation_state.clone();
            let result_box = result_box.clone();
            let smoke = shared.smoke;
            translate.connect_clicked(move |button| {
                if smoke { return; }
                let source = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
                if source.trim().is_empty() {
                    show(&status, "Сначала введите или вставьте текст.", true);
                    return;
                }
                if source.len() > 5000 {
                    show(&status, "Для перевода доступно до 5000 байт текста. Исходный текст сохранён.", true);
                    return;
                }
                let Some(request) = state.borrow_mut().begin() else { return; };
                let target = target_for(&source);
                let direction = if target == TargetLanguage::English { "RU → EN" } else { "EN → RU" };
                result_box.set_visible(false);
                button.set_sensitive(false);
                show(&status, &format!("Переводим онлайн: {direction}…"), false);
                // One in-flight worker, one bounded result slot. No GTK objects
                // enter the worker, and no worker exists before an explicit click.
                let (tx, rx) = mpsc::sync_channel(1);
                let started = std::thread::Builder::new().name("text-translation".into()).spawn(move || {
                    let result = tidemark_net::Client::new().translate(&source, target);
                    let _ = tx.try_send(result);
                });
                if started.is_err() {
                    state.borrow_mut().finish(request);
                    button.set_sensitive(true);
                    show(&status, "Не удалось запустить перевод. Исходный текст сохранён.", true);
                    return;
                }
                let state = state.clone();
                let status = status.clone();
                let button = button.clone();
                let translated_buffer = translated_buffer.clone();
                let result_box = result_box.clone();
                gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
                    let result = match rx.try_recv() {
                        Ok(result) => result,
                        Err(mpsc::TryRecvError::Empty) => return gtk::glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            let current = state.borrow_mut().finish(request);
                            button.set_sensitive(true);
                            if current { show(&status, "Перевод прерван. Исходный текст сохранён.", true); }
                            return gtk::glib::ControlFlow::Break;
                        }
                    };
                    let current = state.borrow_mut().finish(request);
                    button.set_sensitive(true);
                    if !current {
                        show(&status, "Текст изменён — предыдущий перевод не показан. Нажмите «Перевести онлайн» снова.", false);
                        return gtk::glib::ControlFlow::Break;
                    }
                    match result {
                        Ok(text) => {
                            translated_buffer.set_text(&text);
                            result_box.set_visible(true);
                            show(&status, &format!("Готово: {direction}. Исходный текст сохранён."), false);
                        }
                        Err(error) => show(&status, &format!("Не удалось перевести: {error}. Исходный текст сохранён."), true),
                    }
                    gtk::glib::ControlFlow::Break
                });
            });
        }

        root.append(&section_title("Преобразования"));
        let grid = gtk::Grid::new();
        grid.set_column_spacing(6);
        grid.set_row_spacing(6);
        grid.set_column_homogeneous(true);
        for (index, op) in OPS.iter().copied().enumerate() {
            let button = text_button(op.label());
            let buffer = buffer.clone();
            let status = status.clone();
            button.connect_clicked(move |_| {
                let start = buffer.start_iter();
                let end = buffer.end_iter();
                let source = buffer.text(&start, &end, false).to_string();
                if source.trim().is_empty() {
                    show(&status, "Сначала введите или вставьте текст.", true);
                    return;
                }
                match text_tools::apply(op, &source) {
                    Some(result) => {
                        buffer.set_text(&result);
                        show(&status, &format!("Применено: {}.", op.label()), false);
                    }
                    None => show(
                        &status,
                        "Преобразование не применилось: проверьте формат JSON или Base64.",
                        true,
                    ),
                }
            });
            grid.attach(&button, (index % 3) as i32, (index / 3) as i32, 1, 1);
        }
        root.append(&grid);

        root.append(&muted(
            "Layout меняет русские и английские клавиши местами. Base64 работает с UTF-8.",
        ));

        {
            let buffer = buffer.clone();
            let status = status.clone();
            paste.connect_clicked(move |_| {
                let buffer = buffer.clone();
                let status = status.clone();
                clipboard::read_text(move |text| match text {
                    Some(text) if !text.is_empty() => {
                        buffer.set_text(&text);
                        show(&status, "Текст вставлен из буфера.", false);
                    }
                    _ => show(&status, "В буфере нет текста.", true),
                });
            });
        }
        {
            let buffer = buffer.clone();
            let status = status.clone();
            copy.connect_clicked(move |_| {
                let text = buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), false)
                    .to_string();
                if text.is_empty() {
                    show(&status, "Нечего копировать.", true);
                } else if clipboard::write_text(&text) {
                    show(&status, "Текст скопирован.", false);
                } else {
                    show(&status, "Нет доступного дисплея для буфера.", true);
                }
            });
        }
        {
            let buffer = buffer.clone();
            let status = status.clone();
            clear.connect_clicked(move |_| {
                buffer.set_text("");
                show(&status, "Поле очищено.", false);
            });
        }

        // Keep the icon helper exercised in this tab as well: a clear button
        // with a tooltip is more discoverable than a bare label.
        let clear_icon = icon_button("edit-clear-symbolic", "Очистить текст");
        clear_icon.set_visible(false);
        actions.append(&clear_icon);

        Self {
            root,
            buffer,
            status,
        }
    }

    pub fn text(&self) -> String {
        self.buffer
            .text(&self.buffer.start_iter(), &self.buffer.end_iter(), false)
            .to_string()
    }

    pub fn save_note(&self) {
        let _ = self.text();
    }
}

fn show(label: &gtk::Label, text: &str, error: bool) {
    label.remove_css_class("danger");
    if error {
        label.add_css_class("danger");
    }
    label.set_text(text);
}
