//! GTK application entry point, startup handshake, smoke test and the GLib
//! main-context pumps for services.

use std::cell::RefCell;
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
    agent: RefCell<Option<crate::services::agent::Service>>,
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
        agent: RefCell::new(None),
    });

    {
        let state = state.clone();
        app.connect_shutdown(move |_| {
            if let Some(agent) = state.agent.borrow_mut().take() {
                agent.shutdown();
            }
        });
    }
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
            glib::ExitCode::SUCCESS.into()
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
        let weak = Rc::downgrade(&island);
        let app = app.downgrade();
        island.window.connect_close_request(move |_| {
            if let Some(island) = weak.upgrade() {
                island.save_all();
            }
            // Closing the island ends the application even while settings
            // remain open; later activations start a fresh primary instance.
            if let Some(app) = app.upgrade() {
                app.quit();
            }
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

    match crate::services::agent::Service::start() {
        Ok(agent) => {
            island.agent.attach(agent.clone());
            state.agent.replace(Some(agent));
            let weak = Rc::downgrade(&island);
            glib::timeout_add_local(Duration::from_millis(100), move || {
                let Some(island) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                island.pump_agent();
                glib::ControlFlow::Continue
            });
        }
        Err(error) => eprintln!("Tidemark agent IPC unavailable: {error}"),
    }

    {
        let island = Rc::downgrade(&island);
        glib::timeout_add_local(Duration::from_secs(1), move || {
            let Some(island) = island.upgrade() else {
                return glib::ControlFlow::Break;
            };
            island.tick();
            glib::ControlFlow::Continue
        });
    }
    {
        let island = Rc::downgrade(&island);
        let state = Rc::downgrade(state);
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let (Some(island), Some(state)) = (island.upgrade(), state.upgrade()) else {
                return glib::ControlFlow::Break;
            };
            if let Some(receiver) = state.music_rx.borrow().as_ref() {
                while let Ok(event) = receiver.try_recv() {
                    island.handle_music(&event);
                }
            }
            glib::ControlFlow::Continue
        });
    }
    let (shell_tx, shell_rx) = mpsc::channel();
    island.attach_shell(shell::spawn(shell_tx));
    let weak = Rc::downgrade(&island);
    glib::timeout_add_local(Duration::from_millis(100), move || {
        let Some(island) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        while let Ok(event) = shell_rx.try_recv() {
            island.handle_shell(event);
        }
        glib::ControlFlow::Continue
    });
}

fn schedule_smoke(_app: &adw::Application, _state: &Rc<AppState>, island: Rc<Island>) {
    let out_dir = smoke_dir();
    let mut step = 0usize;
    let mut settings: Option<adw::PreferencesWindow> = None;
    // Every view receives real frames before allocation checks and rendering.
    glib::timeout_add_local(Duration::from_millis(400), move || {
        let result = (|| -> Result<bool, String> {
            if step == 0 {
                island.check_geometry(false)?;
                render_png(&island.root_widget(), &out_dir.join("island-compact.png"))?;
                island.expand();
                island.show_tab(model::TABS[0].0);
            } else if step <= model::TABS.len() {
                island.check_geometry(true)?;
                let id = model::TABS[step - 1].0;
                if island.stack.visible_child_name().as_deref() != Some(id) {
                    return Err(format!("tab {id} did not become visible"));
                }
                render_png(
                    &island.root_widget(),
                    &out_dir.join(format!("island-{id}.png")),
                )?;
                println!("SMOKE_TAB {id}");
                if step < model::TABS.len() {
                    island.show_tab(model::TABS[step].0);
                } else {
                    settings = Some(island.open_settings());
                }
            } else if step == model::TABS.len() + 1 {
                let window = settings.as_ref().ok_or("settings were not constructed")?;
                if !window.is_realized() || window.title().as_deref() != Some(SETTINGS_TITLE) {
                    return Err("settings window was not realized with its expected title".into());
                }
                render_png(&island.settings_root(window), &out_dir.join("settings.png"))?;
                window.close();
                island.agent.preview();
            } else if step == model::TABS.len() + 2 {
                island.check_geometry(true)?;
                if !island.agent.root.is_visible() {
                    return Err("agent preview did not render".into());
                }
                render_png(
                    &island.root_widget(),
                    &out_dir.join("island-agent-card.png"),
                )?;
                island.agent.clear();
                island.collapse();
            } else {
                island.check_geometry(false)?;
                render_png(
                    &island.root_widget(),
                    &out_dir.join("island-collapsed-again.png"),
                )?;
                if island.window.title().as_deref() != Some(ISLAND_TITLE) {
                    return Err("unexpected island window title".into());
                }
                println!("SMOKE_OK: all tabs, settings, compact/expanded/collapsed allocations and submitted input regions");
                return Ok(true);
            }
            Ok(false)
        })();
        match result {
            Ok(false) => {
                step += 1;
                glib::ControlFlow::Continue
            }
            Ok(true) => {
                let _ = std::io::Write::flush(&mut std::io::stdout());
                std::process::exit(0);
            }
            Err(error) => {
                eprintln!("SMOKE_FAIL step={step}: {error}");
                let _ = std::io::Write::flush(&mut std::io::stdout());
                std::process::exit(1);
            }
        }
    });
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
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    // Paintable dimensions include the widget's CSS border; Widget::width
    // exposes only its content box and would crop the rendered island.
    let width = paintable.intrinsic_width();
    let height = paintable.intrinsic_height();
    if width <= 0 || height <= 0 {
        return Err(format!("widget has no allocation ({width}x{height})"));
    }
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
