//! Native cards for local coding-agent notices and explicit questions.
use crate::services::agent::{Card, CardId, Service};
use gtk::prelude::*;
use std::{
    cell::{Cell, RefCell},
    time::Instant,
};

pub struct AgentView {
    pub root: gtk::Box,
    service: RefCell<Service>,
    current: RefCell<Option<CardId>>,
    expiry: RefCell<Option<(Instant, gtk::Label)>>,
    preview: bool,
    buttons: RefCell<Vec<gtk::Button>>,
    checked_layout: Cell<bool>,
}
impl AgentView {
    pub fn build(preview: bool) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.add_css_class("tile");
        root.add_css_class("agent-card");
        root.set_visible(false);
        Self {
            root,
            service: RefCell::new(Service::default()),
            current: RefCell::new(None),
            expiry: RefCell::new(None),
            preview,
            buttons: RefCell::new(Vec::new()),
            checked_layout: Cell::new(false),
        }
    }
    pub fn attach(&self, service: Service) {
        self.service.replace(service);
        self.current.replace(None);
    }
    pub fn refresh(&self) -> bool {
        let card = self.service.borrow().poll();
        let id = card.as_ref().map(|card| card.id);
        if *self.current.borrow() != id {
            self.current.replace(id);
            while let Some(child) = self.root.first_child() {
                self.root.remove(&child);
            }
            self.expiry.replace(None);
            self.buttons.borrow_mut().clear();
            self.checked_layout.set(false);
            if let Some(card) = card {
                self.render(card);
            }
            self.root.set_visible(id.is_some());
        } else if id.is_some()
            && !self.checked_layout.get()
            && self.root.width() > 0
            && self.root.height() > 0
        {
            self.checked_layout.set(true);
            // Never leave approval available beside text that would force the
            // card outside the island. Oversized rendering falls back to the
            // terminal; the message itself is never abbreviated for approval.
            if self.root.height() > 420 {
                if let Some(id) = id {
                    self.service.borrow().choose(id, None);
                }
            } else if !self.preview {
                for button in self.buttons.borrow().iter() {
                    button.set_sensitive(true);
                }
            }
        }
        if let Some((deadline, label)) = self.expiry.borrow().as_ref() {
            let left = deadline.saturating_duration_since(Instant::now()).as_secs() + 1;
            label.set_text(&format!("Осталось {}:{:02}", left / 60, left % 60));
        }
        id.is_some()
    }
    fn render(&self, card: Card) {
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let source = plain(&card.msg.source);
        source.add_css_class("tidemark-section");
        source.set_hexpand(true);
        header.append(&source);
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.set_tooltip_text(Some("Закрыть без ответа; продолжить в терминале"));
        let service = self.service.borrow().clone();
        let id = card.id;
        close.connect_clicked(move |_| {
            service.choose(id, None);
        });
        header.append(&close);
        self.root.append(&header);
        let title = plain(&card.msg.title);
        title.add_css_class("tile-title");
        self.root.append(&title);
        self.root.append(&plain(&card.msg.text));
        if matches!(card.id, CardId::Ask(_)) {
            let buttons = gtk::FlowBox::new();
            buttons.set_selection_mode(gtk::SelectionMode::None);
            buttons.set_min_children_per_line(1);
            buttons.set_max_children_per_line(2);
            buttons.set_row_spacing(6);
            buttons.set_column_spacing(6);
            for (index, option) in card.msg.options.iter().enumerate() {
                let button = gtk::Button::new();
                let label = plain(option);
                label.set_xalign(0.5);
                button.set_child(Some(&label));
                button.set_sensitive(false);
                self.buttons.borrow_mut().push(button.clone());
                let service = self.service.borrow().clone();
                let id = card.id;
                button.connect_clicked(move |_| {
                    service.choose(id, Some(index));
                });
                buttons.insert(&button, -1);
            }
            self.root.append(&buttons);
            let hint=plain("Ответ отправляется только выбранной кнопкой. Закрытие оставляет вопрос в терминале.");
            hint.add_css_class("tiny");
            self.root.append(&hint);
        }
        let expiry = plain("");
        expiry.add_css_class("tiny");
        self.root.append(&expiry);
        self.expiry.replace(Some((card.expires, expiry)));
    }
    pub fn dismiss(&self) -> bool {
        self.current
            .borrow()
            .is_some_and(|id| self.service.borrow().choose(id, None))
    }
    pub fn preview(&self) {
        self.service.borrow().preview();
        self.refresh();
    }
    pub fn clear(&self) {
        self.service.borrow().shutdown();
        self.refresh();
    }
}
fn plain(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_use_markup(false);
    label.set_use_underline(false);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_xalign(0.0);
    label
}
