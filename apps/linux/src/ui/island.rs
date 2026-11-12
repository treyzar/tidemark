//! The island window: a compact pill that expands on hover or click.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use tidemark_core::notes::Reminder;

use crate::model;
use crate::services::music::MusicEvent;
use crate::ui::{
    clipboard::ClipboardView, control::ControlView, home::HomeView, notes::NotesView, settings,
    text::TextView, Shared,
};

const ISLAND_TITLE: &str = "tidemark-island";
const SETTINGS_TITLE: &str = "Tidemark — настройки";
const EXPANDED_HEIGHT: i32 = 620;

pub struct Island {
    pub window: adw::ApplicationWindow,
    pub home: HomeView,
    pub notes: NotesView,
    pub control: ControlView,
    pub clipboard: Rc<ClipboardView>,
    pub text: TextView,
    pub stack: gtk::Stack,
    pub switcher: gtk::StackSwitcher,
    revealer: gtk::Revealer,
    root: gtk::Box,
    collapsed: gtk::Box,
    clock: gtk::Label,
    mini: gtk::Label,
    pinned: Cell<bool>,
    hover_source: RefCell<Option<glib::SourceId>>,
    app: adw::Application,
    shared: Shared,
    me: RefCell<Option<Weak<Self>>>,
}

impl Island {
    pub fn build(app: &adw::Application, shared: Shared) -> Rc<Self> {
        let config = shared.config.borrow();
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(ISLAND_TITLE)
            .decorated(false)
            .resizable(false)
            .default_width(config.collapsed_width as i32)
            .default_height(config.collapsed_height as i32)
            .build();
        drop(config);
        window.add_css_class("tidemark-window");

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("tidemark-island");
        root.add_css_class("collapsed");

        let collapsed = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        collapsed.add_css_class("tidemark-collapsed");
        collapsed.set_valign(gtk::Align::Center);
        let clock = gtk::Label::new(Some(&local_clock()));
        clock.add_css_class("tidemark-clock");
        let mini = gtk::Label::new(Some("Tidemark"));
        mini.add_css_class("tidemark-mini");
        mini.set_ellipsize(gtk::pango::EllipsizeMode::End);
        mini.set_hexpand(true);
        collapsed.append(&clock);
        collapsed.append(&mini);
        root.append(&collapsed);

        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        revealer.set_transition_duration(180);
        revealer.set_reveal_child(false);
        revealer.set_halign(gtk::Align::Fill);

        let expanded = gtk::Box::new(gtk::Orientation::Vertical, 0);
        expanded.add_css_class("expanded");
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.add_css_class("tidemark-header");
        let switcher = gtk::StackSwitcher::new();
        switcher.set_hexpand(true);
        switcher.set_halign(gtk::Align::Start);
        let settings_button = gtk::Button::from_icon_name("emblem-system-symbolic");
        settings_button.set_tooltip_text(Some("Настройки"));
        settings_button.update_property(&[gtk::accessible::Property::Label("Настройки")]);
        header.append(&switcher);
        header.append(&settings_button);

        let home = HomeView::build(shared.clone());
        let clipboard = ClipboardView::build(shared.clone());
        let notes = NotesView::build(shared.clone());
        let text = TextView::build(shared.clone());
        let control = ControlView::build(shared.clone());

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(120);
        stack.set_vexpand(true);
        for (id, label, _) in model::TABS {
            let page: gtk::Widget = match id {
                "home" => home.root.clone().upcast(),
                "clipboard" => clipboard.root.clone().upcast(),
                "notes" => notes.root.clone().upcast(),
                "text" => text.root.clone().upcast(),
                _ => control.root.clone().upcast(),
            };
            let scrolled = crate::ui::scroll(&page);
            stack.add_titled(&scrolled, Some(id), Some(label));
        }
        stack.set_visible_child_name("home");
        switcher.set_stack(Some(&stack));
        expanded.append(&header);
        expanded.append(&stack);

        let status = gtk::Label::new(Some(&shell_status()));
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.add_css_class("tidemark-status");
        expanded.append(&status);
        revealer.set_child(Some(&expanded));
        root.append(&revealer);

        let handle = gtk::WindowHandle::new();
        handle.set_child(Some(&root));
        window.set_child(Some(&handle));

        let island = Rc::new(Self {
            window,
            home,
            notes,
            control,
            clipboard,
            text,
            stack,
            switcher,
            settings_button,
            revealer,
            root,
            collapsed,
            clock,
            mini,
            pinned: Cell::new(false),
            hover_source: RefCell::new(None),
            app: app.clone(),
            shared,
            me: RefCell::new(None),
        });
        island.me.replace(Some(Rc::downgrade(&island)));
        island.wire();
        island
    }

    fn wire(&self) {
        let motion = gtk::EventControllerMotion::new();
        {
            let weak = self.weak();
            motion.connect_enter(move |_, _, _| {
                if let Some(island) = weak.upgrade() {
                    island.schedule_expand();
                }
            });
        }
        {
            let weak = self.weak();
            motion.connect_leave(move |_| {
                if let Some(island) = weak.upgrade() {
                    island.cancel_hover();
                    if !island.pinned.get() {
                        island.collapse();
                    }
                }
            });
        }
        self.root.add_controller(motion);

        let click = gtk::GestureClick::new();
        {
            let weak = self.weak();
            click.connect_pressed(move |_, _, _, _| {
                if let Some(island) = weak.upgrade() {
                    island.pinned.set(true);
                    island.expand();
                }
            });
        }
        self.collapsed.add_controller(click);

        let keys = gtk::EventControllerKey::new();
        {
            let weak = self.weak();
            keys.connect_key_pressed(move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape {
                    if let Some(island) = weak.upgrade() {
                        island.pinned.set(false);
                        island.collapse();
                    }
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        self.window.add_controller(keys);

        {
            let weak = self.weak();
            self.stack.connect_visible_child_name_notify(move |_| {
                if let Some(island) = weak.upgrade() {
                    island.refresh_current();
                }
            });
        }

        {
            let weak = self.weak();
            self.settings_button.clone().connect_clicked(move |_| {
                if let Some(island) = weak.upgrade() {
                    island.open_settings();
                }
            });
        }
    }

    fn weak(&self) -> Weak<Self> {
        self.me
            .borrow()
            .clone()
            .expect("island is initialized before signals are wired")
    }

    fn schedule_expand(&self) {
        if self.revealer.reveals_child() {
            return;
        }
        self.cancel_hover();
        let delay = self.shared.config.borrow().hover_delay_ms.min(5000) as u64;
        let weak = self.weak();
        let source = glib::timeout_add_local_once(Duration::from_millis(delay), move || {
            if let Some(island) = weak.upgrade() {
                island.expand();
            }
        });
        *self.hover_source.borrow_mut() = Some(source);
    }

    fn cancel_hover(&self) {
        if let Some(source) = self.hover_source.borrow_mut().take() {
            source.remove();
        }
    }

    pub fn expand(&self) {
        self.cancel_hover();
        if self.revealer.reveals_child() {
            return;
        }
        let width = self.shared.config.borrow().expanded_width as i32;
        self.collapsed.set_visible(false);
        self.revealer.set_reveal_child(true);
        self.root.add_css_class("expanded");
        self.root.remove_css_class("collapsed");
        self.window
            .set_size_request(width.max(320), EXPANDED_HEIGHT);
        self.refresh_current();
    }

    pub fn collapse(&self) {
        self.cancel_hover();
        if !self.revealer.reveals_child() {
            return;
        }
        self.collapsed.set_visible(true);
        self.revealer.set_reveal_child(false);
        self.root.remove_css_class("expanded");
        self.root.add_css_class("collapsed");
        let config = self.shared.config.borrow();
        self.window.set_size_request(
            config.collapsed_width as i32,
            config.collapsed_height as i32,
        );
    }

    pub fn refresh_current(&self) {
        match self.stack.visible_child_name().as_deref() {
            Some("home") => self.home.refresh_tiles(),
            Some("clipboard") => self.clipboard.refresh(),
            Some("notes") => self.notes.refresh(),
            _ => {}
        }
    }

    pub fn show_tab(&self, id: &str) {
        self.stack.set_visible_child_name(id);
        self.refresh_current();
    }

    pub fn handle_music(&self, event: &MusicEvent) {
        self.home.handle_event(event);
        self.update_collapsed();
    }

    pub fn tick(&self) {
        self.clock.set_text(&local_clock());
        self.home.tick();
        self.update_collapsed();
        if self.control.tick() {
            self.notify("Таймер", "Время вышло.");
        }
        let due: Vec<Reminder> = self.notes.take_due();
        for reminder in due {
            self.notify(
                "Напоминание",
                if reminder.text.is_empty() {
                    "Сработало напоминание."
                } else {
                    &reminder.text
                },
            );
        }
    }

    fn update_collapsed(&self) {
        let track = self.home.now_playing();
        self.mini.set_text(track.as_deref().unwrap_or("Tidemark"));
        self.mini.set_tooltip_text(Some(if self.shared.smoke {
            "Режим проверки"
        } else {
            "Tidemark"
        }));
    }

    pub fn save_all(&self) {
        self.notes.save_now();
        self.shared.notes.borrow().save();
        self.shared.config.borrow().save();
        self.shared.clip.borrow().save();
    }

    pub fn open_settings(&self) -> adw::PreferencesWindow {
        for window in self.app.windows() {
            let is_settings = window
                .title()
                .map(|title| title.as_str() == SETTINGS_TITLE)
                .unwrap_or(false);
            if is_settings {
                if let Ok(window) = window.downcast::<adw::PreferencesWindow>() {
                    window.present();
                    return window;
                }
            }
        }
        let window = settings::build(&self.app, self.shared.clone());
        window.present();
        window
    }

    pub fn notify(&self, title: &str, body: &str) {
        use gtk::gio::prelude::ApplicationExt;
        let notification = gtk::gio::Notification::new(title);
        notification.set_body(Some(body));
        self.app.send_notification(None, &notification);
    }

    pub fn root_widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    pub fn settings_root(&self, window: &adw::PreferencesWindow) -> gtk::Widget {
        window
            .child()
            .map(|child| child.upcast())
            .unwrap_or_else(|| window.clone().upcast())
    }
}

fn local_clock() -> String {
    match glib::DateTime::now_local() {
        Ok(dt) => format!("{:02}:{:02}", dt.hour().max(0), dt.minute().max(0)),
        Err(_) => "--:--".into(),
    }
}

fn shell_status() -> String {
    if crate::services::shell::launched_by_adapter() {
        "Запущено расширением GNOME Shell. Мост D-Bus подключается при старте.".into()
    } else {
        model::shell_status_text(&[])
    }
}
