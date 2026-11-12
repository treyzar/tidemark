//! Ordinary capture editor window. Construction is offline; every effect is explicit.
use adw::prelude::*;
use gtk::glib;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{mpsc, Arc},
    time::Duration,
};
use tidemark_capture::{Raster, Rect as ImageRect};
#[path = "../services/capture_state.rs"]
mod state;
use state::{Operation, Rect, Request, Session, Viewport};

pub struct CaptureWindow;
impl CaptureWindow {
    /// Building/presenting this window never captures or changes the clipboard.
    pub fn build(app: &adw::Application, smoke: bool) -> gtk::ApplicationWindow {
        build(app, smoke, None)
    }
    /// Deterministic smoke preview; all effect buttons remain disabled.
    #[allow(dead_code)]
    pub fn preview(app: &adw::Application, image: Raster) -> gtk::ApplicationWindow {
        build(app, true, Some(image))
    }
}
#[derive(Clone, Copy)]
enum Action {
    Capture,
    Save,
    CopyImage,
    Ocr,
    Qr,
    Crop,
    Pixelate,
    Undo,
    CopyText,
    ClearSelection,
}
struct Editor {
    window: glib::WeakRef<gtk::ApplicationWindow>,
    picture: glib::WeakRef<gtk::Picture>,
    drawing: glib::WeakRef<gtk::DrawingArea>,
    placeholder: glib::WeakRef<gtk::Label>,
    status: glib::WeakRef<gtk::Label>,
    result_box: glib::WeakRef<gtk::Box>,
    buffer: gtk::TextBuffer,
    actions: RefCell<Vec<(Action, glib::WeakRef<gtk::Button>)>>,
    session: RefCell<Session>,
    image: RefCell<Option<Arc<Raster>>>,
    undo: RefCell<Option<Arc<Raster>>>,
    texture: RefCell<Option<gtk::gdk::MemoryTexture>>,
    selection: Cell<Option<Rect>>,
    drag_start: Cell<Option<(f64, f64)>>,
    has_text: Cell<bool>,
    restore_after_capture: Cell<bool>,
    chooser: RefCell<Option<gtk::FileChooserNative>>,
}
// GBytes owns an Arc rather than copying the complete RGBA image on the GTK thread.
struct PixelBytes(Arc<Raster>);
impl AsRef<[u8]> for PixelBytes {
    fn as_ref(&self) -> &[u8] {
        self.0.pixels()
    }
}
enum Outcome {
    Image(Raster, bool),
    Saved,
    Text(String, &'static str),
    Canceled,
    SaveCanceled,
}
type Work = Result<Outcome, String>;
fn build(app: &adw::Application, smoke: bool, preview: Option<Raster>) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Tidemark — снимок экрана")
        .default_width(900)
        .default_height(720)
        .build();
    window.set_size_request(560, 480);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new("Снимок экрана", "Tidemark")));
    window.set_titlebar(Some(&header));
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(16);
    body.set_margin_end(16);
    let hint=gtk::Label::new(Some("Создайте снимок через системный диалог. Для выделения протяните прямоугольник по изображению."));
    hint.set_wrap(true);
    hint.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    hint.set_max_width_chars(1);
    hint.set_xalign(0.0);
    hint.add_css_class("dim-label");
    body.append(&hint);
    let actions = gtk::FlowBox::new();
    actions.set_selection_mode(gtk::SelectionMode::None);
    actions.set_min_children_per_line(1);
    actions.set_max_children_per_line(6);
    actions.set_column_spacing(6);
    actions.set_row_spacing(6);
    body.append(&actions);
    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    let picture = gtk::Picture::new();
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_size_request(-1, 200);
    overlay.set_child(Some(&picture));
    let placeholder = gtk::Label::new(Some(if smoke {
        "Режим проверки: снимок не создаётся."
    } else {
        "Снимка пока нет"
    }));
    placeholder.add_css_class("dim-label");
    overlay.add_overlay(&placeholder);
    let drawing = gtk::DrawingArea::new();
    drawing.set_can_target(false);
    drawing.set_hexpand(true);
    drawing.set_vexpand(true);
    drawing.set_halign(gtk::Align::Fill);
    drawing.set_valign(gtk::Align::Fill);
    overlay.add_overlay(&drawing);
    let frame = gtk::Frame::new(None);
    frame.set_child(Some(&overlay));
    frame.set_vexpand(true);
    body.append(&frame);
    let status = gtk::Label::new(Some(if smoke {
        "Действия отключены в режиме проверки."
    } else {
        "Ожидание. Снимок не создаётся автоматически."
    }));
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status.set_max_width_chars(1);
    body.append(&status);
    let buffer = gtk::TextBuffer::new(None);
    let text = gtk::TextView::with_buffer(&buffer);
    text.set_editable(false);
    text.set_cursor_visible(false);
    text.set_wrap_mode(gtk::WrapMode::WordChar);
    let text_scroll = gtk::ScrolledWindow::new();
    text_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    text_scroll.set_min_content_height(110);
    text_scroll.set_max_content_height(160);
    text_scroll.set_child(Some(&text));
    let result_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let result_title = gtk::Label::new(Some("Результат распознавания"));
    result_title.set_xalign(0.0);
    result_title.add_css_class("heading");
    result_box.append(&result_title);
    result_box.append(&text_scroll);
    result_box.set_visible(false);
    body.append(&result_box);
    root.append(&body);
    window.set_child(Some(&root));
    let editor = Rc::new(Editor {
        window: window.downgrade(),
        picture: picture.downgrade(),
        drawing: drawing.downgrade(),
        placeholder: placeholder.downgrade(),
        status: status.downgrade(),
        result_box: result_box.downgrade(),
        buffer,
        actions: RefCell::new(Vec::new()),
        session: RefCell::new(Session::new(smoke)),
        image: RefCell::new(None),
        undo: RefCell::new(None),
        texture: RefCell::new(None),
        selection: Cell::new(None),
        drag_start: Cell::new(None),
        has_text: Cell::new(false),
        restore_after_capture: Cell::new(false),
        chooser: RefCell::new(None),
    });
    for (action, label, tooltip) in [
        (
            Action::Capture,
            "Новый снимок",
            "Системный диалог снимка экрана. Системная копия остаётся на диске даже при закрытии редактора",
        ),
        (
            Action::Save,
            "Сохранить PNG",
            "Сохранить текущее изображение в выбранный файл",
        ),
        (
            Action::CopyImage,
            "Копировать снимок",
            "Копировать текущее изображение; для области сначала обрежьте снимок",
        ),
        (
            Action::Ocr,
            "Распознать текст",
            "Tesseract rus+eng: выделенная область или всё изображение, без сети",
        ),
        (
            Action::Qr,
            "QR-код",
            "Прочитать QR из области или всего снимка; ссылки не открываются",
        ),
        (
            Action::Crop,
            "Обрезать",
            "Обрезать до выделенного прямоугольника",
        ),
        (
            Action::Pixelate,
            "Пикселизация",
            "Пикселизировать выделенную область блоками 16 пикселей",
        ),
        (
            Action::Undo,
            "Отменить правку",
            "Вернуть изображение до последней обрезки или пикселизации",
        ),
        (
            Action::ClearSelection,
            "Снять выделение",
            "Снять прямоугольное выделение",
        ),
        (
            Action::CopyText,
            "Копировать текст",
            "Копировать результат OCR или QR по явному нажатию",
        ),
    ] {
        let button = gtk::Button::with_label(label);
        button.set_tooltip_text(Some(tooltip));
        if matches!(action, Action::Capture) {
            button.add_css_class("suggested-action");
        }
        let state = editor.clone();
        button.connect_clicked(move |_| state.perform(action));
        editor
            .actions
            .borrow_mut()
            .push((action, button.downgrade()));
        actions.insert(&button, -1);
    }
    {
        let editor = editor.clone();
        drawing.set_draw_func(move |_, cr, width, height| {
            let Some(rect) = editor.selection.get() else {
                return;
            };
            let Some(view) = editor.viewport(width, height) else {
                return;
            };
            let (x, y, w, h) = view.outline(rect);
            cr.set_source_rgba(0.3, 0.55, 1.0, 0.9);
            cr.set_line_width(2.0);
            cr.set_dash(&[6.0, 4.0], 0.0);
            cr.rectangle(x, y, w, h);
            let _ = cr.stroke();
        });
    }
    let drag = gtk::GestureDrag::new();
    drag.set_button(1);
    {
        let editor = editor.clone();
        drag.connect_drag_begin(move |_, x, y| {
            if editor.ready() && editor.image.borrow().is_some() {
                editor.drag_start.set(Some((x, y)));
                editor.selection.set(None);
                editor.redraw();
                editor.update_actions();
            }
        });
    }
    {
        let editor = editor.clone();
        drag.connect_drag_update(move |_, dx, dy| editor.drag_to(dx, dy));
    }
    {
        let editor = editor.clone();
        drag.connect_drag_end(move |_, dx, dy| {
            editor.drag_to(dx, dy);
            editor.drag_start.set(None);
        });
    }
    overlay.add_controller(drag);
    {
        let editor = editor.clone();
        window.connect_close_request(move |_| {
            editor.session.borrow_mut().close();
            editor.close_chooser();
            glib::Propagation::Proceed
        });
    }
    if let Some(image) = preview {
        editor.replace_image(Arc::new(image), false);
        editor.message("Синтетический предпросмотр. Действия отключены.", false);
    }
    editor.update_actions();
    window
}
impl Editor {
    fn ready(&self) -> bool {
        let state = self.session.borrow();
        state.interactive() && !state.busy()
    }
    fn viewport(&self, w: i32, h: i32) -> Option<Viewport> {
        let image = self.image.borrow();
        let image = image.as_ref()?;
        Viewport::fit(f64::from(w), f64::from(h), image.width(), image.height())
    }
    fn redraw(&self) {
        if let Some(drawing) = self.drawing.upgrade() {
            drawing.queue_draw();
        }
    }
    fn drag_to(&self, dx: f64, dy: f64) {
        if !self.ready() {
            return;
        }
        let (Some(start), Some(drawing)) = (self.drag_start.get(), self.drawing.upgrade()) else {
            return;
        };
        self.selection.set(
            self.viewport(drawing.width(), drawing.height())
                .and_then(|v| v.selection(start, (start.0 + dx, start.1 + dy))),
        );
        self.redraw();
        self.update_actions();
    }
    fn update_actions(&self) {
        let ready = self.ready();
        let image = self.image.borrow().is_some();
        for (action, weak) in self.actions.borrow().iter() {
            if let Some(button) = weak.upgrade() {
                button.set_sensitive(
                    ready
                        && match action {
                            Action::Capture => true,
                            Action::CopyText => self.has_text.get(),
                            Action::Undo => self.undo.borrow().is_some(),
                            Action::Crop | Action::Pixelate | Action::ClearSelection => {
                                image && self.selection.get().is_some()
                            }
                            _ => image,
                        },
                );
            }
        }
    }
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
    fn replace_image(&self, image: Arc<Raster>, remember: bool) {
        *self.undo.borrow_mut() = if remember {
            self.image.borrow().clone()
        } else {
            None
        };
        self.session.borrow_mut().image_changed();
        self.selection.set(None);
        self.drag_start.set(None);
        self.buffer.set_text("");
        self.has_text.set(false);
        if let Some(result) = self.result_box.upgrade() {
            result.set_visible(false);
        }
        let texture = gtk::gdk::MemoryTexture::new(
            image.width() as i32,
            image.height() as i32,
            gtk::gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(PixelBytes(image.clone())),
            image.width() as usize * 4,
        );
        if let Some(picture) = self.picture.upgrade() {
            picture.set_paintable(Some(&texture));
        }
        if let Some(placeholder) = self.placeholder.upgrade() {
            placeholder.set_visible(false);
        }
        *self.texture.borrow_mut() = Some(texture);
        *self.image.borrow_mut() = Some(image);
        self.redraw();
        self.update_actions();
    }
    fn perform(self: &Rc<Self>, action: Action) {
        if !self.ready() {
            return;
        }
        match action {
            Action::Capture => {
                self.restore_after_capture.set(true);
                if let Some(window) = self.window.upgrade() {
                    window.set_visible(false);
                }
                self.spawn(
                    Operation::Capture,
                    "Ожидаем системный диалог снимка…",
                    || {
                        let result = (|| {
                            let client = tidemark_capture::portal::Client::connect()?;
                            client.screenshot(&Default::default())?.load()
                        })();
                        match result {
                            Ok(image) => Ok(Outcome::Image(image, false)),
                            Err(tidemark_capture::Error::Canceled) => Ok(Outcome::Canceled),
                            Err(error) => Err(format!("Снимок не получен: {error}")),
                        }
                    },
                );
            }
            Action::Save => self.choose_save(),
            Action::CopyImage => {
                if let (Some(window), Some(texture)) =
                    (self.window.upgrade(), self.texture.borrow().as_ref())
                {
                    gtk::prelude::WidgetExt::display(&window)
                        .clipboard()
                        .set_texture(texture);
                    self.message("Изображение скопировано.", false);
                }
            }
            Action::CopyText => {
                if !self.has_text.get() {
                    return;
                }
                if let Some(window) = self.window.upgrade() {
                    let text =
                        self.buffer
                            .text(&self.buffer.start_iter(), &self.buffer.end_iter(), false);
                    gtk::prelude::WidgetExt::display(&window)
                        .clipboard()
                        .set_text(text.as_str());
                    self.message("Результат скопирован.", false);
                }
            }
            Action::ClearSelection => {
                self.selection.set(None);
                self.redraw();
                self.update_actions();
            }
            Action::Undo => {
                let image = self.undo.borrow_mut().take();
                if let Some(image) = image {
                    self.replace_image(image, false);
                    self.message("Последняя правка отменена.", false);
                }
            }
            Action::Crop | Action::Pixelate | Action::Ocr | Action::Qr => {
                let Some(image) = self.image.borrow().clone() else {
                    return;
                };
                let selection = self.selection.get().map(image_rect);
                let op = match action {
                    Action::Crop => Operation::Crop,
                    Action::Pixelate => Operation::Pixelate,
                    Action::Ocr => Operation::Ocr,
                    _ => Operation::Qr,
                };
                self.spawn(
                    op,
                    "Обрабатываем изображение…",
                    move || match action {
                        Action::Crop => image
                            .crop(selection.ok_or("Сначала выделите область.")?)
                            .map(|image| Outcome::Image(image, true))
                            .map_err(|e| e.to_string()),
                        Action::Pixelate => {
                            let mut changed = (*image).clone();
                            changed
                                .pixelate(selection.ok_or("Сначала выделите область.")?, 16)
                                .map_err(|e| e.to_string())?;
                            Ok(Outcome::Image(changed, true))
                        }
                        Action::Ocr | Action::Qr => {
                            let selected = selection
                                .map(|rect| image.crop(rect))
                                .transpose()
                                .map_err(|e| e.to_string())?;
                            let source = selected.as_ref().unwrap_or(&image);
                            if matches!(action, Action::Ocr) {
                                tidemark_capture::ocr::recognize(source, Duration::from_secs(15))
                                    .map(|text| Outcome::Text(text, "OCR"))
                                    .map_err(|e| e.to_string())
                            } else {
                                source
                                    .qr()
                                    .map(|text| Outcome::Text(text.join("\n\n"), "QR"))
                                    .map_err(|e| e.to_string())
                            }
                        }
                        _ => unreachable!(),
                    },
                );
            }
        }
    }
    fn spawn(
        self: &Rc<Self>,
        operation: Operation,
        label: &str,
        work: impl FnOnce() -> Work + Send + 'static,
    ) {
        let request = self.session.borrow_mut().begin(operation);
        let Some(request) = request else {
            return;
        };
        if matches!(operation, Operation::Ocr | Operation::Qr) {
            self.buffer.set_text("");
            self.has_text.set(false);
            if let Some(result) = self.result_box.upgrade() {
                result.set_visible(false);
            }
        }
        self.run(request, label, work);
    }
    fn run(
        self: &Rc<Self>,
        request: Request,
        label: &str,
        work: impl FnOnce() -> Work + Send + 'static,
    ) {
        self.message(label, false);
        self.update_actions();
        let (tx, rx) = mpsc::sync_channel(1);
        if std::thread::Builder::new()
            .name("capture-editor".into())
            .spawn(move || {
                let _ = tx.try_send(work());
            })
            .is_err()
        {
            self.complete(request, Err("Не удалось запустить обработку.".into()));
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(50), move || {
            let Some(editor) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if !editor.session.borrow().interactive() {
                return glib::ControlFlow::Break;
            }
            match rx.try_recv() {
                Ok(result) => {
                    editor.complete(request, result);
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    editor.complete(request, Err("Обработка прервана.".into()));
                    glib::ControlFlow::Break
                }
            }
        });
    }
    fn complete(&self, request: Request, result: Work) {
        let current = self.session.borrow_mut().finish(request);
        if !current {
            return;
        }
        if self.restore_after_capture.replace(false) {
            if let Some(window) = self.window.upgrade() {
                window.present();
            }
        }
        match result {
            Ok(Outcome::Image(image, remember)) => {
                let size = format!("{} × {}", image.width(), image.height());
                self.replace_image(Arc::new(image), remember);
                self.message(
                    &format!("Изображение готово: {size}. Можно выделить область."),
                    false,
                );
            }
            Ok(Outcome::Saved) => self.message("PNG сохранён.", false),
            Ok(Outcome::SaveCanceled) => {
                self.message("Сохранение отменено; файл не изменён.", false)
            }
            Ok(Outcome::Canceled) => {
                self.message("Создание снимка отменено. Изображение не изменено.", false)
            }
            Ok(Outcome::Text(text, source)) => {
                self.has_text.set(!text.trim().is_empty());
                self.buffer.set_text(&text);
                if let Some(box_) = self.result_box.upgrade() {
                    box_.set_visible(true);
                }
                self.message(
                    if text.trim().is_empty() {
                        if source == "QR" {
                            "QR-код не найден."
                        } else {
                            "Текст не распознан."
                        }
                    } else {
                        if source == "QR" {
                            "QR прочитан. Ссылки не открываются; копирование — по кнопке."
                        } else {
                            "Текст распознан локально. Копирование — по кнопке."
                        }
                    },
                    false,
                );
            }
            Err(error) => self.message(&error, true),
        }
        self.update_actions();
    }
    #[allow(deprecated)]
    fn choose_save(self: &Rc<Self>) {
        let (Some(image), Some(window)) = (self.image.borrow().clone(), self.window.upgrade())
        else {
            return;
        };
        let Some(request) = self.session.borrow_mut().begin(Operation::Save) else {
            return;
        };
        self.update_actions();
        let chooser = gtk::FileChooserNative::new(
            Some("Сохранить снимок PNG"),
            Some(&window),
            gtk::FileChooserAction::Save,
            Some("Сохранить"),
            Some("Отмена"),
        );
        chooser.set_modal(true);
        chooser.set_current_name("Снимок.png");
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("PNG"));
        filter.add_pattern("*.png");
        chooser.add_filter(&filter);
        let weak = Rc::downgrade(self);
        chooser.connect_response(move |dialog, response| {
            let Some(editor) = weak.upgrade() else {
                dialog.destroy();
                return;
            };
            editor.chooser.borrow_mut().take();
            if response != gtk::ResponseType::Accept {
                editor.complete(request, Ok(Outcome::SaveCanceled));
                dialog.destroy();
                return;
            }
            let path = dialog.file().and_then(|f| f.path());
            dialog.destroy();
            let Some(path) = path else {
                editor.complete(request, Err("Выберите локальный файл PNG.".into()));
                return;
            };
            if !path
                .extension()
                .is_some_and(|ext| ext.to_str().is_some_and(|e| e.eq_ignore_ascii_case("png")))
            {
                editor.complete(
                    request,
                    Err("Укажите имя с расширением .png; файл не изменён.".into()),
                );
                return;
            }
            let image = image.clone();
            editor.run(request, "Сохраняем PNG…", move || {
                let bytes = image.png().map_err(|e| e.to_string())?;
                tidemark_core::paths::write_atomic(&path, &bytes)
                    .map_err(|_| "Не удалось записать выбранный файл.".to_string())?;
                Ok(Outcome::Saved)
            });
        });
        *self.chooser.borrow_mut() = Some(chooser.clone());
        chooser.show();
    }
    #[allow(deprecated)]
    fn close_chooser(&self) {
        // Release RefCell before destroying: GTK may synchronously emit response.
        let chooser = self.chooser.borrow_mut().take();
        if let Some(chooser) = chooser {
            chooser.destroy();
        }
    }
}
fn image_rect(rect: Rect) -> ImageRect {
    ImageRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}
