//! The island window: a compact pill that expands on hover or click.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use tidemark_core::notes::Reminder;

use crate::model;
use crate::services::music::MusicEvent;
use crate::services::shell::{self, Command as ShellCommand, Event as ShellEvent};
use crate::ui::{
    clipboard::ClipboardView, control::ControlView, home::HomeView, notes::NotesView, settings,
    text::TextView, Shared,
};
use tidemark_core::shell::{Capability, FocusInfo, Rect, ShortcutId};

const ISLAND_TITLE: &str = "tidemark-island";
const SETTINGS_TITLE: &str = "Tidemark — настройки";
const EXPANDED_HEIGHT: i32 = 620;

pub struct Island {
    pub window: gtk::ApplicationWindow,
    pub home: HomeView,
    pub notes: NotesView,
    pub control: ControlView,
    pub clipboard: Rc<ClipboardView>,
    pub text: TextView,
    pub shelf: crate::ui::shelf::ShelfView,
    pub stack: gtk::Stack,
    pub agent: crate::ui::agent::AgentView,
    agent_previous: Cell<Option<(bool, bool)>>,
    pub switcher: gtk::StackSwitcher,
    pub launcher: crate::ui::launcher::LauncherView,
    content: gtk::Stack,
    search_button: gtk::Button,
    launcher_previous: Cell<Option<(bool, bool)>>,
    last_launcher_enabled: Cell<Option<bool>>,
    last_clipboard_enabled: Cell<Option<bool>>,
    settings_button: gtk::Button,
    revealer: gtk::Revealer,
    root: gtk::Box,
    collapsed: gtk::Box,
    clock: gtk::Label,
    status: gtk::Label,
    restart: gtk::Button,
    shell: RefCell<shell::Service>,
    shell_caps: RefCell<Vec<Capability>>,
    focus: RefCell<Option<FocusInfo>>,
    shell_layout: RefCell<String>,
    caps_lock: Cell<bool>,
    sleep_armed: Cell<bool>,
    primary_monitor: Cell<Option<u32>>,
    fullscreen: RefCell<HashSet<u32>>,
    shell_hidden: Cell<bool>,
    last_placement: Cell<Option<Rect>>,
    mini: gtk::Label,
    pinned: Cell<bool>,
    hover_source: RefCell<Option<glib::SourceId>>,
    app: adw::Application,
    shared: Shared,
    me: RefCell<Option<Weak<Self>>>,
    input_shape: RefCell<Option<(i32, i32, i32, i32, i32)>>,
    input_region: RefCell<Option<gtk::cairo::Region>>,
}

impl Island {
    pub fn build(app: &adw::Application, shared: Shared) -> Rc<Self> {
        let config = shared.config.borrow();
        // Adwaita 1.5's breakpoint bin allocates against the child's vertical
        // request at width=-1. A fixed-size, wrapped overlay needs GTK's
        // regular height-for-width allocation; settings still use Adwaita.
        let window = gtk::ApplicationWindow::builder()
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
        // Resize the native surface immediately so a collapsed island never
        // leaves an expanded transparent surface intercepting other apps.
        revealer.set_transition_type(gtk::RevealerTransitionType::None);
        revealer.set_reveal_child(false);
        revealer.set_visible(false);
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
        let search_button = gtk::Button::from_icon_name("system-search-symbolic");
        search_button.set_tooltip_text(Some("Поиск · Super+Alt+Space"));
        search_button.update_property(&[gtk::accessible::Property::Label("Поиск")]);
        search_button.set_visible(shared.config.borrow().launcher);
        header.append(&switcher);
        header.append(&search_button);
        header.append(&settings_button);

        let home = HomeView::build(shared.clone());
        let clipboard = ClipboardView::build(shared.clone());
        let notes = NotesView::build(shared.clone());
        let text = TextView::build(shared.clone());
        let shelf = crate::ui::shelf::ShelfView::build(shared.clone());
        let control = ControlView::build(shared.clone());

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(120);
        stack.set_vexpand(true);
        for (id, label, icon) in model::TABS {
            let page: gtk::Widget = match id {
                "home" => home.root.clone().upcast(),
                "clipboard" => clipboard.root.clone().upcast(),
                "notes" => notes.root.clone().upcast(),
                "text" => text.root.clone().upcast(),
                "shelf" => shelf.root.clone().upcast(),
                "control" => control.root.clone().upcast(),
                _ => unreachable!("unknown island tab"),
            };
            let scrolled = crate::ui::scroll(&page);
            // Icons retain the page title as the tooltip/accessibility label
            // while keeping the five-tab header inside the configured width.
            stack
                .add_titled(&scrolled, Some(id), label)
                .set_icon_name(icon);
        }
        stack.set_visible_child_name("home");
        switcher.set_stack(Some(&stack));
        expanded.append(&header);
        let agent = crate::ui::agent::AgentView::build(shared.smoke);
        expanded.append(&agent.root);
        let launcher = crate::ui::launcher::LauncherView::build(shared.clone());
        let content = gtk::Stack::new();
        content.set_hhomogeneous(false);
        content.set_vhomogeneous(false);
        content.set_vexpand(true);
        content.add_named(&stack, Some("tabs"));
        content.add_named(&launcher.root, Some("launcher"));
        content.set_visible_child_name("tabs");
        // The card above must retain its complete text/buttons. A Stack's
        // visible page can otherwise contribute its minimum height to the
        // parent even when its inner page is scrollable, pushing the window
        // beyond 620px. This outer viewport makes the body explicitly shrink
        // and scroll within the space left below a card.
        let body = gtk::ScrolledWindow::new();
        body.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        body.set_min_content_height(0);
        body.set_propagate_natural_height(false);
        body.set_vexpand(true);
        body.set_child(Some(&content));
        expanded.append(&body);

        let status = gtk::Label::new(Some(&shell_status()));
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        status.add_css_class("tidemark-status");
        expanded.append(&status);
        let restart = gtk::Button::with_label("Перезапустить для GNOME…");
        restart.set_visible(false);
        expanded.append(&restart);
        revealer.set_child(Some(&expanded));
        root.append(&revealer);

        let handle = gtk::WindowHandle::new();
        handle.set_child(Some(&root));
        window.set_child(Some(&handle));
        // Adwaita/Wayland can initially allocate its natural 360×200 size
        // despite builder defaults. Apply the same explicit compact request
        // used by collapse(), before the first presentation.
        {
            let config = shared.config.borrow();
            window.set_size_request(
                config.collapsed_width as i32,
                config.collapsed_height as i32,
            );
            window.set_default_size(
                config.collapsed_width as i32,
                config.collapsed_height as i32,
            );
        }

        let island = Rc::new(Self {
            window,
            home,
            notes,
            control,
            clipboard,
            text,
            shelf,
            stack,
            agent,
            agent_previous: Cell::new(None),
            switcher,
            launcher,
            content,
            search_button,
            launcher_previous: Cell::new(None),
            last_launcher_enabled: Cell::new(None),
            last_clipboard_enabled: Cell::new(None),
            settings_button,
            revealer,
            root,
            collapsed,
            clock,
            status,
            restart,
            shell: RefCell::new(shell::Service::default()),
            shell_caps: RefCell::new(Vec::new()),
            focus: RefCell::new(None),
            shell_layout: RefCell::new(String::new()),
            caps_lock: Cell::new(false),
            sleep_armed: Cell::new(false),
            primary_monitor: Cell::new(None),
            fullscreen: RefCell::new(HashSet::new()),
            shell_hidden: Cell::new(false),
            last_placement: Cell::new(None),
            mini,
            pinned: Cell::new(false),
            hover_source: RefCell::new(None),
            app: app.clone(),
            shared,
            me: RefCell::new(None),
            input_shape: RefCell::new(None),
            input_region: RefCell::new(None),
        });
        island.me.replace(Some(Rc::downgrade(&island)));
        island.wire();
        crate::ui::install_hand_cursors(&island.window);
        island
    }

    fn wire(&self) {
        let weak = self.weak();
        self.home.connect_navigate(move |tab| {
            if let Some(island) = weak.upgrade() {
                island.expand();
                island.show_tab(tab);
            }
        });
        let shelf_drop = self.shelf.drop_target();
        let weak = self.weak();
        shelf_drop.connect_enter(move |_, _, _| {
            if let Some(island) = weak.upgrade() {
                island.expand();
                island.show_tab("shelf");
            }
            gtk::gdk::DragAction::COPY
        });
        self.window.add_controller(shelf_drop);
        let weak = self.weak();
        self.search_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                if island.launcher_visible() {
                    island.close_launcher(false);
                } else {
                    island.open_launcher();
                }
            }
        });
        let weak = self.weak();
        self.launcher.connect_dismiss(move || {
            if let Some(island) = weak.upgrade() {
                island.close_launcher(false);
            }
        });
        let tabs = gtk::GestureClick::new();
        tabs.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.weak();
        tabs.connect_pressed(move |_, _, _, _| {
            if let Some(island) = weak.upgrade() {
                island.close_launcher(false);
            }
        });
        self.switcher.add_controller(tabs);
        let weak = self.weak();
        self.restart.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.offer_handover();
            }
        });
        let weak = self.weak();
        self.window.connect_realize(move |window| {
            if let Some(surface) = window.surface() {
                let weak = weak.clone();
                surface.connect_layout(move |_, _, _| {
                    if let Some(island) = weak.upgrade() {
                        island.queue_input_region();
                    }
                });
            }
            if let Some(island) = weak.upgrade() {
                island.queue_input_region();
            }
        });
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
        if !self.shared.smoke {
            self.root.add_controller(motion);
        }

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
                        if island.launcher_visible() {
                            island.close_launcher(false);
                        } else if island.agent.dismiss() {
                            island.pump_agent();
                        } else {
                            island.pinned.set(false);
                            island.collapse();
                        }
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
                island.hover_source.borrow_mut().take();
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
        self.revealer.set_visible(true);
        self.revealer.set_reveal_child(true);
        self.root.add_css_class("expanded");
        self.root.remove_css_class("collapsed");
        self.window
            .set_size_request(width.max(320), EXPANDED_HEIGHT);
        self.window
            .set_default_size(width.max(320), EXPANDED_HEIGHT);
        self.refresh_current();
        self.place_shell();
    }

    pub fn collapse(&self) {
        // Any route to compact mode closes search without recursively restoring
        // the geometry captured when search opened.
        self.content.set_visible_child_name("tabs");
        self.launcher_previous.set(None);
        self.search_button.set_icon_name("system-search-symbolic");
        self.cancel_hover();
        if !self.revealer.reveals_child() {
            return;
        }
        self.collapsed.set_visible(true);
        self.revealer.set_reveal_child(false);
        self.revealer.set_visible(false);
        self.root.remove_css_class("expanded");
        self.root.add_css_class("collapsed");
        let config = self.shared.config.borrow();
        self.window.set_size_request(
            config.collapsed_width as i32,
            config.collapsed_height as i32,
        );
        self.window.set_default_size(
            config.collapsed_width as i32,
            config.collapsed_height as i32,
        );
        drop(config);
        self.place_shell();
    }

    fn update_input_region(&self) {
        let Some(surface) = self.window.surface() else {
            return;
        };
        let Some(bounds) = self.root.compute_bounds(&self.window) else {
            return;
        };
        // Widget::width/height report the content box, excluding the CSS
        // border. Input must cover the visible border box in surface space.
        let width = bounds.width().round() as i32;
        let height = bounds.height().round() as i32;
        if width <= 0 || height <= 0 {
            return;
        }
        let radius = if self.revealer.reveals_child() {
            self.shared.config.borrow().expanded_radius.clamp(0.0, 64.0) as i32
        } else {
            18
        };
        let shape = (
            bounds.x().round() as i32,
            bounds.y().round() as i32,
            width,
            height,
            radius.min(width / 2).min(height / 2),
        );
        if self.input_shape.borrow().as_ref() == Some(&shape) {
            return;
        }
        let (x, y, w, h, r) = shape;
        let mut rows = Vec::with_capacity(h as usize);
        for row in 0..h {
            let dy = if row < r {
                r as f64 - (row as f64 + 0.5)
            } else if row >= h - r {
                (row as f64 + 0.5) - (h - r) as f64
            } else {
                0.0
            };
            let inset = if dy > 0.0 {
                (r as f64 - ((r * r) as f64 - dy * dy).max(0.0).sqrt()).ceil() as i32
            } else {
                0
            };
            rows.push(gtk::cairo::RectangleInt::new(
                x + inset,
                y + row,
                w - 2 * inset,
                1,
            ));
        }
        let region = gtk::cairo::Region::create_rectangles(&rows);
        surface.set_input_region(&region);
        self.input_shape.replace(Some(shape));
        self.input_region.replace(Some(region));
    }

    fn queue_input_region(&self) {
        let weak = self.weak();
        // Tick callbacks precede layout: retain this for a second frame to
        // read the final allocation, then stop requesting idle redraws.
        let first = Cell::new(true);
        self.root.add_tick_callback(move |_, _| {
            let Some(island) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            island.update_input_region();
            if first.replace(false) {
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
    }

    /// Check the real post-layout allocation and the exact region submitted
    /// to GDK. GTK has no public input-region getter; compositor click routing
    /// is a separate desktop acceptance check.
    pub fn check_geometry(&self, expanded: bool) -> Result<(), String> {
        self.update_input_region();
        let config = self.shared.config.borrow();
        let expected = if expanded {
            ((config.expanded_width as i32).max(320), EXPANDED_HEIGHT)
        } else {
            (
                config.collapsed_width as i32,
                config.collapsed_height as i32,
            )
        };
        let bounds = self
            .root
            .compute_bounds(&self.window)
            .ok_or("missing visible bounds")?;
        let actual = (
            bounds.width().round() as i32,
            bounds.height().round() as i32,
        );
        let surface = self.window.surface().ok_or("missing GDK surface")?;
        if actual != expected
            || (surface.width(), surface.height()) != actual
            || bounds.x().round() as i32 != 0
            || bounds.y().round() as i32 != 0
        {
            return Err(format!(
                "expected {expected:?}, visible {actual:?} at ({},{}), content {}x{}, surface {}x{}",
                bounds.x(), bounds.y(), self.root.width(), self.root.height(),
                surface.width(),
                surface.height()
            ));
        }
        let shape = self.input_shape.borrow();
        let &(x, y, w, h, r) = shape.as_ref().ok_or("missing input shape")?;
        let region = self.input_region.borrow();
        let region = region.as_ref().ok_or("missing submitted region")?;
        if !region.contains_point(x + w / 2, y + h / 2)
            || region.contains_point(x - 1, y)
            || region.contains_point(x + w, y + h / 2)
            || (r > 1 && region.contains_point(x, y))
        {
            return Err("input region does not match visible island".into());
        }
        println!(
            "SMOKE_GEOMETRY expanded={expanded} allocation={}x{} radius={r} input_rectangles={}",
            actual.0,
            actual.1,
            region.num_rectangles()
        );
        Ok(())
    }

    pub fn clipboard_history_enabled(&self) -> bool {
        self.shared.config.borrow().clipboard_history
    }
    pub fn launcher_enabled(&self) -> bool {
        self.shared.config.borrow().launcher
    }
    fn launcher_visible(&self) -> bool {
        self.content.visible_child_name().as_deref() == Some("launcher")
    }
    pub fn open_launcher(&self) {
        if !self.launcher_enabled() || self.launcher_visible() {
            return;
        }
        if self.agent.root.is_visible() {
            self.status
                .set_text("Сначала ответьте на карточку агента или закройте её без ответа.");
            return;
        }
        self.launcher_previous
            .set(Some((self.revealer.reveals_child(), self.pinned.get())));
        self.pinned.set(true);
        self.expand();
        self.content.set_visible_child_name("launcher");
        self.search_button.set_icon_name("go-previous-symbolic");
        self.search_button
            .set_tooltip_text(Some("Назад к вкладкам"));
        // An explicit search action should receive keyboard input. Fullscreen
        // restoration deliberately uses only set_visible elsewhere.
        self.window.present();
        let weak = self.weak();
        glib::idle_add_local_once(move || {
            if let Some(island) = weak.upgrade() {
                if island.launcher_visible() {
                    island.launcher.focus_entry();
                }
            }
        });
    }
    pub fn close_launcher(&self, collapse: bool) {
        if !self.launcher_visible() {
            return;
        }
        self.content.set_visible_child_name("tabs");
        self.search_button.set_icon_name("system-search-symbolic");
        self.search_button
            .set_tooltip_text(Some("Поиск · Super+Alt+Space"));
        if let Some((was_expanded, pinned)) = self.launcher_previous.take() {
            self.pinned.set(pinned);
            if collapse || !was_expanded {
                self.pinned.set(false);
                self.collapse();
            }
        }
    }
    fn sync_launcher_setting(&self) {
        let enabled = self.launcher_enabled();
        self.search_button.set_visible(enabled);
        if !enabled {
            self.close_launcher(false);
        }
        if self.last_launcher_enabled.get() != Some(enabled)
            && self.shell.borrow().send(ShellCommand::Shortcuts(enabled))
        {
            self.last_launcher_enabled.set(Some(enabled));
        }
    }

    pub fn pump_agent(&self) {
        if self.agent.refresh() {
            if self.launcher_visible() {
                self.close_launcher(false);
            }
            if self.agent_previous.get().is_none() {
                self.agent_previous
                    .set(Some((self.revealer.reveals_child(), self.pinned.get())));
                self.pinned.set(true);
                self.expand();
            }
        } else if let Some((expanded, pinned)) = self.agent_previous.take() {
            self.pinned.set(pinned);
            if !expanded {
                self.collapse();
            }
        }
    }

    pub fn attach_shell(&self, service: shell::Service) {
        self.shared.shell.replace(service.clone());
        self.shell.replace(service);
    }
    fn sync_clipboard_setting(&self) {
        let enabled = self.clipboard_history_enabled()
            && self
                .shell_caps
                .borrow()
                .contains(&Capability::ClipboardRead);
        if self.last_clipboard_enabled.get() != Some(enabled)
            && self
                .shell
                .borrow()
                .send(ShellCommand::CaptureClipboard(enabled))
        {
            self.last_clipboard_enabled.set(Some(enabled));
            self.clipboard.refresh();
        }
    }

    fn place_shell(&self) {
        if !self.shell_caps.borrow().contains(&Capability::PlaceWindows) {
            return;
        }
        let expanded = self.revealer.reveals_child();
        let config = self.shared.config.borrow();
        let rect = Rect {
            x: 0,
            y: config.top_offset as i32,
            w: if expanded {
                (config.expanded_width as i32).max(320)
            } else {
                config.collapsed_width as i32
            },
            h: if expanded {
                EXPANDED_HEIGHT
            } else {
                config.collapsed_height as i32
            },
        };
        if self.last_placement.get() != Some(rect)
            && self.shell.borrow().send(ShellCommand::Place(rect))
        {
            self.last_placement.set(Some(rect));
        }
    }

    fn apply_fullscreen(&self) {
        let hide = shell::hide_for_fullscreen(
            self.shared.config.borrow().hide_in_fullscreen,
            self.primary_monitor.get(),
            &self.fullscreen.borrow(),
        );
        if self.shell_hidden.replace(hide) != hide {
            // set_visible maps without requesting activation; present() would
            // steal focus from the application leaving fullscreen.
            self.window.set_visible(!hide);
            if !hide {
                self.last_placement.set(None);
                self.place_shell();
            }
        }
    }

    fn mark_unsupported(&self, cap: Capability) {
        self.shell_caps
            .borrow_mut()
            .retain(|current| *current != cap);
        self.shared
            .shell_caps
            .borrow_mut()
            .retain(|current| *current != cap);
        if cap == Capability::ClipboardRead {
            self.last_clipboard_enabled.set(None);
            self.sync_clipboard_setting();
        }
        if cap == Capability::WindowActions {
            self.control.set_window_context(
                self.shared.shell_peer.borrow().clone(),
                self.focus.borrow().clone(),
                false,
            );
        }
    }

    pub fn handle_shell(&self, event: ShellEvent) {
        match event {
            ShellEvent::Connected(caps, peer) => {
                self.shared.shell_peer.replace(Some(peer));
                self.shared.shell_caps.replace(caps.clone());
                self.shell_caps.replace(caps);
                self.home.set_window_context(
                    self.focus.borrow().clone(),
                    self.shell_caps
                        .borrow()
                        .contains(&Capability::FocusTracking),
                );
                self.clipboard.context_changed();
                self.control.set_window_context(
                    self.shared.shell_peer.borrow().clone(),
                    self.focus.borrow().clone(),
                    self.shell_caps
                        .borrow()
                        .contains(&Capability::WindowActions),
                );
                self.last_clipboard_enabled.set(None);
                self.sync_clipboard_setting();
                self.last_placement.set(None);
                self.last_launcher_enabled.set(None);
                self.sync_launcher_setting();
                self.restart.set_visible(false);
                let implemented = [
                    Capability::PlaceWindows,
                    Capability::WindowActions,
                    Capability::ClipboardRead,
                    Capability::ClipboardWrite,
                    Capability::FocusTracking,
                    Capability::FullscreenTracking,
                    Capability::MaximizedTracking,
                    Capability::KeyboardLayout,
                    Capability::CapsLock,
                    Capability::GlobalShortcuts,
                ];
                let names: Vec<_> = implemented
                    .iter()
                    .filter(|cap| self.shell_caps.borrow().contains(cap))
                    .map(|cap| model::capability_text(*cap).0)
                    .collect();
                self.status
                    .set_text(&format!("GNOME подключён · {}", names.join(", ")));
                self.place_shell();
            }
            ShellEvent::Disconnected(message) => {
                self.home.set_window_context(None, false);
                self.shell_caps.borrow_mut().clear();
                self.shared.shell_caps.borrow_mut().clear();
                self.shared.shell_peer.borrow_mut().take();
                self.clipboard.context_changed();
                self.control.set_window_context(None, None, false);
                self.primary_monitor.set(None);
                self.fullscreen.borrow_mut().clear();
                self.apply_fullscreen();
                self.focus.replace(None);
                self.shell_layout.borrow_mut().clear();
                self.caps_lock.set(false);
                self.status.set_text(&message);
            }
            ShellEvent::NeedsHandover => {
                self.restart.set_visible(true);
                self.status.set_text("GNOME требует доверенный запуск. Перезапуск возможен только с вашим подтверждением.");
            }
            ShellEvent::HandoverAccepted => self.app.quit(),
            ShellEvent::WindowResult {
                request,
                peer,
                id,
                result,
            } => {
                if let Err(tidemark_core::shell::ShellError::Unsupported(cap)) = &result {
                    self.mark_unsupported(*cap);
                }
                self.control.handle_window_result(request, peer, id, result);
            }
            ShellEvent::ClipboardOffer { offer, key } => self.clipboard.offer_changed(
                key,
                offer.sensitive
                    || offer
                        .mimes
                        .iter()
                        .any(|mime| mime == "x-kde-passwordManagerHint"),
            ),
            ShellEvent::ClipboardText(key, text) => {
                self.clipboard.receive_text(key, text);
                if self.home.root.is_mapped() {
                    self.home.refresh_tiles();
                }
            }
            ShellEvent::ClipboardWritten {
                request,
                peer,
                result,
            } => {
                if let Err(tidemark_core::shell::ShellError::Unsupported(cap)) = &result {
                    self.mark_unsupported(*cap);
                }
                self.clipboard.written(request, peer, result);
            }
            ShellEvent::Focus(focus) => {
                self.home.set_window_context(
                    focus.clone(),
                    self.shell_caps
                        .borrow()
                        .contains(&Capability::FocusTracking),
                );
                self.control.set_window_context(
                    self.shared.shell_peer.borrow().clone(),
                    focus.clone(),
                    self.shell_caps
                        .borrow()
                        .contains(&Capability::WindowActions),
                );
                self.focus.replace(focus);
                self.refresh_shell_state();
            }
            ShellEvent::Layout(layout) => {
                self.shell_layout.replace(layout);
                self.refresh_shell_state();
            }
            ShellEvent::CapsLock(on) => {
                self.caps_lock.set(on);
                self.refresh_shell_state();
            }
            ShellEvent::Maximized(covered) => {
                if covered && self.shared.config.borrow().shrink_under_windows {
                    self.collapse();
                }
            }
            ShellEvent::PrimaryMonitor(monitor) => {
                self.primary_monitor.set(Some(monitor));
                self.apply_fullscreen();
            }
            ShellEvent::Fullscreen(monitor, on) => {
                if on {
                    self.fullscreen.borrow_mut().insert(monitor);
                } else {
                    self.fullscreen.borrow_mut().remove(&monitor);
                }
                self.apply_fullscreen();
            }
            ShellEvent::Shortcut(ShortcutId::Launcher) => {
                if self.launcher_enabled() {
                    if self.launcher_visible() {
                        self.close_launcher(true);
                    } else {
                        self.open_launcher();
                    }
                }
            }
            ShellEvent::Shortcut(ShortcutId::ToggleTheme) => {
                let dark = self.shared.is_dark();
                self.shared.config.borrow_mut().theme = if dark { "light" } else { "dark" }.into();
                self.shared.save_config();
                self.shared.reload_css();
                self.update_input_region();
            }
            ShellEvent::Shortcut(ShortcutId::LikeTrack) => {
                self.status
                    .set_text("Лайки Yandex Music пока недоступны; действие не выполнено.");
            }
            ShellEvent::Bindings(results) => {
                let failed: Vec<_> = results
                    .iter()
                    .filter(|(_, ok)| !ok)
                    .map(|(id, _)| id.as_str())
                    .collect();
                if !failed.is_empty() {
                    self.status
                        .set_text(&format!("Сочетания заняты: {}", failed.join(", ")));
                }
            }
            ShellEvent::Error(error) => {
                if let tidemark_core::shell::ShellError::Unsupported(cap) = &error {
                    self.mark_unsupported(*cap);
                }
                self.status.set_text(&format!("GNOME: {error}"));
            }
        }
        self.shared
            .shell_status
            .replace(self.status.text().to_string());
    }

    fn refresh_shell_state(&self) {
        let focus = self.focus.borrow();
        let title = focus
            .as_ref()
            .map(|v| v.title.chars().take(65).collect::<String>())
            .unwrap_or_else(|| "Нет активного приложения".into());
        self.status.set_text(&format!(
            "GNOME · {title} · {}{}",
            self.shell_layout.borrow(),
            if self.caps_lock.get() {
                " · Caps Lock"
            } else {
                ""
            }
        ));
    }

    fn offer_handover(&self) {
        use tidemark_core::timer::State;
        if matches!(
            self.shared.timers.borrow().state,
            State::Running { .. } | State::Paused { .. }
        ) || self.sleep_armed.get()
            || !self.text.text().is_empty()
        {
            self.status.set_text("Перед перезапуском завершите таймеры и сохраните текст из вкладки «Текст»: их перенос пока недоступен.");
            return;
        }
        let dialog = adw::MessageDialog::builder().transient_for(&self.window)
            .heading("Перезапустить с интеграцией GNOME?")
            .body("Заметки, напоминания и история будут сохранены. GNOME запустит доверенный экземпляр приложения.")
            .build();
        dialog.add_response("cancel", "Отмена");
        dialog.add_response("restart", "Сохранить и перезапустить");
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = self.weak();
        dialog.connect_response(Some("restart"), move |_, _| {
            if let Some(island) = weak.upgrade() {
                // Recheck after the dialog: a timer may have been armed meanwhile.
                if matches!(
                    island.shared.timers.borrow().state,
                    State::Running { .. } | State::Paused { .. }
                ) || island.sleep_armed.get()
                    || !island.text.text().is_empty()
                {
                    island
                        .status
                        .set_text("Активные таймеры препятствуют безопасному перезапуску.");
                    return;
                }
                island.notes.save_now();
                let saved =
                    (|| -> std::io::Result<()> {
                        island.shared.notes.borrow().save_to(
                            &tidemark_core::config::Config::data_dir().join("notes.json"),
                        )?;
                        island
                            .shared
                            .config
                            .borrow()
                            .save_to(&tidemark_core::config::Config::path())?;
                        island
                            .shared
                            .clip
                            .borrow()
                            .save_to(&tidemark_core::clip_history::dir())?;
                        Ok(())
                    })();
                if let Err(error) = saved {
                    island.status.set_text(&format!(
                        "Перезапуск отменён: не удалось сохранить данные: {error}"
                    ));
                    return;
                }
                if !island.shell.borrow().send(ShellCommand::Handover) {
                    island
                        .status
                        .set_text("Мост GNOME занят; перезапуск не запрошен. Повторите попытку.");
                }
            }
        });
        dialog.present();
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
        self.close_launcher(false);
        self.stack.set_visible_child_name(id);
        self.refresh_current();
    }

    pub fn handle_music(&self, event: &MusicEvent) {
        match event {
            MusicEvent::SleepArmed(_) => self.sleep_armed.set(true),
            MusicEvent::SleepCancelled | MusicEvent::SleepFired => self.sleep_armed.set(false),
            _ => {}
        }
        self.home.handle_event(event);
        self.update_collapsed();
    }

    pub fn tick(&self) {
        self.sync_launcher_setting();
        self.sync_clipboard_setting();
        self.update_input_region();
        self.apply_fullscreen();
        self.place_shell();
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
        // Snapshot the native window, whose CSS node paints the background
        // and whose internal children include the header. GtkWindow::child
        // exposes only an internal content node with a transparent background.
        window.clone().upcast()
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
