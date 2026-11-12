//! GTK application entry point, startup handshake, smoke test and the GLib
//! main-context pumps for services.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use tidemark_core::clip_history::History;
use tidemark_core::config::Config;
use tidemark_core::notes;
use tidemark_core::timer::Timer;

use crate::model;
use crate::services::music::{self, MusicEvent};
use crate::services::shell;
use crate::ui::island::Island;
use crate::ui::Shared;

const APP_ID: &str = "io.github.treyzar.Tidemark";
const ISLAND_TITLE: &str = "tidemark-island";
const SETTINGS_TITLE: &str = "Tidemark — настройки";

struct AppState {
    island: RefCell<Option<Rc<Island>>>,
    music_rx: RefCell<Option<Receiver<MusicEvent>>>,
    smoke: bool,
}

pub fn run(smoke: bool) {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if !smoke && !shell::launched_by_adapter() {
        if matches!(shell::preflight(&raw), shell::LaunchDecision::Launched) {
            return;
        }
    }

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.add_main_option(
        "settings",
        glib::Char::from(0u8),
        glib::OptionFlags::empty(),
        glib::OptionArg::None,
        "Открыть настройки",
        None,
    );

    let state = Rc::new(AppState {
        island: RefCell::new(None),
        music_rx: RefCell::new(None),
        smoke,
    });

    {
        let state = state.clone();
        app.connect_activate(move |app| ensure_ui(app, &state));
    }
    {
        let state = state.clone();
        app.connect_command_line(move |app, command_line| {
            ensure_ui(app, &state);
            let wants_settings = command_line.options_dict().contains("settings")
                || command_line
                    .arguments()
                    .iter()
                    .any(|arg| arg.to_str() == Some("--settings"));
            if wants_settings {
                if let Some(island) = state.island.borrow().as_ref() {
                    island.open_settings();
                }
            }
            glib::ExitCode::SUCCESS
        });
    }

    if smoke {
        app.run_with_args(&["tidemark"]);
        // ensure_ui always exits the process after the smoke sequence; this
        // is only reached when activation never happened.
        eprintln!("SMOKE_FAIL: GTK activation did not run");
        std::process::exit(1);
    }
    let mut arguments = vec!["tidemark".to_owned()];
    arguments.extend(raw);
    app.run_with_args(&arguments);
}

fn ensure_ui(app: &adw::Application, state: &Rc<AppState>) {
    if let Some(island) = state.island.borrow().as_ref() {
        island.window.present();
        return;
    }

    let config = Rc::new(RefCell::new(Config::load()));
    let clip = Rc::new(RefCell::new(History::load()));
    let notes = Rc::new(RefCell::new(notes::Store::load()));
    let timers = Rc::new(RefCell::new(Timer::default()));

    let (music_service, receiver) = if state.smoke {
        (music::MusicService::inactive(), None)
    } else {
        let (tx, rx) = mpsc::channel();
        (music::spawn(tx), Some(rx))
    };

    let shared = Shared::new(config, clip, notes, timers, music_service, state.smoke);
    shared.install_css();

    let island = Island::build(app, shared);
    {
        let island = island.clone();
        let window = island.window.clone();
        window.connect_close_request(move |_| {
            island.save_all();
            glib::Propagation::Proceed
        });
    }
    island.window.present();
    *state.island.borrow_mut() = Some(island.clone());
    *state.music_rx.borrow_mut() = receiver;

    if state.smoke {
        schedule_smoke(app, state, island);
        return;
    }

    {
        let island = island.clone();
        glib::timeout_add_local(Duration::from_secs(1), move || {
            island.tick();
            glib::ControlFlow::Continue
        });
    }
    {
        let island = island.clone();
        let state = state.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            if let Some(receiver) = state.music_rx.borrow().as_ref() {
                while let Ok(event) = receiver.try_recv() {
                    island.handle_music(&event);
                }
            }
            glib::ControlFlow::Continue
        });
    }
    if shell::launched_by_adapter() {
        start_shell_bridge(island);
    }
}

fn schedule_smoke(app: &adw::Application, state: &Rc<AppState>, island: Rc<Island>) {
    let app = app.clone();
    let _ = state;
    glib::timeout_add_local_once(Duration::from_millis(350), move || {
        island.expand();
        for (id, _, _) in model::TABS {
            island.show_tab(id);
        }
        island.show_tab("home");
        let settings = island.open_settings();
        glib::timeout_add_local_once(Duration::from_millis(700), move || {
            let out_dir = smoke_dir();
            let home_path = out_dir.join("island-home.png");
            let settings_path = out_dir.join("settings.png");
            let home_width = island.root_widget().width();
            let home_height = island.root_widget().height();
            match render_png(&island.root_widget(), &home_path) {
                Ok(()) => println!("SMOKE_PNG island={}", home_path.display()),
                Err(error) => eprintln!("SMOKE_PNG_SKIPPED island: {error}"),
            }
            match render_png(&island.settings_root(&settings), &settings_path) {
                Ok(()) => println!("SMOKE_PNG settings={}", settings_path.display()),
                Err(error) => eprintln!("SMOKE_PNG_SKIPPED settings: {error}"),
            }

            let title_ok = island
                .window
                .title()
                .map(|title| title.as_str() == ISLAND_TITLE)
                .unwrap_or(false);
            let settings_title_ok = settings
                .title()
                .map(|title| title.as_str() == SETTINGS_TITLE)
                .unwrap_or(false);
            let pages_ok = model::TABS
                .iter()
                .all(|(id, _, _)| island.stack.child_by_name(id).is_some());
            let geometry_ok = home_width > 0 && home_height > 0;
            let realized = island.window.is_realized() && settings.is_realized();

            println!(
                "SMOKE_STATE title={title_ok} settings={settings_title_ok} pages={pages_ok} \
                 realized={realized} geometry={home_width}x{home_height}"
            );
            if title_ok && settings_title_ok && pages_ok && realized && geometry_ok {
                let _ = std::io::Write::flush(&mut std::io::stdout());
                std::process::exit(0);
            }
            eprintln!("SMOKE_FAIL: construction checks did not pass");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            std::process::exit(1);
        });
    });
    // Keep the application value alive until the timed exit.
    let _ = app;
}

fn smoke_dir() -> PathBuf {
    let dir = std::env::var_os("TIDEMARK_SMOKE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("tidemark-smoke"));
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("SMOKE_PNG_DIR: cannot create {}: {error}", dir.display());
    }
    dir
}

fn render_png(widget: &gtk::Widget, path: &Path) -> Result<(), String> {
    let width = widget.width();
    let height = widget.height();
    if width <= 0 || height <= 0 {
        return Err(format!("widget has no allocation ({width}x{height})"));
    }
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width as f64, height as f64);
    let node = snapshot
        .to_node()
        .ok_or_else(|| "empty render node".to_string())?;
    let native = widget
        .native()
        .ok_or_else(|| "widget is not in a native surface".to_string())?;
    let renderer = native
        .renderer()
        .ok_or_else(|| "native surface has no renderer".to_string())?;
    let texture = renderer.render_texture(&node, None);
    texture
        .save_to_png(path)
        .map_err(|error| format!("save {}: {error}", path.display()))
}

fn start_shell_bridge(island: Rc<Island>) {
    // Hello is performed on a worker so the GTK loop never blocks on D-Bus.
    // The UI currently keeps GNOME-only actions disabled until the bridge is
    // complete; this call establishes the authenticated, unicast connection.
    let _ = island;
    std::thread::Builder::new()
        .name("tidemark-shell".into())
        .spawn(|| {
            if let Err(error) = shell::hello() {
                eprintln!("Tidemark shell adapter: {error}");
            }
        })
        .ok();
}
