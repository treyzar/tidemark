//! Control tab: a real countdown timer plus honest status for desktop
//! features whose GNOME bridge is not wired into this build yet.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use tidemark_core::timer::{self, State};

use crate::model;
use crate::ui::{card, muted, page_header, section_title, text_button, Shared};

pub struct ControlView {
    pub root: gtk::Box,
    shared: Shared,
    clock: gtk::Label,
    state: gtk::Label,
    bridge_status: gtk::Label,
    preset: Rc<Cell<u64>>,
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
        window_card.append(&section_title("Окно и система"));
        window_card.append(&muted(
            "Состояние GNOME показано ниже. Управление чужими окнами, захват экрана \
             и поиск ещё не подключены; кнопки этих действий выключены.",
        ));
        for label in [
            "Поверх всех окон",
            "Половина экрана",
            "Прозрачность окна",
            "Снимок экрана и OCR",
            "Поиск приложений по Alt+Space",
        ] {
            let button = text_button(label);
            button.set_sensitive(false);
            button.set_tooltip_text(Some("Действие ещё не подключено в этой сборке."));
            window_card.append(&button);
        }
        root.append(&window_card);

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
            "Буфер обмена: чтение только при фокусе, ручное добавление.",
        ));
        status_card.append(&muted("Док приложений пока недоступен в этой сборке."));
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
        }
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
