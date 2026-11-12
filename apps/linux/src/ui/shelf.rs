//! File references with native GTK file drag-and-drop. No file copies/deletions.
use crate::ui::{page_header, text_button, Shared};
use gtk::{gio, prelude::*};
use std::{cell::RefCell, rc::Rc};
#[path = "../services/shelf.rs"]
mod state;
pub struct ShelfView {
    pub root: gtk::Box,
    inner: Rc<Inner>,
}
struct Inner {
    shared: Shared,
    model: RefCell<state::Shelf>,
    list: gtk::Box,
    status: gtk::Label,
}
impl ShelfView {
    pub fn build(shared: Shared) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("tidemark-page");
        root.append(&page_header(
            "Полка файлов",
            Some("Перетащите файлы сюда, затем из полки в другое приложение"),
        ));
        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        status.add_css_class("tiny");
        let model = if shared.smoke {
            Ok(state::Shelf::default())
        } else {
            state::Shelf::load(&Self::path())
        };
        if model.is_err() {
            status.set_text("Не удалось прочитать полку. Существующий файл не изменён");
        }
        let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let inner = Rc::new(Inner {
            shared,
            model: RefCell::new(model.unwrap_or_default()),
            list,
            status,
        });
        let clear = text_button("Очистить полку");
        clear.set_tooltip_text(Some("Убрать только ссылки; файлы останутся на диске"));
        clear.set_sensitive(!inner.shared.smoke);
        let weak = Rc::downgrade(&inner);
        clear.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.store(state::Shelf::default());
            }
        });
        root.append(&clear);
        root.append(&inner.list);
        root.append(&inner.status);
        inner.render();
        let view = Self { root, inner };
        view.root.add_controller(view.drop_target());
        view
    }
    fn path() -> std::path::PathBuf {
        tidemark_core::config::Config::data_dir().join("shelf.json")
    }
    pub fn drop_target(&self) -> gtk::DropTarget {
        let target = gtk::DropTarget::new(
            gtk::gdk::FileList::static_type(),
            gtk::gdk::DragAction::COPY,
        );
        let weak = Rc::downgrade(&self.inner);
        target.connect_drop(move |_, value, _, _| {
            let Some(inner) = weak.upgrade() else {
                return false;
            };
            if inner.shared.smoke {
                return false;
            }
            let Ok(files) = value.get::<gtk::gdk::FileList>() else {
                return false;
            };
            let uris: Vec<_> = files
                .files()
                .into_iter()
                .filter(|file| file.path().is_some_and(|path| path.is_absolute()))
                .take(20)
                .map(|file| file.uri().to_string())
                .collect();
            if uris.is_empty() {
                inner.status.set_text("Полка принимает локальные файлы");
                return false;
            }
            let mut next = inner.model.borrow().clone();
            next.add(&uris);
            inner.store(next)
        });
        target
    }
}
impl Inner {
    fn store(self: &Rc<Self>, next: state::Shelf) -> bool {
        if self.shared.smoke {
            return false;
        }
        if next.save(&ShelfView::path()).is_err() {
            self.status.set_text("Не удалось сохранить полку");
            return false;
        }
        self.model.replace(next);
        self.status
            .set_text("Ссылки сохранены; исходные файлы не менялись");
        self.render();
        true
    }
    fn render(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        if self.model.borrow().files.is_empty() {
            self.list
                .append(&gtk::Label::new(Some("Перетащите сюда до 20 файлов")));
        }
        for uri in self.model.borrow().files.clone() {
            let file = gio::File::for_uri(&uri);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.add_css_class("tile");
            let name = file
                .basename()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Файл".into());
            let label = gtk::Label::new(Some(&name));
            label.set_xalign(0.0);
            label.set_hexpand(true);
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            label.set_tooltip_text(Some(&uri));
            row.append(&label);
            let drag = gtk::DragSource::new();
            drag.set_actions(gtk::gdk::DragAction::COPY);
            let file = file.clone();
            drag.connect_prepare(move |_, _, _| {
                Some(gtk::gdk::ContentProvider::for_value(
                    &gtk::gdk::FileList::from_array(&[file.clone()]).to_value(),
                ))
            });
            row.add_controller(drag);
            let open = text_button("Открыть");
            open.set_sensitive(!self.shared.smoke);
            let open_uri = uri.clone();
            let status = self.status.clone();
            open.connect_clicked(move |_| {
                if gio::AppInfo::launch_default_for_uri(&open_uri, None::<&gio::AppLaunchContext>)
                    .is_err()
                {
                    status.set_text("Не удалось открыть файл; проверьте, доступен ли он");
                }
            });
            row.append(&open);
            let remove = gtk::Button::from_icon_name("window-close-symbolic");
            remove.set_sensitive(!self.shared.smoke);
            remove.set_tooltip_text(Some("Убрать ссылку, сохранив файл"));
            let weak = Rc::downgrade(self);
            remove.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    let mut next = inner.model.borrow().clone();
                    next.files.retain(|old| old != &uri);
                    inner.store(next);
                }
            });
            row.append(&remove);
            self.list.append(&row);
        }
    }
}
