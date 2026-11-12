//! User-selected installed applications. Activation stays inside GIO.
use crate::ui::Shared;
use gtk::{gio, prelude::*};
use std::{cell::Cell, rc::Rc};
use tidemark_core::config::Config;
#[path = "../services/dock.rs"]
mod backend;

pub struct DockView {
    pub root: gtk::Box,
    inner: Rc<Inner>,
}
struct Inner {
    root: gtk::glib::WeakRef<gtk::Box>,
    shared: Shared,
    editing: Cell<bool>,
}
impl DockView {
    pub fn build(shared: Shared) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let inner = Rc::new(Inner {
            root: root.downgrade(),
            shared,
            editing: Cell::new(false),
        });
        inner.render();
        Self { root, inner }
    }
    pub fn refresh(&self) {
        self.inner.render();
    }
}
impl Inner {
    fn ids(&self) -> Vec<String> {
        let saved = self.shared.config.borrow().dock_apps.clone();
        backend::normalize(saved.unwrap_or_else(|| {
            [
                "org.gnome.Nautilus.desktop",
                "org.gnome.TextEditor.desktop",
                "org.gnome.Terminal.desktop",
                "org.gnome.SystemMonitor.desktop",
            ]
            .into_iter()
            .filter(|id| gio::DesktopAppInfo::new(id).is_some())
            .map(String::from)
            .collect()
        }))
    }
    fn store(self: &Rc<Self>, ids: Vec<String>) -> bool {
        let mut config = self.shared.config.borrow().clone();
        config.dock_apps = Some(backend::normalize(ids));
        if config.save_to(&Config::path()).is_err() {
            return false;
        }
        *self.shared.config.borrow_mut() = config;
        self.render();
        true
    }
    fn render(self: &Rc<Self>) {
        let Some(root) = self.root.upgrade() else {
            return;
        };
        while let Some(child) = root.first_child() {
            root.remove(&child);
        }
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_min_children_per_line(1);
        flow.set_max_children_per_line(5);
        flow.set_row_spacing(4);
        flow.set_column_spacing(4);
        let status = gtk::Label::new(None);
        status.set_wrap(true);
        status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        status.set_max_width_chars(1);
        status.set_xalign(0.0);
        status.add_css_class("tiny");
        status.set_visible(false);
        for id in self.ids() {
            let Some(app) = gio::DesktopAppInfo::new(&id) else {
                continue;
            };
            let button = gtk::Button::new();
            button.set_size_request(40, 40);
            if let Some(icon) = app.icon() {
                let image = gtk::Image::from_gicon(&icon);
                image.set_pixel_size(28);
                button.set_child(Some(&image));
            } else {
                button.set_icon_name("application-x-executable-symbolic");
            }
            let name = app.display_name();
            let label = if self.editing.get() {
                format!("Убрать {name}")
            } else {
                name.to_string()
            };
            button.set_tooltip_text(Some(&label));
            button.update_property(&[gtk::accessible::Property::Label(&label)]);
            button.set_cursor_from_name(Some("pointer"));
            let weak = Rc::downgrade(self);
            let status = status.clone();
            button.connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                if this.shared.smoke {
                    return;
                }
                if this.editing.get() {
                    let mut ids = this.ids();
                    ids.retain(|saved| saved != &id);
                    if !this.store(ids) {
                        status.set_text("Не удалось сохранить док");
                        status.set_visible(true);
                    }
                } else if app.launch(&[], None::<&gio::AppLaunchContext>).is_err() {
                    status.set_text("Не удалось открыть приложение");
                    status.set_visible(true);
                }
            });
            flow.insert(&button, -1);
        }
        let add = gtk::Button::from_icon_name("list-add-symbolic");
        add.set_tooltip_text(Some("Добавить приложение"));
        add.set_sensitive(!self.shared.smoke && self.ids().len() < backend::MAX_APPS);
        let weak = Rc::downgrade(self);
        let message = status.clone();
        add.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.choose(&message);
            }
        });
        flow.insert(&add, -1);
        let edit = gtk::ToggleButton::new();
        edit.set_icon_name("document-edit-symbolic");
        edit.set_tooltip_text(Some("Убрать приложения из дока"));
        edit.set_active(self.editing.get());
        edit.set_sensitive(!self.shared.smoke);
        let weak = Rc::downgrade(self);
        edit.connect_toggled(move |button| {
            if let Some(this) = weak.upgrade() {
                this.editing.set(button.is_active());
                this.render();
            }
        });
        flow.insert(&edit, -1);
        root.append(&flow);
        if self.editing.get() {
            root.append(&gtk::Label::new(Some("Нажмите значок, чтобы убрать")));
        }
        root.append(&status);
    }
    #[allow(deprecated)]
    fn choose(self: &Rc<Self>, status: &gtk::Label) {
        if self.shared.smoke {
            return;
        }
        let Some(parent) = self
            .root
            .upgrade()
            .and_then(|r| r.root())
            .and_then(|r| r.downcast::<gtk::Window>().ok())
        else {
            return;
        };
        let dialog = gtk::AppChooserDialog::for_content_type(
            Some(&parent),
            gtk::DialogFlags::MODAL,
            "application/octet-stream",
        );
        dialog.set_heading("Добавить приложение в док");
        if let Ok(widget) = dialog.widget().downcast::<gtk::AppChooserWidget>() {
            widget.set_show_all(true);
        }
        let weak = Rc::downgrade(self);
        let status = status.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Ok {
                if let (Some(this), Some(id)) =
                    (weak.upgrade(), dialog.app_info().and_then(|a| a.id()))
                {
                    let mut ids = this.ids();
                    ids.push(id.to_string());
                    if !this.store(ids) {
                        status.set_text("Не удалось сохранить док");
                        status.set_visible(true);
                    }
                }
            }
            dialog.close();
        });
        dialog.present();
    }
}
