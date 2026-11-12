//! GTK4 + libadwaita views for the Linux island.

pub mod agent;
pub mod clipboard;
pub mod control;
pub mod home;
pub mod island;
pub mod launcher;
pub mod notes;
pub mod settings;
pub mod text;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gtk::prelude::*;
use tidemark_core::clip_history::History;
use tidemark_core::config::{self, Config};
use tidemark_core::notes as core_notes;
use tidemark_core::timer::Timer;

use crate::services::music::MusicService;

/// State shared by every tab. Core stores are the single source of truth;
/// there are no copies of the config model in the UI.
#[derive(Clone)]
pub struct Shared {
    pub config: Rc<RefCell<Config>>,
    pub clip: Rc<RefCell<History>>,
    pub notes: Rc<RefCell<core_notes::Store>>,
    pub timers: Rc<RefCell<Timer>>,
    pub music: MusicService,
    pub smoke: bool,
    pub shell_status: Rc<RefCell<String>>,
    css: Rc<gtk::CssProvider>,
}

impl Shared {
    pub fn new(
        config: Rc<RefCell<Config>>,
        clip: Rc<RefCell<History>>,
        notes: Rc<RefCell<core_notes::Store>>,
        timers: Rc<RefCell<Timer>>,
        music: MusicService,
        smoke: bool,
    ) -> Self {
        Self {
            config,
            clip,
            notes,
            timers,
            music,
            smoke,
            shell_status: Rc::new(RefCell::new("GNOME: ожидание адаптера".into())),
            css: Rc::new(gtk::CssProvider::new()),
        }
    }

    pub fn install_css(&self) {
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                self.css.as_ref(),
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        self.reload_css();
    }

    pub fn reload_css(&self) {
        let css = build_css(&self.config.borrow(), is_dark(&self.config.borrow()));
        self.css.load_from_string(&css);
    }

    pub fn save_config(&self) {
        self.config.borrow().save();
    }

    pub fn is_dark(&self) -> bool {
        is_dark(&self.config.borrow())
    }

    pub fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

pub fn is_dark(config: &Config) -> bool {
    match config.theme.as_str() {
        "dark" => true,
        "light" => false,
        _ => adw::StyleManager::default().is_dark(),
    }
}

fn build_css(config: &Config, dark: bool) -> String {
    let accent = config::parse_hex(&config.accent)
        .map(|_| normalize(&config.accent))
        .unwrap_or_else(|| "#7C6CF2".into());
    let fallback_bg = if dark { "#0E0E12" } else { "#F7F7FA" };
    let background = config::parse_hex(&config.background)
        .map(|_| normalize(&config.background))
        .unwrap_or_else(|| fallback_bg.into());
    let text = if dark { "#F2F2F5" } else { "#1B1B21" };
    let subtext = if dark {
        "rgba(255,255,255,0.55)"
    } else {
        "rgba(0,0,0,0.55)"
    };
    let tile = if dark {
        "rgba(255,255,255,0.07)"
    } else {
        "rgba(0,0,0,0.05)"
    };
    let hover = if dark {
        "rgba(255,255,255,0.13)"
    } else {
        "rgba(0,0,0,0.10)"
    };
    let border = if dark {
        "rgba(255,255,255,0.10)"
    } else {
        "rgba(0,0,0,0.10)"
    };
    let opacity = config.opacity.clamp(0.2, 1.0);
    let bg = with_alpha(&background, opacity);
    let radius = config.expanded_radius.clamp(0.0, 64.0);

    format!(
        r#"
window.tidemark-window,
window.tidemark-window.csd,
window.tidemark-window.solid-csd {{
    background: transparent;
    border: none;
    box-shadow: none;
    border-radius: 0;
    padding: 0;
    margin: 0;
    min-width: 0;
    min-height: 0;
}}
.tidemark-island {{
    background: {bg};
    color: {text};
    border: 1px solid {border};
    border-radius: 18px;
    padding: 0;
}}
.tidemark-island.expanded {{ border-radius: {radius}px; }}
.tidemark-collapsed {{ padding: 8px 14px; }}
.tidemark-clock {{ font-weight: 600; font-size: 14px; }}
.tidemark-mini {{ color: {subtext}; font-size: 12px; }}
.tidemark-header {{ padding: 10px 14px 4px 14px; }}
.tidemark-title {{ font-weight: 600; font-size: 16px; }}
.tidemark-section {{ color: {subtext}; font-weight: 500; font-size: 12px; }}
.muted {{ color: {subtext}; font-size: 12px; }}
.tiny {{ color: {subtext}; font-size: 11px; }}
.accent {{ color: {accent}; }}
.danger {{ color: #E5484D; }}
.tile {{
    background: {tile};
    border-radius: 14px;
    padding: 10px 12px;
}}
.tile:hover {{ background: {hover}; }}
.tile-title {{ font-weight: 600; font-size: 13px; }}
.tile-value {{ font-size: 18px; font-weight: 600; }}
.tidemark-list {{ background: transparent; }}
.tidemark-row {{ padding: 8px 6px; border-bottom: 1px solid {border}; }}
.tidemark-row:hover {{ background: {tile}; }}
.cover {{ border-radius: 10px; }}
.tidemark-status {{ padding: 4px 14px 10px 14px; color: {subtext}; font-size: 11px; }}
.tidemark-page {{ padding: 6px 14px 14px 14px; }}
.tidemark-big {{ font-size: 40px; font-weight: 600; }}
progressbar.tidemark {{ min-height: 4px; }}
"#
    )
}

fn normalize(hex: &str) -> String {
    let hex = hex.trim().trim_start_matches('#').to_uppercase();
    format!("#{hex}")
}

fn with_alpha(hex: &str, alpha: f32) -> String {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return format!("rgba(14,14,18,{alpha:.2})");
    }
    match u32::from_str_radix(hex, 16) {
        Ok(value) => {
            let r = (value >> 16) & 0xFF;
            let g = (value >> 8) & 0xFF;
            let b = value & 0xFF;
            format!("rgba({r},{g},{b},{alpha:.2})")
        }
        Err(_) => format!("rgba(14,14,18,{alpha:.2})"),
    }
}

pub fn page_header(title: &str, subtitle: Option<&str>) -> gtk::Box {
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 2);
    bx.add_css_class("tidemark-page");
    let label = gtk::Label::new(Some(title));
    label.set_xalign(0.0);
    label.add_css_class("tidemark-title");
    bx.append(&label);
    if let Some(subtitle) = subtitle {
        let sub = gtk::Label::new(Some(subtitle));
        sub.set_xalign(0.0);
        sub.set_wrap(true);
        sub.add_css_class("muted");
        bx.append(&sub);
    }
    bx
}

pub fn card() -> gtk::Box {
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 8);
    bx.add_css_class("tile");
    bx
}

pub fn section_title(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("tidemark-section");
    label
}

pub fn muted(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.add_css_class("muted");
    label
}

pub fn icon_button(icon: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(label));
    button.set_cursor_from_name(Some("pointer"));
    button.set_accessible_role(gtk::AccessibleRole::Button);
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    button
}

pub fn text_button(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.set_cursor_from_name(Some("pointer"));
    button.set_accessible_role(gtk::AccessibleRole::Button);
    button
}

pub fn scroll(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scrolled.set_child(Some(child));
    scrolled.set_vexpand(true);
    scrolled
}

/// Cover native Adwaita internals and controls created after the first map,
/// while preserving text-entry I-beams and other input-specific cursors.
pub fn install_hand_cursors(root: &impl IsA<gtk::Widget>) {
    let root = root.as_ref();
    apply_hand_cursors(root);
    root.connect_map(|root| apply_hand_cursors(root));
    let weak = root.downgrade();
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
        let Some(root) = weak.upgrade() else {
            return gtk::glib::ControlFlow::Break;
        };
        if root.is_visible() {
            apply_hand_cursors(&root);
        }
        gtk::glib::ControlFlow::Continue
    });
}
fn apply_hand_cursors(widget: &gtk::Widget) {
    let clickable = widget.is::<gtk::Button>()
        || widget.is::<gtk::Switch>()
        || widget.is::<gtk::DropDown>()
        || widget.is::<gtk::MenuButton>()
        || widget.css_name().as_str() == "button";
    if clickable && widget.cursor().and_then(|cursor| cursor.name()).as_deref() != Some("pointer") {
        widget.set_cursor_from_name(Some("pointer"));
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        apply_hand_cursors(&widget);
    }
}
