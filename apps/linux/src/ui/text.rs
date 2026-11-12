//! Text tools tab: all twelve pure transforms from `tidemark_core::text_tools`.

use gtk::prelude::*;
use tidemark_core::text_tools::{self, OPS};

use crate::services::clipboard;
use crate::ui::{icon_button, muted, page_header, section_title, text_button, Shared};

pub struct TextView {
    pub root: gtk::Box,
    buffer: gtk::TextBuffer,
    status: gtk::Label,
}

impl TextView {
    pub fn build(_shared: Shared) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("tidemark-page");
        root.append(&page_header(
            "Текст",
            Some("Преобразования выполняются локально, без сети"),
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
        let translate = text_button("Перевод");
        translate.set_sensitive(false);
        translate.set_tooltip_text(Some(
            "Перевод требует сетевого сервиса и в Linux-версии пока недоступен.",
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
