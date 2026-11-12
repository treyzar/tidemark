//! Explicit system controls. Host work never runs on the GTK thread.
use gtk::{glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use tidemark_system::{
    brightness::{self, Brightness},
    client::{Battery, BatteryStatus, BluetoothAdapter, WifiState},
    metrics::{self, CpuSample, Memory},
    volume::{self, Volume},
    Client, Error, Result,
};
#[path = "../services/system_state.rs"]
mod state;
use state::{Request, State};

pub struct SystemView {
    pub root: gtk::Box,
    inner: Rc<Inner>,
}
struct Data {
    volume: Result<Volume>,
    wifi: Result<WifiState>,
    bluetooth: Result<Vec<BluetoothAdapter>>,
    brightness: Result<Vec<Brightness>>,
    battery: Result<Battery>,
    memory: Result<Memory>,
    cpu: Result<CpuSample>,
}
impl Default for Data {
    fn default() -> Self {
        Self {
            volume: Err(Error::Unsupported),
            wifi: Err(Error::Unsupported),
            bluetooth: Err(Error::Unsupported),
            brightness: Err(Error::Unsupported),
            battery: Err(Error::Unsupported),
            memory: Err(Error::Unsupported),
            cpu: Err(Error::NotReady),
        }
    }
}
enum Action {
    Refresh,
    Volume(Volume, f64),
    Mute(Volume, bool),
    Wifi(WifiState, bool),
    Bluetooth(BluetoothAdapter, bool),
    Brightness(Brightness, u32),
}
enum Outcome {
    Snapshot(Data),
    Volume(Result<Volume>),
    Wifi(Result<WifiState>),
    Bluetooth(BluetoothAdapter, Result<BluetoothAdapter>),
    Brightness(Result<Brightness>),
}
struct Job {
    request: Request,
    rx: mpsc::Receiver<Outcome>,
    cancel: Arc<AtomicBool>,
}
struct Inner {
    smoke: bool,
    state: RefCell<State>,
    job: RefCell<Option<Job>>,
    data: RefCell<Data>,
    next: Cell<Instant>,
    updating: Cell<bool>,
    volume: gtk::Scale,
    volume_status: gtk::Label,
    volume_apply: gtk::Button,
    mute: gtk::Button,
    volume_target: RefCell<Option<Volume>>,
    wifi_status: gtk::Label,
    wifi: gtk::Button,
    bluetooth: gtk::Box,
    brightness: gtk::Scale,
    brightness_status: gtk::Label,
    brightness_apply: gtk::Button,
    brightness_target: RefCell<Option<Brightness>>,
    stats: gtk::Label,
    status: gtk::Label,
    refresh: gtk::Button,
    previous_cpu: Cell<Option<CpuSample>>,
    cpu_percent: Cell<Option<f64>>,
}
impl SystemView {
    /// Keep this handle alive while the mounted widget is in use.
    pub fn build(smoke: bool) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let sound = crate::ui::card();
        sound.append(&crate::ui::section_title("Звук компьютера"));
        let volume_status = label("Нет данных о громкости");
        sound.append(&volume_status);
        let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 100., 1.);
        volume.set_draw_value(true);
        volume.set_digits(0);
        volume.set_hexpand(true);
        sound.append(&volume);
        let sound_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let volume_apply = crate::ui::text_button("Применить громкость");
        let mute = crate::ui::text_button("Выключить звук");
        sound_actions.append(&volume_apply);
        sound_actions.append(&mute);
        sound.append(&sound_actions);
        root.append(&sound);

        let radios = crate::ui::card();
        radios.append(&crate::ui::section_title("Подключения"));
        let wifi_status = label("Wi-Fi: нет данных");
        let wifi = crate::ui::text_button("Включить Wi-Fi");
        radios.append(&wifi_status);
        radios.append(&wifi);
        let bluetooth = gtk::Box::new(gtk::Orientation::Vertical, 6);
        radios.append(&bluetooth);
        root.append(&radios);

        let screen = crate::ui::card();
        screen.append(&crate::ui::section_title("Яркость экрана"));
        let brightness_status = label("Регулировка яркости недоступна");
        screen.append(&brightness_status);
        let brightness = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 100., 1.);
        brightness.set_draw_value(true);
        brightness.set_digits(0);
        screen.append(&brightness);
        let brightness_apply = crate::ui::text_button("Применить яркость");
        screen.append(&brightness_apply);
        root.append(&screen);

        let monitor = crate::ui::card();
        monitor.append(&crate::ui::section_title("Состояние компьютера"));
        let stats = label("Нет данных о системе");
        monitor.append(&stats);
        root.append(&monitor);
        let status = label(if smoke {
            "Проверка интерфейса: доступ к системе отключён"
        } else {
            "Читаем состояние системы…"
        });
        let refresh = crate::ui::text_button("Обновить состояние");
        root.append(&refresh);
        root.append(&status);
        let inner = Rc::new(Inner {
            smoke,
            state: RefCell::new(State::default()),
            job: RefCell::new(None),
            data: RefCell::new(Data::default()),
            next: Cell::new(Instant::now()),
            updating: Cell::new(false),
            volume,
            volume_status,
            volume_apply,
            mute,
            volume_target: RefCell::new(None),
            wifi_status,
            wifi,
            bluetooth,
            brightness,
            brightness_status,
            brightness_apply,
            brightness_target: RefCell::new(None),
            stats,
            status,
            refresh,
            previous_cpu: Cell::new(None),
            cpu_percent: Cell::new(None),
        });
        inner.connect();
        inner.render();
        if !smoke {
            inner.start(Action::Refresh);
        }
        let weak = Rc::downgrade(&inner);
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(inner) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            inner.tick();
            glib::ControlFlow::Continue
        });
        Self { root, inner }
    }
    pub fn tick(&self) {
        self.inner.tick();
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.state.get_mut().close();
        if let Some(job) = self.job.get_mut() {
            job.cancel.store(true, Ordering::Release);
        }
    }
}
impl Inner {
    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.volume.connect_value_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                if !this.updating.get() {
                    *this.volume_target.borrow_mut() =
                        this.data.borrow().volume.as_ref().ok().copied();
                    this.sensitivity();
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.volume_apply.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                let target = *this.volume_target.borrow();
                if let Some(target) = target {
                    this.start(Action::Volume(target, this.volume.value() / 100.));
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.mute.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                let target = this.data.borrow().volume.as_ref().ok().copied();
                if let Some(target) = target {
                    this.start(Action::Mute(target, !target.muted));
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.wifi.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                let target = this.data.borrow().wifi.as_ref().ok().cloned();
                if let Some(target) = target {
                    let want = !target.enabled;
                    this.start(Action::Wifi(target, want));
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.brightness.connect_value_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                if !this.updating.get() {
                    *this.brightness_target.borrow_mut() = this
                        .data
                        .borrow()
                        .brightness
                        .as_ref()
                        .ok()
                        .and_then(|v| v.first())
                        .cloned();
                    this.sensitivity();
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.brightness_apply.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                let target = this.brightness_target.borrow().clone();
                if let Some(target) = target {
                    let want =
                        (this.brightness.value() / 100. * f64::from(target.maximum)).round() as u32;
                    this.start(Action::Brightness(target, want));
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.refresh.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.start(Action::Refresh);
            }
        });
    }
    fn start(self: &Rc<Self>, action: Action) {
        if self.smoke {
            return;
        }
        let Some(request) = self.state.borrow_mut().begin() else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let started = std::thread::Builder::new()
            .name("system-controls".into())
            .spawn(move || {
                if worker_cancel.load(Ordering::Acquire) {
                    return;
                }
                let outcome = perform(action, &worker_cancel);
                let _ = tx.try_send(outcome);
            });
        if started.is_err() {
            self.state.borrow_mut().finish(request);
            self.status
                .set_text("Не удалось запустить управление системой");
            self.next.set(Instant::now() + Duration::from_secs(30));
            self.sensitivity();
            return;
        }
        *self.job.borrow_mut() = Some(Job {
            request,
            rx,
            cancel,
        });
        self.status.set_text("Проверяем и подтверждаем состояние…");
        self.sensitivity();
    }
    fn tick(self: &Rc<Self>) {
        let completion = self
            .job
            .borrow()
            .as_ref()
            .and_then(|j| match j.rx.try_recv() {
                Ok(outcome) => Some((j.request, Some(outcome))),
                Err(mpsc::TryRecvError::Disconnected) => Some((j.request, None)),
                Err(mpsc::TryRecvError::Empty) => None,
            });
        if let Some((request, outcome)) = completion {
            self.job.borrow_mut().take();
            if self.state.borrow_mut().finish(request) {
                if let Some(outcome) = outcome {
                    let message = match &outcome {
                        Outcome::Snapshot(_) => "Состояние обновлено",
                        Outcome::Volume(Ok(_))
                        | Outcome::Wifi(Ok(_))
                        | Outcome::Bluetooth(_, Ok(_))
                        | Outcome::Brightness(Ok(_)) => "Изменение подтверждено",
                        _ => "Изменение не подтверждено. Проверьте состояние устройства",
                    };
                    let mut data = self.data.borrow_mut();
                    match outcome {
                        Outcome::Snapshot(snapshot) => {
                            if let Ok(cpu) = snapshot.cpu.as_ref() {
                                let cpu = *cpu;
                                self.cpu_percent.set(
                                    self.previous_cpu
                                        .replace(Some(cpu))
                                        .and_then(|old| old.utilization(cpu).ok()),
                                );
                            } else {
                                self.cpu_percent.set(None);
                                self.previous_cpu.set(None);
                            }
                            *data = snapshot;
                        }
                        Outcome::Volume(result) => {
                            data.volume = result;
                            self.volume_target.borrow_mut().take();
                        }
                        Outcome::Wifi(result) => data.wifi = result,
                        Outcome::Bluetooth(target, result) => {
                            if let Ok(adapters) = &mut data.bluetooth {
                                if let Some(index) =
                                    adapters.iter().position(|a| a.path == target.path)
                                {
                                    match result {
                                        Ok(adapter) => adapters[index] = adapter,
                                        Err(error) => data.bluetooth = Err(error),
                                    }
                                }
                            }
                        }
                        Outcome::Brightness(result) => {
                            data.brightness = result.map(|target| vec![target]);
                            self.brightness_target.borrow_mut().take();
                        }
                    }
                    drop(data);
                    self.status.set_text(message);
                } else {
                    *self.data.borrow_mut() = Data::default();
                    self.previous_cpu.set(None);
                    self.cpu_percent.set(None);
                    self.volume_target.borrow_mut().take();
                    self.brightness_target.borrow_mut().take();
                    self.status
                        .set_text("Управление системой прервано. Обновите состояние");
                }
                self.next.set(Instant::now() + Duration::from_secs(5));
                self.render();
            }
        }
        if !self.smoke && !self.state.borrow().busy() && Instant::now() >= self.next.get() {
            self.start(Action::Refresh);
        }
    }
    fn render(self: &Rc<Self>) {
        let data = self.data.borrow();
        self.updating.set(true);
        match &data.volume {
            Ok(value) => {
                self.volume_status.set_text(&format!(
                    "Сейчас {:.0}% · {}",
                    value.level * 100.,
                    if value.muted {
                        "звук выключен"
                    } else {
                        "звук включён"
                    }
                ));
                self.mute.set_label(if value.muted {
                    "Включить звук"
                } else {
                    "Выключить звук"
                });
                if self.volume_target.borrow().is_none() {
                    self.volume.set_value(value.level.min(1.) * 100.);
                }
            }
            Err(error) => self
                .volume_status
                .set_text(&format!("Громкость: {}", error_text(error))),
        }
        match &data.wifi {
            Ok(value) => {
                self.wifi_status.set_text(if !value.hardware_enabled {
                    "Wi-Fi отключён аппаратным переключателем"
                } else if value.enabled {
                    "Wi-Fi включён"
                } else {
                    "Wi-Fi выключен"
                });
                self.wifi.set_label(if value.enabled {
                    "Выключить Wi-Fi"
                } else {
                    "Включить Wi-Fi"
                });
            }
            Err(error) => self
                .wifi_status
                .set_text(&format!("Wi-Fi: {}", error_text(error))),
        }
        while let Some(child) = self.bluetooth.first_child() {
            self.bluetooth.remove(&child);
        }
        match &data.bluetooth {
            Ok(adapters) => {
                for target in adapters {
                    let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
                    let name = target.alias.as_deref().unwrap_or("Bluetooth");
                    row.append(&label(&format!(
                        "{name} · {}",
                        if target.powered {
                            "включён"
                        } else {
                            "выключен"
                        }
                    )));
                    let button = crate::ui::text_button(if target.powered {
                        "Выключить Bluetooth"
                    } else {
                        "Включить Bluetooth"
                    });
                    button.set_sensitive(!self.smoke && !self.state.borrow().busy());
                    let weak = Rc::downgrade(self);
                    let target = target.clone();
                    button.connect_clicked(move |_| {
                        if let Some(this) = weak.upgrade() {
                            this.start(Action::Bluetooth(target.clone(), !target.powered));
                        }
                    });
                    row.append(&button);
                    self.bluetooth.append(&row);
                }
            }
            Err(error) => self
                .bluetooth
                .append(&label(&format!("Bluetooth: {}", error_text(error)))),
        }
        match data.brightness.as_ref().ok().and_then(|v| v.first()) {
            Some(value) => {
                self.brightness_status.set_text(&format!(
                    "Сейчас {:.0}%",
                    f64::from(value.current) * 100. / f64::from(value.maximum)
                ));
                if self.brightness_target.borrow().is_none() {
                    self.brightness
                        .set_value(f64::from(value.current) * 100. / f64::from(value.maximum));
                }
            }
            None => self.brightness_status.set_text(&format!(
                "Яркость: {}",
                data.brightness
                    .as_ref()
                    .err()
                    .map(error_text)
                    .unwrap_or("устройство недоступно")
            )),
        }
        let cpu = match &data.cpu {
            Ok(_) => self
                .cpu_percent
                .get()
                .map(|v| format!("Процессор: {v:.0}%"))
                .unwrap_or_else(|| "Процессор: нужен ещё один замер".into()),
            Err(error) => format!("Процессор: {}", error_text(error)),
        };
        let ram = data
            .memory
            .as_ref()
            .map(|m| {
                format!(
                    "Память: {:.1} / {:.1} ГБ",
                    m.used_bytes() as f64 / 1_073_741_824.,
                    m.total_bytes as f64 / 1_073_741_824.
                )
            })
            .unwrap_or_else(|e| format!("Память: {}", error_text(e)));
        let battery = data
            .battery
            .as_ref()
            .map(|b| {
                format!(
                    "Батарея: {:.0}% · {}",
                    b.percentage,
                    battery_status(b.status)
                )
            })
            .unwrap_or_else(|e| format!("Батарея: {}", error_text(e)));
        self.stats.set_text(&format!("{cpu}\n{ram}\n{battery}"));
        self.updating.set(false);
        drop(data);
        self.sensitivity();
    }
    fn sensitivity(&self) {
        let data = self.data.borrow();
        let enabled = !self.smoke && !self.state.borrow().busy();
        self.refresh.set_sensitive(enabled);
        self.volume.set_visible(data.volume.is_ok());
        self.volume.set_sensitive(enabled && data.volume.is_ok());
        self.volume_apply
            .set_sensitive(enabled && data.volume.is_ok() && self.volume_target.borrow().is_some());
        self.mute.set_sensitive(enabled && data.volume.is_ok());
        self.wifi
            .set_sensitive(enabled && data.wifi.as_ref().is_ok_and(|w| w.hardware_enabled));
        let backlight = data.brightness.as_ref().is_ok_and(|v| !v.is_empty());
        self.brightness.set_visible(backlight);
        self.brightness.set_sensitive(enabled && backlight);
        self.brightness_apply
            .set_sensitive(enabled && backlight && self.brightness_target.borrow().is_some());
        // Dynamically built adapter buttons must also stop accepting commands.
        let mut row = self.bluetooth.first_child();
        while let Some(widget) = row {
            row = widget.next_sibling();
            widget.set_sensitive(enabled);
        }
    }
}
fn label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_max_width_chars(1);
    label.add_css_class("tiny");
    label
}
fn error_text(error: &Error) -> &'static str {
    match error {
        Error::Unsupported => "недоступно",
        Error::Denied => "нет разрешения",
        Error::Timeout => "время ожидания истекло",
        Error::Changed => "устройство изменилось — обновите состояние",
        Error::NotConfirmed => "изменение не подтверждено",
        Error::NotReady => "нужен ещё один замер",
        _ => "не удалось получить состояние",
    }
}
fn battery_status(status: BatteryStatus) -> &'static str {
    match status {
        BatteryStatus::Unknown => "состояние неизвестно",
        BatteryStatus::Charging => "заряжается",
        BatteryStatus::Discharging => "разряжается",
        BatteryStatus::Empty => "разряжена",
        BatteryStatus::FullyCharged => "заряжена",
        BatteryStatus::PendingCharge => "ожидает зарядки",
        BatteryStatus::PendingDischarge => "ожидает разрядки",
    }
}
fn perform(action: Action, canceled: &AtomicBool) -> Outcome {
    match action {
        Action::Refresh => {
            let mut data = Data::default();
            let client = Client::connect();
            data.volume = volume::read();
            if canceled.load(Ordering::Acquire) {
                return Outcome::Snapshot(data);
            }
            data.wifi = client.as_ref().map_err(Clone::clone).and_then(Client::wifi);
            if canceled.load(Ordering::Acquire) {
                return Outcome::Snapshot(data);
            }
            data.bluetooth = client
                .as_ref()
                .map_err(Clone::clone)
                .and_then(Client::bluetooth);
            if canceled.load(Ordering::Acquire) {
                return Outcome::Snapshot(data);
            }
            data.battery = client
                .as_ref()
                .map_err(Clone::clone)
                .and_then(Client::battery);
            data.brightness = brightness::inventory();
            data.cpu = metrics::cpu();
            data.memory = metrics::memory();
            Outcome::Snapshot(data)
        }
        Action::Volume(target, level) => Outcome::Volume(volume::set_level(&target, level)),
        Action::Mute(target, want) => Outcome::Volume(volume::set_muted(&target, want)),
        Action::Wifi(target, want) => {
            Outcome::Wifi(Client::connect().and_then(|c| c.set_wifi(&target, want)))
        }
        Action::Bluetooth(target, want) => {
            let result = Client::connect().and_then(|c| c.set_bluetooth_powered(&target, want));
            Outcome::Bluetooth(target, result)
        }
        Action::Brightness(target, want) => {
            Outcome::Brightness(Client::connect().and_then(|c| c.set_brightness(&target, want)))
        }
    }
}
