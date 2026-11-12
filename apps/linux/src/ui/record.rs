//! Explicit, video-only recording window. Construction never starts a portal.
use adw::prelude::*;
use gtk::glib;
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tidemark_record::{Error, Options, Recorder, Sources, Status};
#[path = "../services/record_state.rs"]
mod state;
use state::{Phase, Session, Stop};

static SHUTDOWN_HOOK: std::sync::Once = std::sync::Once::new();

/// Install before recording can start. GApplication::quit emits shutdown even
/// when individual windows never receive close_request.
pub fn install_shutdown_hook(app: &adw::Application) {
    SHUTDOWN_HOOK.call_once(|| {
        app.connect_shutdown(|_| {
            // Do not wait for portal consent or a UI worker. Existing encoders
            // get a short EOS window, then an owned-process abort fallback.
            let _ = Recorder::shutdown_all(Duration::from_secs(2));
        });
    });
}

/// Inert in both normal and smoke construction; only Start performs capture.
pub fn build(app: &adw::Application, smoke: bool) -> adw::ApplicationWindow {
    if !smoke {
        install_shutdown_hook(app);
    }
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Tidemark — запись экрана")
        .default_width(640)
        .default_height(540)
        .build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(
        "Запись экрана",
        "Видео без звука",
    )));
    root.append(&header);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_start(20);
    content.set_margin_end(20);
    content.set_margin_top(8);
    content.set_margin_bottom(20);
    root.append(&content);
    let intro = label("Экран или окно сохраняется в WebM. Источник выбирается системным диалогом. Звук не записывается.");
    content.append(&intro);
    let source = gtk::DropDown::from_strings(&["Экран или окно", "Только экран", "Только окно"]);
    source.set_tooltip_text(Some("Что предложить в системном диалоге"));
    content.append(&source);
    let options = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    options.append(&label("Частота кадров"));
    let fps = gtk::DropDown::from_strings(&["24 кадр/с", "30 кадр/с", "60 кадр/с"]);
    fps.set_selected(1);
    options.append(&fps);
    let cursor_label = label("Показывать курсор");
    cursor_label.set_hexpand(true);
    options.append(&cursor_label);
    let cursor = gtk::Switch::new();
    cursor.set_active(true);
    cursor.set_valign(gtk::Align::Center);
    options.append(&cursor);
    content.append(&options);
    let file_label = label("Выберите имя нового файла .webm");
    file_label.set_selectable(true);
    content.append(&file_label);
    let choose = gtk::Button::with_label("Выбрать файл…");
    content.append(&choose);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let start = gtk::Button::with_label("Начать запись");
    start.add_css_class("suggested-action");
    let stop = gtk::Button::with_label("Остановить и сохранить");
    actions.append(&start);
    actions.append(&stop);
    content.append(&actions);
    let elapsed = label("00:00");
    elapsed.add_css_class("monospace");
    content.append(&elapsed);
    let status = label(if smoke {
        "Проверка интерфейса: захват и запись отключены"
    } else {
        "Готово к записи видео без звука"
    });
    content.append(&status);
    window.set_content(Some(&root));
    let ui = Rc::new(Ui {
        window: window.downgrade(),
        source: source.downgrade(),
        fps: fps.downgrade(),
        cursor: cursor.downgrade(),
        choose: choose.downgrade(),
        start: start.downgrade(),
        stop: stop.downgrade(),
        file_label: file_label.downgrade(),
        elapsed: elapsed.downgrade(),
        status: status.downgrade(),
        path: RefCell::new(None),
        chooser: RefCell::new(None),
        session: RefCell::new(Session::default()),
        control: RefCell::new(None),
        began: Cell::new(None),
        smoke,
    });
    for button in [&choose, &start, &stop] {
        button.set_cursor_from_name(Some("pointer"));
    }
    {
        let ui = ui.clone();
        choose.connect_clicked(move |_| ui.choose_file());
    }
    {
        let ui = ui.clone();
        start.connect_clicked(move |_| ui.start());
    }
    {
        let ui = ui.clone();
        stop.connect_clicked(move |_| ui.stop());
    }
    {
        let ui = ui.clone();
        window.connect_close_request(move |_| {
            ui.close();
            glib::Propagation::Proceed
        });
    }
    ui.update_actions();
    crate::ui::install_hand_cursors(&window);
    window
}

struct Ui {
    window: glib::WeakRef<adw::ApplicationWindow>,
    source: glib::WeakRef<gtk::DropDown>,
    fps: glib::WeakRef<gtk::DropDown>,
    cursor: glib::WeakRef<gtk::Switch>,
    choose: glib::WeakRef<gtk::Button>,
    start: glib::WeakRef<gtk::Button>,
    stop: glib::WeakRef<gtk::Button>,
    file_label: glib::WeakRef<gtk::Label>,
    elapsed: glib::WeakRef<gtk::Label>,
    status: glib::WeakRef<gtk::Label>,
    path: RefCell<Option<PathBuf>>,
    chooser: RefCell<Option<gtk::FileChooserNative>>,
    session: RefCell<Session>,
    control: RefCell<Option<Arc<AtomicU8>>>,
    began: Cell<Option<Instant>>,
    smoke: bool,
}
#[derive(Clone)]
struct Progress {
    phase: Phase,
    result: Option<Result<PathBuf, Error>>,
}
impl Ui {
    fn message(&self, text: &str, error: bool) {
        if let Some(status) = self.status.upgrade() {
            status.set_text(text);
            if error {
                status.add_css_class("error");
            } else {
                status.remove_css_class("error");
            }
        }
    }
    fn update_actions(&self) {
        let phase = self.session.borrow().phase();
        let idle = phase == Phase::Idle && !self.smoke;
        if let Some(button) = self.choose.upgrade() {
            button.set_sensitive(idle);
        }
        if let Some(button) = self.start.upgrade() {
            button.set_sensitive(idle && self.path.borrow().is_some());
        }
        if let Some(button) = self.stop.upgrade() {
            button
                .set_sensitive(!self.smoke && matches!(phase, Phase::Starting | Phase::Recording));
            button.set_label(if phase == Phase::Starting {
                "Отменить запуск"
            } else {
                "Остановить и сохранить"
            });
        }
        if let Some(source) = self.source.upgrade() {
            source.set_sensitive(idle);
        }
        if let Some(fps) = self.fps.upgrade() {
            fps.set_sensitive(idle);
        }
        if let Some(cursor) = self.cursor.upgrade() {
            cursor.set_sensitive(idle);
        }
    }
    #[allow(deprecated)]
    fn choose_file(self: &Rc<Self>) {
        if self.smoke || self.session.borrow().phase() != Phase::Idle {
            return;
        }
        if let Some(chooser) = self.chooser.borrow().as_ref() {
            chooser.show();
            return;
        }
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let chooser = gtk::FileChooserNative::new(
            Some("Новый файл записи WebM"),
            Some(&window),
            gtk::FileChooserAction::Save,
            Some("Выбрать"),
            Some("Отмена"),
        );
        chooser.set_current_name("Tidemark-recording.webm");
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Видео WebM"));
        filter.add_pattern("*.webm");
        chooser.add_filter(&filter);
        let weak = Rc::downgrade(self);
        chooser.connect_response(move |chooser, response| {
            let Some(ui) = weak.upgrade() else { chooser.destroy(); return; };
            if ui.chooser.borrow_mut().take().is_none() { return; }
            let path = if response == gtk::ResponseType::Accept { chooser.file().and_then(|file| file.path()) } else { None };
            chooser.destroy();
            if ui.session.borrow().phase() != Phase::Idle { return; }
            if response != gtk::ResponseType::Accept { return; }
            let Some(path) = path else { ui.message("Выберите локальный файл .webm", true); return; };
            if Options::default().validate(&path).is_err() { ui.message("Укажите полный путь к файлу с расширением .webm", true); return; }
            if path.exists() { ui.message("Файл уже существует. Выберите новое имя: запись не заменяет существующие файлы", true); return; }
            if let Some(label) = ui.file_label.upgrade() { label.set_text(&path.to_string_lossy()); }
            ui.path.replace(Some(path)); ui.update_actions(); ui.message("Файл выбран. Нажмите «Начать запись»", false);
        });
        self.chooser.replace(Some(chooser.clone()));
        chooser.show();
    }
    fn start(self: &Rc<Self>) {
        if self.smoke {
            return;
        }
        let Some(path) = self.path.borrow().clone() else {
            return;
        };
        if path.exists() {
            self.message("Выберите новое имя файла перед следующей записью", true);
            return;
        }
        let Some(request) = self.session.borrow_mut().begin() else {
            return;
        };
        let options = Options {
            sources: match self.source.upgrade().map(|d| d.selected()) {
                Some(1) => Sources::Monitor,
                Some(2) => Sources::Window,
                _ => Sources::Any,
            },
            fps: match self.fps.upgrade().map(|d| d.selected()) {
                Some(0) => 24,
                Some(2) => 60,
                _ => 30,
            },
            show_cursor: self.cursor.upgrade().is_some_and(|s| s.is_active()),
            ..Options::default()
        };
        let control = Arc::new(AtomicU8::new(0));
        let progress = Arc::new(Mutex::new(Progress {
            phase: Phase::Starting,
            result: None,
        }));
        let flag = control.clone();
        let shared = progress.clone();
        let started = std::thread::Builder::new()
            .name("recording-ui".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker(options, path, &flag, &shared)
                }))
                .unwrap_or(Err(Error::Worker));
                shared.lock().unwrap_or_else(|e| e.into_inner()).result = Some(result);
            });
        if started.is_err() {
            self.session.borrow_mut().finish(request);
            self.message("Не удалось запустить запись", true);
            self.update_actions();
            return;
        }
        self.control.replace(Some(control));
        self.began.set(None);
        if let Some(elapsed) = self.elapsed.upgrade() {
            elapsed.set_text("00:00");
        }
        self.message(
            "Выберите источник в системном диалоге. Записывается только видео",
            false,
        );
        self.update_actions();
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(ui) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if ui.session.borrow().phase() == Phase::Closed {
                return glib::ControlFlow::Break;
            }
            let snapshot = progress.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(result) = snapshot.result {
                if ui.session.borrow_mut().finish(request) {
                    ui.control.borrow_mut().take();
                    match result {
                        Ok(path) => ui.message(&format!("Запись сохранена:\n{}", path.display()), false),
                        Err(Error::Canceled) => ui.message("Запуск записи отменён", false),
                        Err(Error::Encoder) => ui.message("Не удалось запустить видеокодек GStreamer. Проверьте установку пакета записи", true),
                        Err(Error::Output) => ui.message("Не удалось создать файл. Выберите новое имя WebM", true),
                        Err(error) => ui.message(&format!("Запись не подтверждена: {error}"), true),
                    }
                    ui.update_actions();
                }
                return glib::ControlFlow::Break;
            }
            if snapshot.phase == Phase::Recording && ui.session.borrow_mut().ready(request) {
                ui.began.set(Some(Instant::now()));
                ui.message("Идёт запись видео без звука", false);
                ui.update_actions();
            } else if snapshot.phase == Phase::Stopping
                && ui.session.borrow().phase() == Phase::Recording
            {
                ui.session.borrow_mut().request_stop();
                ui.message("Завершаем и сохраняем WebM…", false);
                ui.update_actions();
            }
            if let (Some(began), Some(label)) = (ui.began.get(), ui.elapsed.upgrade()) {
                let secs = began.elapsed().as_secs();
                label.set_text(&format!("{:02}:{:02}", secs / 60, secs % 60));
            }
            glib::ControlFlow::Continue
        });
    }
    fn stop(&self) {
        if self.smoke {
            return;
        }
        let Some(stop) = self.session.borrow_mut().request_stop() else {
            return;
        };
        self.signal(stop);
        self.message(
            if stop == Stop::Cancel {
                "Отменяем запуск. При необходимости закройте системный диалог выбора источника"
            } else {
                "Завершаем и сохраняем WebM…"
            },
            false,
        );
        self.update_actions();
    }
    fn signal(&self, stop: Stop) {
        if let Some(control) = self.control.borrow().as_ref() {
            control.store(if stop == Stop::Cancel { 2 } else { 1 }, Ordering::Release);
        }
    }
    #[allow(deprecated)]
    fn close(&self) {
        if let Some(stop) = self.session.borrow_mut().close() {
            self.signal(stop);
        }
        let chooser = self.chooser.borrow_mut().take();
        if let Some(chooser) = chooser {
            chooser.destroy();
        }
    }
}
impl Drop for Ui {
    fn drop(&mut self) {
        if let Some(stop) = self.session.get_mut().close() {
            if let Some(control) = self.control.get_mut().take() {
                control.store(if stop == Stop::Cancel { 2 } else { 1 }, Ordering::Release);
            }
        }
    }
}
fn worker(
    options: Options,
    path: PathBuf,
    control: &AtomicU8,
    progress: &Mutex<Progress>,
) -> Result<PathBuf, Error> {
    if control.load(Ordering::Acquire) == 2 {
        return Err(Error::Canceled);
    }
    let recorder = Recorder::start(options, path)?;
    if control.load(Ordering::Acquire) == 2 {
        drop(recorder);
        return Err(Error::Canceled);
    }
    progress.lock().unwrap_or_else(|e| e.into_inner()).phase = Phase::Recording;
    loop {
        match control.load(Ordering::Acquire) {
            2 => {
                drop(recorder);
                return Err(Error::Canceled);
            }
            1 => {
                progress.lock().unwrap_or_else(|e| e.into_inner()).phase = Phase::Stopping;
                return recorder.stop();
            }
            _ => (),
        }
        if let Some(result) = recorder.poll() {
            return result;
        }
        if recorder.status() == Status::Stopping {
            progress.lock().unwrap_or_else(|e| e.into_inner()).phase = Phase::Stopping;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_use_markup(false);
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label
}
