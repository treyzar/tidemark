//! Control tab: a real countdown timer plus honest status for desktop
//! features whose GNOME bridge is not wired into this build yet.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use tidemark_core::timer::{self, State};

use crate::model;
use crate::services::shell::{Command, Peer};
use crate::ui::{card, muted, page_header, section_title, text_button, Shared};
use tidemark_core::shell::{Capability, FocusInfo, ShellError, WindowAction, WindowState};

pub struct ControlView {
    pub root: gtk::Box,
    shared: Shared,
    clock: gtk::Label,
    state: gtk::Label,
    bridge_status: gtk::Label,
    preset: Rc<Cell<u64>>,
    window_controls: Vec<gtk::Widget>,
    window_gate: gtk::Box,
    window_title: gtk::Label,
    window_status: gtk::Label,
    above: gtk::Button,
    opacity: gtk::SpinButton,
    shown_opacity: Cell<Option<(u64, u8)>>,
    _system: crate::ui::system::SystemView,
    target: Rc<RefCell<Option<(Peer, FocusInfo)>>>,
    pending: Rc<RefCell<Option<(u64, Peer, u64)>>>,
}

impl ControlView {
    pub fn build(shared: Shared) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("tidemark-page");
        root.append(&page_header(
            "Управление",
            Some("Таймер и состояние системных возможностей"),
        ));

        let timer_card = card();
        timer_card.append(&section_title("Таймер"));
        let clock = gtk::Label::new(Some("5:00"));
        clock.set_xalign(0.5);
        clock.add_css_class("tidemark-big");
        timer_card.append(&clock);
        let state = gtk::Label::new(Some("Готов к запуску"));
        state.set_xalign(0.5);
        state.add_css_class("muted");
        timer_card.append(&state);

        let preset = Rc::new(Cell::new(5u64));
        let presets = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        presets.set_halign(gtk::Align::Center);
        for (minutes, label) in model::timer_presets() {
            let button = text_button(label);
            let shared = shared.clone();
            let clock = clock.clone();
            let state_label = state.clone();
            let selected = preset.clone();
            button.connect_clicked(move |_| {
                selected.set(minutes as u64);
                let mut timer = shared.timers.borrow_mut();
                timer.start(Duration::from_secs(minutes as u64 * 60), Instant::now());
                state_label.set_text("Идёт отсчёт");
                clock.set_text(&timer::clock(timer.left(Instant::now())));
            });
            presets.append(&button);
        }
        timer_card.append(&presets);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.set_halign(gtk::Align::Center);
        let toggle = text_button("Старт / Пауза");
        let reset = text_button("Сброс");
        actions.append(&toggle);
        actions.append(&reset);
        timer_card.append(&actions);

        {
            let shared = shared.clone();
            let clock = clock.clone();
            let state_label = state.clone();
            let selected = preset.clone();
            toggle.connect_clicked(move |_| {
                let mut timer = shared.timers.borrow_mut();
                if matches!(timer.state, State::Idle | State::Done) {
                    timer.start(Duration::from_secs(selected.get() * 60), Instant::now());
                    state_label.set_text("Идёт отсчёт");
                } else {
                    timer.toggle(Instant::now());
                    state_label.set_text(if timer.running() {
                        "Идёт отсчёт"
                    } else {
                        "Пауза"
                    });
                }
                clock.set_text(&timer::clock(timer.left(Instant::now())));
            });
        }
        {
            let shared = shared.clone();
            let clock = clock.clone();
            let state_label = state.clone();
            reset.connect_clicked(move |_| {
                shared.timers.borrow_mut().reset();
                state_label.set_text("Готов к запуску");
                clock.set_text(&timer::clock(shared.timers.borrow().length));
            });
        }
        root.append(&timer_card);

        let window_card = card();
        window_card.append(&section_title("Последнее активное окно"));
        let window_title = muted("GNOME не подключён или внешнее окно не выбрано.");
        window_title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        window_card.append(&window_title);
        let window_status = muted("Действия доступны после подтверждённого подключения GNOME.");
        let target = Rc::new(RefCell::new(None::<(Peer, FocusInfo)>));
        let pending = Rc::new(RefCell::new(None::<(u64, Peer, u64)>));
        let serial = Rc::new(Cell::new(0u64));
        let buttons = gtk::FlowBox::new();
        buttons.set_selection_mode(gtk::SelectionMode::None);
        buttons.set_min_children_per_line(1);
        buttons.set_max_children_per_line(2);
        buttons.set_column_spacing(6);
        buttons.set_row_spacing(6);
        let mut window_controls: Vec<gtk::Widget> = Vec::new();
        let above = text_button("Поверх всех окон");
        let opacity = gtk::SpinButton::with_range(10.0, 100.0, 5.0);
        opacity.set_value(100.0);
        let actions = [
            ("Активировать", WindowAction::Activate),
            ("Свернуть", WindowAction::Minimize),
            ("Развернуть / восстановить", WindowAction::ToggleMaximize),
            ("Левая половина", WindowAction::TileLeft),
            ("Правая половина", WindowAction::TileRight),
        ];
        for (label, action) in actions {
            let button = text_button(label);
            let shared = shared.clone();
            let target = target.clone();
            let pending = pending.clone();
            let serial = serial.clone();
            let status = window_status.clone();
            let gate = window_card.clone();
            button.connect_clicked(move |_| {
                submit_window(&shared, &target, &pending, &serial, &status, &gate, action);
            });
            button.set_sensitive(false);
            window_controls.push(button.clone().upcast());
            buttons.insert(&button, -1);
        }
        {
            let shared = shared.clone();
            let target = target.clone();
            let pending = pending.clone();
            let serial = serial.clone();
            let status = window_status.clone();
            let gate = window_card.clone();
            above.connect_clicked(move |_| {
                let on = target.borrow().as_ref().map(|(_, focus)| !focus.above);
                if let Some(on) = on {
                    submit_window(
                        &shared,
                        &target,
                        &pending,
                        &serial,
                        &status,
                        &gate,
                        WindowAction::SetAbove(on),
                    );
                }
            });
        }
        above.set_sensitive(false);
        window_controls.push(above.clone().upcast());
        buttons.insert(&above, -1);
        window_card.append(&buttons);
        let opacity_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let opacity_label = muted("Прозрачность, %");
        opacity_label.set_hexpand(true);
        opacity_row.append(&opacity_label);
        opacity.set_sensitive(false);
        window_controls.push(opacity.clone().upcast());
        opacity_row.append(&opacity);
        let apply = text_button("Применить");
        {
            let shared = shared.clone();
            let target = target.clone();
            let pending = pending.clone();
            let serial = serial.clone();
            let status = window_status.clone();
            let opacity = opacity.clone();
            let gate = window_card.clone();
            apply.connect_clicked(move |_| {
                submit_window(
                    &shared,
                    &target,
                    &pending,
                    &serial,
                    &status,
                    &gate,
                    WindowAction::SetOpacity(
                        (opacity.value() * 255.0 / 100.0).round().clamp(26.0, 255.0) as u8,
                    ),
                );
            });
        }
        apply.set_sensitive(false);
        window_controls.push(apply.clone().upcast());
        opacity_row.append(&apply);
        window_card.append(&opacity_row);
        window_card.append(&window_status);
        root.append(&window_card);

        let capture = text_button("Снимок экрана · OCR / QR");
        capture.set_sensitive(!shared.smoke);
        let smoke = shared.smoke;
        capture.connect_clicked(move |_| {
            if let Some(app) = gtk::gio::Application::default()
                .and_then(|app| app.downcast::<adw::Application>().ok())
            {
                crate::ui::capture::CaptureWindow::build(&app, smoke).present();
            }
        });
        root.append(&capture);
        let recording = text_button("Запись экрана · WebM без звука");
        recording.set_sensitive(!shared.smoke);
        recording.connect_clicked(move |_| {
            if let Some(app) = gtk::gio::Application::default()
                .and_then(|app| app.downcast::<adw::Application>().ok())
            {
                crate::ui::record::build(&app, smoke).present();
            }
        });
        root.append(&recording);
        let system = crate::ui::system::SystemView::build(shared.smoke);
        root.append(&system.root);

        let status_card = card();
        status_card.append(&section_title("Состояние Linux-версии"));
        let bridge_status = muted(&shared.shell_status.borrow());
        status_card.append(&bridge_status);
        status_card.append(&muted(if shared.smoke {
            "Режим проверки: MPRIS-воркер не запускается, буфер не читается."
        } else {
            "Музыка: реальный MPRIS-воркер в отдельном потоке."
        }));
        status_card.append(&muted(
            "Буфер: фоновая текстовая история через GNOME; ручное добавление при фокусе без него.",
        ));
        status_card.append(&muted(
            "Избранные приложения доступны в доке на главной странице.",
        ));
        root.append(&status_card);

        let now = Instant::now();
        let left = shared.timers.borrow().left(now);
        clock.set_text(&timer::clock(left));

        Self {
            root,
            shared,
            clock,
            state,
            bridge_status,
            preset,
            window_controls,
            window_gate: window_card,
            window_title,
            window_status,
            above,
            opacity,
            shown_opacity: Cell::new(None),
            _system: system,
            target,
            pending,
        }
    }

    pub fn set_window_context(
        &self,
        peer: Option<Peer>,
        focus: Option<FocusInfo>,
        supported: bool,
    ) {
        let next = peer.zip(focus);
        let changed = self
            .target
            .borrow()
            .as_ref()
            .map(|(p, f)| (p.clone(), f.id))
            != next.as_ref().map(|(p, f)| (p.clone(), f.id));
        self.target.replace(next);
        if changed {
            self.pending.borrow_mut().take();
        }
        self.update_window(supported);
    }
    fn update_window(&self, supported: bool) {
        self.window_gate
            .set_sensitive(self.pending.borrow().is_none());
        let target = self.target.borrow();
        if let Some((_, focus)) = target.as_ref() {
            self.window_title.set_text(&format!(
                "{} · {}",
                focus.app_id,
                focus.title.chars().take(120).collect::<String>()
            ));
            self.above.set_label(if focus.above {
                "Снять закрепление"
            } else {
                "Поверх всех окон"
            });
            let opacity_key = (focus.id, focus.opacity);
            if self.shown_opacity.get() != Some(opacity_key) {
                self.opacity
                    .set_value(f64::from(focus.opacity) * 100.0 / 255.0);
                self.shown_opacity.set(Some(opacity_key));
            }
        } else {
            self.window_title
                .set_text("Нет подтверждённого внешнего окна.");
        }
        let ready =
            supported && target.is_some() && self.pending.borrow().is_none() && !self.shared.smoke;
        for control in &self.window_controls {
            control.set_sensitive(ready);
        }
        if !supported {
            self.window_status
                .set_text("Управление окнами недоступно без поддерживающего адаптера GNOME.");
        }
    }
    pub fn handle_window_result(
        &self,
        request: u64,
        peer: Peer,
        id: u64,
        result: Result<WindowState, ShellError>,
    ) {
        if self.pending.borrow().as_ref() != Some(&(request, peer.clone(), id)) {
            return;
        }
        self.pending.borrow_mut().take();
        if let Some((current, focus)) = self.target.borrow_mut().as_mut() {
            if current == &peer && focus.id == id {
                match result {
                    Ok(state) => {
                        focus.above = state.above;
                        focus.maximized = state.maximized;
                        focus.opacity = state.opacity;
                        self.window_status.set_text(if state.minimized {
                            "GNOME подтвердил: окно свёрнуто."
                        } else {
                            "Состояние окна подтверждено GNOME."
                        });
                    }
                    Err(error) => self
                        .window_status
                        .set_text(&format!("Действие не подтверждено: {error}")),
                }
            }
        }
        self.update_window(
            self.shared
                .shell_caps
                .borrow()
                .contains(&Capability::WindowActions),
        );
    }

    /// Called once per second. Returns true on the tick where the countdown
    /// crossed zero, so the app can raise a notification exactly once.
    pub fn tick(&self) -> bool {
        self.bridge_status
            .set_text(&self.shared.shell_status.borrow());
        let now = Instant::now();
        let (left, due) = {
            let mut timer = self.shared.timers.borrow_mut();
            let due = timer.check_done(now);
            (timer.left(now), due)
        };
        if due {
            self.state.set_text("Время вышло");
            self.clock.set_text("0:00");
        } else {
            let running = self.shared.timers.borrow().running();
            if running || matches!(self.shared.timers.borrow().state, State::Paused { .. }) {
                self.clock.set_text(&timer::clock(left));
                if matches!(self.shared.timers.borrow().state, State::Paused { .. }) {
                    self.state.set_text("Пауза");
                }
            }
        }
        due
    }

    pub fn preset_minutes(&self) -> u64 {
        self.preset.get()
    }
}

fn submit_window(
    shared: &Shared,
    target: &Rc<RefCell<Option<(Peer, FocusInfo)>>>,
    pending: &Rc<RefCell<Option<(u64, Peer, u64)>>>,
    serial: &Cell<u64>,
    status: &gtk::Label,
    gate: &gtk::Box,
    action: WindowAction,
) {
    if shared.smoke || pending.borrow().is_some() {
        return;
    }
    let Some((peer, focus)) = target.borrow().clone() else {
        return;
    };
    if shared.shell_peer.borrow().as_ref() != Some(&peer)
        || !shared
            .shell_caps
            .borrow()
            .contains(&Capability::WindowActions)
    {
        status.set_text("GNOME изменился; выберите окно заново.");
        return;
    }
    let request = serial.get().wrapping_add(1);
    serial.set(request);
    if shared.shell.borrow().send(Command::Window {
        request,
        peer: peer.clone(),
        id: focus.id,
        action,
    }) {
        pending.replace(Some((request, peer, focus.id)));
        gate.set_sensitive(false);
        status.set_text("Ожидаем подтверждения GNOME…");
    } else {
        status.set_text("Мост GNOME занят; действие не отправлено.");
    }
}
