//! Full-screen overlay over a frozen screenshot: pick an area (screenshot
//! with annotations, OCR, QR) or a color (eyedropper). Drawn with software
//! Direct2D; the screenshot lives only while the overlay is open. The
//! result goes back to the island with a posted message.

use crate::capture::{Image, Shot};
use crate::textinput::TextField;
use std::cell::RefCell;
use windows::core::{w, Result};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::Vector2;

pub const WM_OVERLAY_DONE: u32 = WM_APP + 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Snip,
    Ocr,
    Qr,
    Picker,
}

pub enum Outcome {
    /// Annotated screenshot of the area.
    Snip(Image),
    /// Plain crop for OCR or QR.
    Area(Mode, Image),
    Color([u8; 3]),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Arrow,
    Rect,
    Text,
    Hide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    Tool(Tool),
    Color(usize),
    Undo,
    Cancel,
    Done,
}

const COLORS: [[f32; 3]; 6] = [[0.94, 0.27, 0.27], [0.98, 0.80, 0.08], [0.20, 0.78, 0.35], [0.23, 0.51, 0.96], [1.0, 1.0, 1.0], [0.05, 0.05, 0.05]];

enum Ann {
    Arrow { a: (f32, f32), b: (f32, f32), color: usize },
    Rect { a: (f32, f32), b: (f32, f32), color: usize },
    Text { at: (f32, f32), text: String, color: usize },
    /// Pixelated cover over (x, y, w, h), pixels averaged from the shot.
    Hide { r: (f32, f32, f32, f32), pix: Image },
}

struct Overlay {
    hwnd: HWND,
    island: HWND,
    mode: Mode,
    shot: Shot,
    factory: ID2D1Factory,
    rt: ID2D1HwndRenderTarget,
    bg: ID2D1Bitmap,
    font: IDWriteTextFormat,
    label: IDWriteTextFormat,
    /// Selection (x0, y0, x1, y1) in client pixels, normalized.
    sel: Option<(f32, f32, f32, f32)>,
    drag_from: Option<(f32, f32)>,
    editing: bool,
    tool: Tool,
    color: usize,
    anns: Vec<Ann>,
    current: Option<Ann>,
    text: Option<((f32, f32), TextField)>,
    cursor: (f32, f32),
    scale: f32,
    /// Result sent; the window is hidden and closes on the next message.
    closing: bool,
}

thread_local! {
    static OVERLAY: RefCell<Option<Overlay>> = const { RefCell::new(None) };
    static RESULT: RefCell<Option<Outcome>> = const { RefCell::new(None) };
}

/// The result of the overlay that just closed (after WM_OVERLAY_DONE).
pub fn take_result() -> Option<Outcome> {
    RESULT.with(|r| r.borrow_mut().take())
}

pub fn is_open() -> bool {
    OVERLAY.with(|o| o.borrow().is_some())
}

fn with<R>(f: impl FnOnce(&mut Overlay) -> R) -> Option<R> {
    OVERLAY.with(|o| o.try_borrow_mut().ok().and_then(|mut o| o.as_mut().map(f)))
}

fn norm(a: (f32, f32), b: (f32, f32)) -> (f32, f32, f32, f32) {
    (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: x0, top: y0, right: x1, bottom: y1 }
}

fn col(c: [f32; 3], a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c[0], g: c[1], b: c[2], a }
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

/// Opens the overlay over `shot`. Returns false if it could not be created.
pub fn open(island: HWND, mode: Mode, shot: Shot, scale: f32) -> bool {
    if is_open() {
        return false;
    }
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else { return false };
        let instance: HINSTANCE = module.into();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
            lpszClassName: w!("WinIslandOverlay"),
            ..Default::default()
        };
        RegisterClassExW(&class); // Fails harmlessly when already registered.
        let (w, h) = (shot.image.w as i32, shot.image.h as i32);
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("WinIslandOverlay"),
            w!("Win Island"),
            WS_POPUP,
            shot.x,
            shot.y,
            w,
            h,
            None,
            None,
            Some(instance),
            None,
        ) else {
            return false;
        };
        let Some(overlay) = Overlay::new(hwnd, island, mode, shot, scale).ok() else {
            let _ = DestroyWindow(hwnd);
            return false;
        };
        OVERLAY.with(|o| *o.borrow_mut() = Some(overlay));
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        with(|o| o.render());
        true
    }
}

impl Overlay {
    fn new(hwnd: HWND, island: HWND, mode: Mode, shot: Shot, scale: f32) -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U { width: shot.image.w, height: shot.image.h },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let rt = factory.CreateHwndRenderTarget(&props, &hprops)?;
            let bg = bitmap_from(&rt, &shot.image)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let make = |size: f32, weight| dwrite.CreateTextFormat(w!("Segoe UI Variable Text"), None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("ru-ru"));
            let font = make(22.0 * scale, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let label = make(12.0 * scale, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            label.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            label.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            Ok(Self {
                hwnd,
                island,
                mode,
                shot,
                factory,
                rt,
                bg,
                font,
                label,
                sel: None,
                drag_from: None,
                editing: false,
                tool: Tool::Arrow,
                color: 0,
                anns: Vec::new(),
                current: None,
                text: None,
                cursor: (0.0, 0.0),
                scale,
                closing: false,
            })
        }
    }

    fn finish(&mut self, outcome: Outcome) {
        if self.closing {
            return;
        }
        self.closing = true;
        RESULT.with(|r| *r.borrow_mut() = Some(outcome));
        unsafe {
            // Hide now; destroy later, outside this borrow, so WM_DESTROY can
            // drop the screenshot.
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            let _ = PostMessageW(Some(self.island), WM_OVERLAY_DONE, WPARAM(0), LPARAM(0));
        }
    }

    fn sel_size(&self) -> (i32, i32) {
        self.sel.map_or((0, 0), |(x0, y0, x1, y1)| ((x1 - x0).round() as i32, (y1 - y0).round() as i32))
    }

    fn crop(&self) -> Image {
        let (x0, y0, _, _) = self.sel.unwrap_or_default();
        let (w, h) = self.sel_size();
        self.shot.image.crop(x0.round() as i32, y0.round() as i32, w, h)
    }

    /// Toolbar buttons under (or above) the selection.
    fn toolbar(&self) -> Vec<(Button, D2D_RECT_F)> {
        let Some((x0, y0, x1, y1)) = self.sel else { return Vec::new() };
        let k = self.scale;
        let b = 30.0 * k;
        let gap = 4.0 * k;
        let mut items: Vec<(Button, f32)> = Vec::new();
        for t in [Tool::Arrow, Tool::Rect, Tool::Text, Tool::Hide] {
            items.push((Button::Tool(t), b));
        }
        for i in 0..COLORS.len() {
            items.push((Button::Color(i), 22.0 * k));
        }
        items.push((Button::Undo, b));
        items.push((Button::Cancel, b));
        items.push((Button::Done, 92.0 * k));
        let total: f32 = items.iter().map(|(_, w)| w + gap).sum::<f32>() + 8.0 * k;
        let (_, sh) = (self.shot.image.w as f32, self.shot.image.h as f32);
        let bar_h = b + 10.0 * k;
        let top = if y1 + bar_h + 8.0 * k < sh { y1 + 8.0 * k } else if y0 - bar_h - 8.0 * k > 0.0 { y0 - bar_h - 8.0 * k } else { y1 - bar_h - 8.0 * k };
        let mut x = (x1 - total).max(x0.min(self.shot.image.w as f32 - total)).max(0.0) + 4.0 * k;
        items
            .into_iter()
            .map(|(btn, w)| {
                let h = if matches!(btn, Button::Color(_)) { 22.0 * k } else { b };
                let r = rect(x, top + (bar_h - h) / 2.0, x + w, top + (bar_h + h) / 2.0);
                x += w + gap;
                (btn, r)
            })
            .collect()
    }

    fn button_at(&self, x: f32, y: f32) -> Option<Button> {
        self.toolbar().into_iter().find(|(_, r)| x >= r.left && x <= r.right && y >= r.top && y <= r.bottom).map(|(b, _)| b)
    }

    fn inside_sel(&self, x: f32, y: f32) -> bool {
        self.sel.is_some_and(|(x0, y0, x1, y1)| x >= x0 && x <= x1 && y >= y0 && y <= y1)
    }

    fn commit_text(&mut self) {
        if let Some((at, field)) = self.text.take() {
            if !field.text.trim().is_empty() {
                self.anns.push(Ann::Text { at, text: field.text, color: self.color });
            }
        }
    }

    fn done(&mut self) {
        self.commit_text();
        if self.sel.is_none() || self.sel_size().0 < 2 || self.sel_size().1 < 2 {
            return;
        }
        let img = self.render_final().unwrap_or_else(|_| self.crop());
        self.finish(Outcome::Snip(img));
    }

    fn mouse_down(&mut self, x: f32, y: f32) {
        if self.mode == Mode::Picker {
            let p = self.shot.image.pixel(x as u32, y as u32);
            self.finish(Outcome::Color([p[2], p[1], p[0]]));
            return;
        }
        if self.editing {
            if let Some(b) = self.button_at(x, y) {
                match b {
                    Button::Tool(t) => {
                        self.commit_text();
                        self.tool = t;
                    }
                    Button::Color(i) => {
                        self.color = i;
                        if let Some((_, _)) = &self.text {
                            // Recolor the text being typed.
                        }
                    }
                    Button::Undo => {
                        if self.text.take().is_none() {
                            self.anns.pop();
                        }
                    }
                    Button::Cancel => return self.finish(Outcome::Cancelled),
                    Button::Done => return self.done(),
                }
                return;
            }
            if self.inside_sel(x, y) {
                self.commit_text();
                match self.tool {
                    Tool::Text => self.text = Some(((x, y), TextField::new(false))),
                    Tool::Arrow => self.current = Some(Ann::Arrow { a: (x, y), b: (x, y), color: self.color }),
                    Tool::Rect => self.current = Some(Ann::Rect { a: (x, y), b: (x, y), color: self.color }),
                    Tool::Hide => self.current = Some(Ann::Hide { r: (x, y, 0.0, 0.0), pix: Image { w: 0, h: 0, pixels: Vec::new() } }),
                }
                return;
            }
            if !self.anns.is_empty() || self.text.is_some() {
                return; // Keep the work; a new area needs Esc first.
            }
            self.editing = false;
        }
        self.drag_from = Some((x, y));
        self.sel = None;
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        if let Some(from) = self.drag_from {
            self.sel = Some(norm(from, (x, y)));
        }
        let (sx0, sy0, sx1, sy1) = self.sel.unwrap_or_default();
        let clamp = |x: f32, y: f32| (x.clamp(sx0, sx1), y.clamp(sy0, sy1));
        match &mut self.current {
            Some(Ann::Arrow { b, .. }) | Some(Ann::Rect { b, .. }) => *b = clamp(x, y),
            Some(Ann::Hide { r, .. }) => {
                let (cx, cy) = clamp(x, y);
                r.2 = cx - r.0;
                r.3 = cy - r.1;
            }
            _ => {}
        }
    }

    fn mouse_up(&mut self) {
        if self.drag_from.take().is_some() {
            let (w, h) = self.sel_size();
            if w < 4 || h < 4 {
                self.sel = None;
                return;
            }
            match self.mode {
                Mode::Snip => self.editing = true,
                Mode::Ocr | Mode::Qr => {
                    let img = self.crop();
                    let mode = self.mode;
                    self.finish(Outcome::Area(mode, img));
                }
                Mode::Picker => {}
            }
            return;
        }
        match self.current.take() {
            Some(Ann::Hide { r, .. }) => {
                let (x0, y0, x1, y1) = norm((r.0, r.1), (r.0 + r.2, r.1 + r.3));
                if x1 - x0 >= 3.0 && y1 - y0 >= 3.0 {
                    let crop = self.shot.image.crop(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32);
                    let block = (10.0 * self.scale).round() as u32;
                    self.anns.push(Ann::Hide { r: (x0, y0, x1 - x0, y1 - y0), pix: crop.pixelate(block) });
                }
            }
            Some(a @ Ann::Arrow { a: p, b: q, .. }) | Some(a @ Ann::Rect { a: p, b: q, .. }) => {
                if (p.0 - q.0).abs() + (p.1 - q.1).abs() > 4.0 {
                    self.anns.push(a);
                }
            }
            _ => {}
        }
    }

    /// Esc: drop the text being typed, else leave.
    fn escape(&mut self) {
        if self.text.take().is_some() {
            return;
        }
        self.finish(Outcome::Cancelled);
    }

    fn key(&mut self, vk: u16) -> bool {
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        match VIRTUAL_KEY(vk) {
            VK_ESCAPE => self.escape(),
            VK_RETURN => {
                if self.text.is_some() {
                    self.commit_text();
                } else if self.editing {
                    self.done();
                }
            }
            VK_Z if ctrl => {
                if self.text.take().is_none() {
                    self.anns.pop();
                }
            }
            VK_BACK => {
                if let Some((_, f)) = &mut self.text {
                    f.backspace(ctrl);
                }
            }
            VK_C if ctrl && self.editing => self.done(),
            _ => return false,
        }
        true
    }

    fn char_input(&mut self, c: char) {
        if let Some((_, f)) = &mut self.text {
            if !c.is_control() {
                f.insert(&c.to_string());
            }
        }
    }

    fn render(&self) {
        if self.closing {
            return;
        }
        unsafe {
            let rt: &ID2D1RenderTarget = &self.rt;
            rt.BeginDraw();
            let full = rect(0.0, 0.0, self.shot.image.w as f32, self.shot.image.h as f32);
            rt.DrawBitmap(&self.bg, Some(&full), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None);
            let Ok(brush) = rt.CreateSolidColorBrush(&col([0.0; 3], 0.45), None) else {
                let _ = rt.EndDraw(None, None);
                return;
            };
            let k = self.scale;
            if self.mode == Mode::Picker {
                self.paint_picker(rt, &brush);
            } else {
                // Dim everything outside the selection.
                match self.sel {
                    Some((x0, y0, x1, y1)) => {
                        for r in [rect(0.0, 0.0, full.right, y0), rect(0.0, y1, full.right, full.bottom), rect(0.0, y0, x0, y1), rect(x1, y0, full.right, y1)] {
                            rt.FillRectangle(&r, &brush);
                        }
                        brush.SetColor(&col([1.0; 3], 0.9));
                        rt.DrawRectangle(&rect(x0, y0, x1, y1), &brush, 1.0 * k, None);
                        // Size label.
                        let (w, h) = self.sel_size();
                        let label = format!("{w} × {h}");
                        let lw = (label.chars().count() as f32 * 7.5 + 16.0) * k;
                        let ly = if y0 > 26.0 * k { y0 - 26.0 * k } else { y0 + 4.0 * k };
                        let lr = rect(x0, ly, x0 + lw, ly + 22.0 * k);
                        brush.SetColor(&col([0.08; 3], 0.85));
                        rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: lr, radiusX: 6.0 * k, radiusY: 6.0 * k }, &brush);
                        brush.SetColor(&col([1.0; 3], 1.0));
                        let t: Vec<u16> = label.encode_utf16().collect();
                        rt.DrawText(&t, &self.label, &lr, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);

                        rt.PushAxisAlignedClip(&rect(x0, y0, x1, y1), D2D1_ANTIALIAS_MODE_ALIASED);
                        for a in self.anns.iter().chain(self.current.iter()) {
                            draw_ann(rt, &self.factory, &self.font, a, (0.0, 0.0), k);
                        }
                        if let Some((at, field)) = &self.text {
                            let text = if field.is_empty() { "Текст…".to_string() } else { field.text.clone() };
                            draw_text(rt, &self.font, &text, *at, self.color, if field.is_empty() { 0.5 } else { 1.0 });
                            // Caret after the text.
                            let wide: Vec<u16> = field.text.encode_utf16().collect();
                            if let Ok(layout) = DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED)
                                .and_then(|f| f.CreateTextLayout(&wide, &self.font, 4000.0, 200.0))
                            {
                                let mut m = DWRITE_TEXT_METRICS::default();
                                let _ = layout.GetMetrics(&mut m);
                                let cx = at.0 + if field.is_empty() { 0.0 } else { m.widthIncludingTrailingWhitespace };
                                brush.SetColor(&col(COLORS[self.color], 1.0));
                                rt.FillRectangle(&rect(cx, at.1, cx + 2.0 * k, at.1 + 28.0 * k), &brush);
                            }
                        }
                        rt.PopAxisAlignedClip();
                        if self.editing {
                            self.paint_toolbar(rt, &brush);
                        }
                    }
                    None => {
                        rt.FillRectangle(&full, &brush);
                        let hint = match self.mode {
                            Mode::Ocr => "Выделите текст на экране · Esc — отмена",
                            Mode::Qr => "Выделите QR-код · Esc — отмена",
                            _ => "Выделите область · Esc — отмена",
                        };
                        let (cx, cy) = self.cursor;
                        let hr = rect(cx + 16.0 * k, cy + 18.0 * k, cx + 16.0 * k + 280.0 * k, cy + 42.0 * k);
                        brush.SetColor(&col([0.08; 3], 0.85));
                        rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: hr, radiusX: 6.0 * k, radiusY: 6.0 * k }, &brush);
                        brush.SetColor(&col([1.0; 3], 1.0));
                        let t: Vec<u16> = hint.encode_utf16().collect();
                        rt.DrawText(&t, &self.label, &hr, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                    }
                }
            }
            let _ = rt.EndDraw(None, None);
        }
    }

    fn paint_toolbar(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush) {
        let k = self.scale;
        let items = self.toolbar();
        let (Some(first), Some(last)) = (items.first(), items.last()) else { return };
        unsafe {
            let bar = rect(first.1.left - 6.0 * k, first.1.top.min(last.1.top) - 5.0 * k, last.1.right + 6.0 * k, first.1.bottom.max(last.1.bottom) + 5.0 * k);
            brush.SetColor(&col([0.1; 3], 0.95));
            rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: bar, radiusX: 12.0 * k, radiusY: 12.0 * k }, brush);
            for (b, r) in &items {
                let (cx, cy) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
                let active = matches!(b, Button::Tool(t) if *t == self.tool);
                if active || matches!(b, Button::Done) {
                    brush.SetColor(&if matches!(b, Button::Done) { col([0.95; 3], 1.0) } else { col([0.3; 3], 1.0) });
                    rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: *r, radiusX: 8.0 * k, radiusY: 8.0 * k }, brush);
                }
                let ink = col([0.95; 3], 1.0);
                brush.SetColor(&ink);
                match b {
                    Button::Tool(Tool::Arrow) => {
                        rt.DrawLine(pt(cx - 6.0 * k, cy + 6.0 * k), pt(cx + 6.0 * k, cy - 6.0 * k), brush, 1.8 * k, None);
                        rt.DrawLine(pt(cx + 6.0 * k, cy - 6.0 * k), pt(cx, cy - 6.0 * k), brush, 1.8 * k, None);
                        rt.DrawLine(pt(cx + 6.0 * k, cy - 6.0 * k), pt(cx + 6.0 * k, cy), brush, 1.8 * k, None);
                    }
                    Button::Tool(Tool::Rect) => rt.DrawRectangle(&rect(cx - 7.0 * k, cy - 5.0 * k, cx + 7.0 * k, cy + 5.0 * k), brush, 1.8 * k, None),
                    Button::Tool(Tool::Text) => {
                        rt.DrawLine(pt(cx - 6.0 * k, cy - 6.0 * k), pt(cx + 6.0 * k, cy - 6.0 * k), brush, 2.0 * k, None);
                        rt.DrawLine(pt(cx, cy - 6.0 * k), pt(cx, cy + 7.0 * k), brush, 2.0 * k, None);
                    }
                    Button::Tool(Tool::Hide) => {
                        for i in 0..3 {
                            for j in 0..3 {
                                if (i + j) % 2 == 0 {
                                    let x = cx - 7.0 * k + i as f32 * 5.0 * k;
                                    let y = cy - 7.0 * k + j as f32 * 5.0 * k;
                                    rt.FillRectangle(&rect(x, y, x + 4.0 * k, y + 4.0 * k), brush);
                                }
                            }
                        }
                    }
                    Button::Color(i) => {
                        brush.SetColor(&col(COLORS[*i], 1.0));
                        let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: 8.0 * k, radiusY: 8.0 * k };
                        rt.FillEllipse(&e, brush);
                        if *i == self.color {
                            brush.SetColor(&ink);
                            let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: 10.5 * k, radiusY: 10.5 * k };
                            rt.DrawEllipse(&e, brush, 1.5 * k, None);
                        }
                    }
                    Button::Undo => {
                        rt.DrawLine(pt(cx - 6.0 * k, cy), pt(cx + 5.0 * k, cy), brush, 1.8 * k, None);
                        rt.DrawLine(pt(cx - 6.0 * k, cy), pt(cx - 2.0 * k, cy - 4.0 * k), brush, 1.8 * k, None);
                        rt.DrawLine(pt(cx - 6.0 * k, cy), pt(cx - 2.0 * k, cy + 4.0 * k), brush, 1.8 * k, None);
                    }
                    Button::Cancel => {
                        rt.DrawLine(pt(cx - 5.0 * k, cy - 5.0 * k), pt(cx + 5.0 * k, cy + 5.0 * k), brush, 1.8 * k, None);
                        rt.DrawLine(pt(cx + 5.0 * k, cy - 5.0 * k), pt(cx - 5.0 * k, cy + 5.0 * k), brush, 1.8 * k, None);
                    }
                    Button::Done => {
                        brush.SetColor(&col([0.05; 3], 1.0));
                        let t: Vec<u16> = "✓ Готово".encode_utf16().collect();
                        rt.DrawText(&t, &self.label, r, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                    }
                }
            }
        }
    }

    fn paint_picker(&self, rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush) {
        let k = self.scale;
        let (cx, cy) = self.cursor;
        let (iw, ih) = (self.shot.image.w as f32, self.shot.image.h as f32);
        let n = 11.0;
        let zoom = 10.0 * k;
        let size = n * zoom;
        // Loupe next to the cursor, flipped near screen edges.
        let lx = if cx + 24.0 * k + size < iw { cx + 24.0 * k } else { cx - 24.0 * k - size };
        let ly = if cy + 24.0 * k + size + 30.0 * k < ih { cy + 24.0 * k } else { cy - 24.0 * k - size - 30.0 * k };
        let (px, py) = (cx.floor(), cy.floor());
        let src = rect(px - 5.0, py - 5.0, px + 6.0, py + 6.0);
        let dst = rect(lx, ly, lx + size, ly + size);
        unsafe {
            brush.SetColor(&col([0.1; 3], 1.0));
            rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(lx - 4.0 * k, ly - 4.0 * k, lx + size + 4.0 * k, ly + size + 30.0 * k), radiusX: 10.0 * k, radiusY: 10.0 * k }, brush);
            rt.DrawBitmap(&self.bg, Some(&dst), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, Some(&src));
            // Center cell.
            brush.SetColor(&col([1.0; 3], 1.0));
            let c0 = lx + 5.0 * zoom;
            let r0 = ly + 5.0 * zoom;
            rt.DrawRectangle(&rect(c0, r0, c0 + zoom, r0 + zoom), brush, 1.5 * k, None);
            let p = self.shot.image.pixel(px.clamp(0.0, iw - 1.0) as u32, py.clamp(0.0, ih - 1.0) as u32);
            let hex = format!("#{:02X}{:02X}{:02X}", p[2], p[1], p[0]);
            let t: Vec<u16> = hex.encode_utf16().collect();
            rt.DrawText(&t, &self.label, &rect(lx, ly + size, lx + size, ly + size + 26.0 * k), brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    /// The selection with all annotations, rendered off-screen.
    fn render_final(&self) -> Result<Image> {
        let crop = self.crop();
        let (x0, y0, _, _) = self.sel.unwrap_or_default();
        unsafe {
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let target = wic.CreateBitmap(crop.w, crop.h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let rt = self.factory.CreateWicBitmapRenderTarget(&target, &props)?;
            let bmp = bitmap_from(&rt, &crop)?;
            rt.BeginDraw();
            rt.DrawBitmap(&bmp, Some(&rect(0.0, 0.0, crop.w as f32, crop.h as f32)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None);
            for a in &self.anns {
                draw_ann(&rt, &self.factory, &self.font, a, (-x0.round(), -y0.round()), self.scale);
            }
            rt.EndDraw(None, None)?;
            let mut pixels = vec![0u8; (crop.w * crop.h * 4) as usize];
            target.CopyPixels(std::ptr::null(), crop.w * 4, &mut pixels)?;
            for p in pixels.chunks_exact_mut(4) {
                p[3] = 255;
            }
            Ok(Image { w: crop.w, h: crop.h, pixels })
        }
    }
}

fn bitmap_from(rt: &ID2D1RenderTarget, img: &Image) -> Result<ID2D1Bitmap> {
    unsafe {
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        rt.CreateBitmap(D2D_SIZE_U { width: img.w.max(1), height: img.h.max(1) }, Some(img.pixels.as_ptr() as *const _), img.w * 4, &props)
    }
}

fn draw_text(rt: &ID2D1RenderTarget, font: &IDWriteTextFormat, text: &str, at: (f32, f32), color: usize, alpha: f32) {
    unsafe {
        let Ok(brush) = rt.CreateSolidColorBrush(&col(COLORS[color], alpha), None) else { return };
        let t: Vec<u16> = text.encode_utf16().collect();
        let r = rect(at.0, at.1, at.0 + 4000.0, at.1 + 200.0);
        // A thin dark outline keeps light text readable on light shots.
        if let Ok(shadow) = rt.CreateSolidColorBrush(&col([0.0; 3], 0.55 * alpha), None) {
            for (dx, dy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
                let s = rect(r.left + dx, r.top + dy, r.right + dx, r.bottom + dy);
                rt.DrawText(&t, font, &s, &shadow, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            }
        }
        rt.DrawText(&t, font, &r, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
    }
}

/// Draws one annotation, shifted by `off` (for the final crop).
fn draw_ann(rt: &ID2D1RenderTarget, factory: &ID2D1Factory, font: &IDWriteTextFormat, a: &Ann, off: (f32, f32), k: f32) {
    let sh = |p: (f32, f32)| (p.0 + off.0, p.1 + off.1);
    unsafe {
        match a {
            Ann::Arrow { a, b, color } => {
                let (a, b) = (sh(*a), sh(*b));
                let Ok(brush) = rt.CreateSolidColorBrush(&col(COLORS[*color], 1.0), None) else { return };
                let w = 4.0 * k;
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let len = (dx * dx + dy * dy).sqrt().max(1.0);
                let (ux, uy) = (dx / len, dy / len);
                let head = (16.0 * k).min(len * 0.6);
                let base = (b.0 - ux * head, b.1 - uy * head);
                rt.DrawLine(pt(a.0, a.1), pt(base.0, base.1), &brush, w, None);
                let (nx, ny) = (-uy * head * 0.55, ux * head * 0.55);
                if let Ok(geo) = factory.CreatePathGeometry() {
                    if let Ok(sink) = geo.Open() {
                        sink.BeginFigure(pt(b.0, b.1), D2D1_FIGURE_BEGIN_FILLED);
                        sink.AddLine(pt(base.0 + nx, base.1 + ny));
                        sink.AddLine(pt(base.0 - nx, base.1 - ny));
                        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                        if sink.Close().is_ok() {
                            rt.FillGeometry(&geo, &brush, None);
                        }
                    }
                }
            }
            Ann::Rect { a, b, color } => {
                let (a, b) = (sh(*a), sh(*b));
                let Ok(brush) = rt.CreateSolidColorBrush(&col(COLORS[*color], 1.0), None) else { return };
                let (x0, y0, x1, y1) = norm(a, b);
                rt.DrawRectangle(&rect(x0, y0, x1, y1), &brush, 3.0 * k, None);
            }
            Ann::Text { at, text, color } => draw_text(rt, font, text, sh(*at), *color, 1.0),
            Ann::Hide { r, pix } => {
                let (x, y) = sh((r.0, r.1));
                if pix.w == 0 {
                    // Being dragged: show the area.
                    if let Ok(brush) = rt.CreateSolidColorBrush(&col([0.5; 3], 0.6), None) {
                        let (x0, y0, x1, y1) = norm((x, y), (x + r.2, y + r.3));
                        rt.FillRectangle(&rect(x0, y0, x1, y1), &brush);
                    }
                    return;
                }
                if let Ok(bmp) = bitmap_from(rt, pix) {
                    rt.DrawBitmap(&bmp, Some(&rect(x, y, x + r.2, y + r.3)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None);
                }
            }
        }
    }
}

fn client_pt(lparam: LPARAM) -> (f32, f32) {
    ((lparam.0 & 0xFFFF) as i16 as f32, ((lparam.0 >> 16) & 0xFFFF) as i16 as f32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let redraw = |f: &mut dyn FnMut(&mut Overlay)| {
        with(|o| {
            f(o);
            o.render();
        });
    };
    match msg {
        WM_LBUTTONDOWN => {
            let (x, y) = client_pt(lparam);
            let _ = SetCapture(hwnd);
            redraw(&mut |o| o.mouse_down(x, y));
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let (x, y) = client_pt(lparam);
            redraw(&mut |o| o.mouse_move(x, y));
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            redraw(&mut |o| o.mouse_up());
            return LRESULT(0);
        }
        WM_RBUTTONUP => {
            redraw(&mut |o| o.escape());
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            let vk = wparam.0 as u16;
            redraw(&mut |o| {
                o.key(vk);
            });
            return LRESULT(0);
        }
        WM_CHAR => {
            if let Some(c) = char::from_u32(wparam.0 as u32) {
                redraw(&mut |o| o.char_input(c));
            }
            return LRESULT(0);
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            with(|o| o.render());
            let _ = EndPaint(hwnd, &ps);
            return LRESULT(0);
        }
        WM_ACTIVATE if wparam.0 & 0xFFFF == 0 => {
            // Switched away (Alt+Tab, Win key): give up rather than linger.
            with(|o| o.finish(Outcome::Cancelled));
            return LRESULT(0);
        }
        WM_DESTROY => {
            // Drop the screenshot and Direct2D resources right away.
            OVERLAY.with(|o| {
                if let Ok(mut o) = o.try_borrow_mut() {
                    *o = None;
                }
            });
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

