//! Reusable native launcher. The parent mounts `root` and calls `focus_entry`.
use crate::ui::{page_header, Shared};
use gtk::{gio, glib, prelude::*};
use std::{cell::RefCell, path::PathBuf, rc::Rc};
#[path = "../services/launcher.rs"]
mod backend;
use backend::desktop::{self, App};

type Dismiss = Rc<RefCell<Option<Box<dyn Fn()>>>>;
#[derive(Clone)]
enum Hit {
    App(usize),
    Calculation(String),
    Path(PathBuf),
    Web(String),
}
struct Row {
    hit: Hit,
    title: String,
    detail: String,
    icon: Option<gio::Icon>,
    fallback: &'static str,
}
pub struct LauncherView {
    pub root: gtk::Box,
    entry: gtk::SearchEntry,
    dismiss: Dismiss,
}
impl LauncherView {
    pub fn build(shared: Shared) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("tidemark-page");
        root.append(&page_header(
            "Поиск",
            Some("Приложения, файлы, вычисления и интернет"),
        ));
        let entry = gtk::SearchEntry::new();
        entry.set_placeholder_text(Some("Приложение, полный путь, 2 + 2 или запрос…"));
        entry.set_hexpand(true);
        root.append(&entry);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.set_activate_on_single_click(true);
        list.add_css_class("boxed-list");
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scrolled.set_min_content_height(180);
        scrolled.set_max_content_height(400);
        scrolled.set_propagate_natural_height(true);
        scrolled.set_child(Some(&list));
        root.append(&scrolled);
        let status = gtk::Label::new(Some("↑ ↓ — выбрать · Enter — открыть · Esc — закрыть"));
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.add_css_class("muted");
        status.add_css_class("tiny");
        root.append(&status);
        // Enumerating local desktop metadata does not launch an application or
        // send network requests. AppInfo owns desktop activation; Exec is opaque.
        let apps = Rc::new(desktop::installed());
        let names: Rc<Vec<String>> = Rc::new(apps.iter().map(|a| a.name.clone()).collect());
        let hits = Rc::new(RefCell::new(Vec::<Hit>::new()));
        let dismiss: Dismiss = Rc::new(RefCell::new(None));
        let activate: Rc<dyn Fn(Hit)> = {
            let apps = apps.clone();
            let status = status.clone();
            let dismiss = dismiss.clone();
            let smoke = shared.smoke;
            Rc::new(move |hit| {
                if smoke {
                    status.set_text("Проверка интерфейса: запуск отключён.");
                    return;
                }
                let result: Result<(), String> = match hit {
                    Hit::App(index) => apps
                        .get(index)
                        .ok_or_else(|| "Приложение больше недоступно.".to_string())
                        .and_then(|app| {
                            app.info
                                .launch(&[], None::<&gio::AppLaunchContext>)
                                .map_err(|_| "Не удалось запустить приложение.".to_string())
                        }),
                    Hit::Path(path) => {
                        // Revalidate at activation in case the file disappeared.
                        if !path.is_absolute() || !path.exists() {
                            Err("Файл больше не существует.".into())
                        } else {
                            gio::AppInfo::launch_default_for_uri(
                                gio::File::for_path(path).uri().as_str(),
                                None::<&gio::AppLaunchContext>,
                            )
                            .map_err(|_| "Не удалось открыть файл подходящим приложением.".into())
                        }
                    }
                    Hit::Web(url) => {
                        gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>)
                            .map_err(|_| "Не удалось открыть браузер.".into())
                    }
                    Hit::Calculation(value) => {
                        if crate::services::clipboard::write_text(&value) {
                            status.set_text("Результат скопирован.");
                        } else {
                            status.set_text("Не удалось скопировать результат.");
                        }
                        return;
                    }
                };
                match result {
                    Ok(()) => {
                        status.set_text("Открыто.");
                        if let Some(close) = dismiss.borrow().as_ref() {
                            close();
                        }
                    }
                    Err(error) => status.set_text(&error),
                }
            })
        };
        let refresh: Rc<dyn Fn(&str)> = {
            let apps = apps.clone();
            let names = names.clone();
            let hits = hits.clone();
            let list = list.clone();
            let status = status.clone();
            let shared = shared.clone();
            let scrolled = scrolled.clone();
            Rc::new(move |query| {
                while let Some(child) = list.first_child() {
                    list.remove(&child);
                }
                hits.borrow_mut().clear();
                let query = query.trim();
                if query.len() > 1024 {
                    status.set_text("Сократите запрос до 1024 байт.");
                    return;
                }
                let mut rows = Vec::new();
                if let Some(value) = backend::calc(query) {
                    let value = backend::format_number(value);
                    rows.push(Row {
                        hit: Hit::Calculation(value.clone()),
                        title: format!("= {value}"),
                        detail: "Калькулятор · Enter — скопировать".into(),
                        icon: None,
                        fallback: "accessories-calculator-symbolic",
                    });
                }
                if let Some(path) = backend::absolute_existing_path(query) {
                    rows.push(Row {
                        title: path
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string()),
                        detail: path.display().to_string(),
                        hit: Hit::Path(path),
                        icon: None,
                        fallback: "folder-open-symbolic",
                    });
                }
                let web = backend::search_url(&shared.config.borrow().search_url, query);
                let app_limit =
                    backend::MAX_RESULTS.saturating_sub(rows.len() + usize::from(web.is_some()));
                for index in backend::ranked(&names, query, app_limit) {
                    let App { info, name, detail } = &apps[index];
                    rows.push(Row {
                        hit: Hit::App(index),
                        title: name.clone(),
                        detail: detail.clone(),
                        icon: info.icon(),
                        fallback: "application-x-executable-symbolic",
                    });
                }
                if let Some(url) = web {
                    rows.push(Row {
                        hit: Hit::Web(url),
                        title: format!("Искать «{query}» в интернете"),
                        detail: "Открыть поиск в браузере".into(),
                        icon: None,
                        fallback: "system-search-symbolic",
                    });
                }
                for row in rows.into_iter().take(backend::MAX_RESULTS) {
                    hits.borrow_mut().push(row.hit);
                    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                    content.set_margin_top(8);
                    content.set_margin_bottom(8);
                    content.set_margin_start(10);
                    content.set_margin_end(10);
                    let icon = if let Some(icon) = row.icon {
                        gtk::Image::from_gicon(&icon)
                    } else {
                        gtk::Image::from_icon_name(row.fallback)
                    };
                    icon.set_pixel_size(24);
                    content.append(&icon);
                    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
                    labels.set_hexpand(true);
                    let title = gtk::Label::new(Some(&row.title));
                    title.set_xalign(0.0);
                    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    title.set_max_width_chars(55);
                    labels.append(&title);
                    let detail = gtk::Label::new(Some(&row.detail));
                    detail.set_xalign(0.0);
                    detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    detail.set_max_width_chars(62);
                    detail.add_css_class("muted");
                    detail.add_css_class("tiny");
                    labels.append(&detail);
                    content.append(&labels);
                    let list_row = gtk::ListBoxRow::new();
                    list_row.set_child(Some(&content));
                    list.append(&list_row);
                }
                if let Some(first) = list.row_at_index(0) {
                    list.select_row(Some(&first));
                }
                scrolled.vadjustment().set_value(0.0);
                status.set_text(if hits.borrow().is_empty() {
                    "Ничего не найдено. Попробуйте другое название или полный путь."
                } else {
                    "↑ ↓ — выбрать · Enter — открыть · Esc — закрыть"
                });
            })
        };
        refresh("");
        {
            let refresh = refresh.clone();
            entry.connect_search_changed(move |entry| refresh(entry.text().as_str()));
        }
        {
            let hits = hits.clone();
            let activate = activate.clone();
            list.connect_row_activated(move |_, row| {
                let hit = hits.borrow().get(row.index() as usize).cloned();
                if let Some(hit) = hit {
                    activate(hit);
                }
            });
        }
        {
            let hits = hits.clone();
            let list = list.clone();
            let activate = activate.clone();
            entry.connect_activate(move |_| {
                if let Some(row) = list.selected_row() {
                    let hit = hits.borrow().get(row.index() as usize).cloned();
                    if let Some(hit) = hit {
                        activate(hit);
                    }
                }
            });
        }
        {
            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            let list = list.clone();
            let scrolled = scrolled.clone();
            let hits = hits.clone();
            let dismiss = dismiss.clone();
            keys.connect_key_pressed(move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape {
                    if let Some(close) = dismiss.borrow().as_ref() {
                        close();
                    }
                    return glib::Propagation::Stop;
                }
                let delta = if key == gtk::gdk::Key::Down {
                    1
                } else if key == gtk::gdk::Key::Up {
                    -1
                } else {
                    return glib::Propagation::Proceed;
                };
                let current = list.selected_row().map(|r| r.index() as usize);
                if let Some(next) = backend::next_selection(current, delta, hits.borrow().len()) {
                    if let Some(row) = list.row_at_index(next as i32) {
                        list.select_row(Some(&row));
                        let adjustment = scrolled.vadjustment();
                        let y = row.allocation().y() as f64;
                        let bottom = y + row.height() as f64;
                        if y < adjustment.value() {
                            adjustment.set_value(y);
                        } else if bottom > adjustment.value() + adjustment.page_size() {
                            adjustment.set_value(bottom - adjustment.page_size());
                        }
                    }
                }
                glib::Propagation::Stop
            });
            entry.add_controller(keys);
        }
        Self {
            root,
            entry,
            dismiss,
        }
    }
    pub fn focus_entry(&self) {
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
    }
    /// Wire this to the parent popover/window close action. Escape and successful
    /// launches request dismissal; calculation copies keep the launcher open.
    pub fn connect_dismiss(&self, callback: impl Fn() + 'static) {
        *self.dismiss.borrow_mut() = Some(Box::new(callback));
    }
}
