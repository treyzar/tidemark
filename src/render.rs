//! Renderer: D3D11 device -> DXGI composition swap chain -> DirectComposition
//! visual on the island window, drawn with Direct2D/DirectWrite. Nothing is
//! drawn unless the island changes, so an idle island costs no CPU time.

use crate::clip_history::Glyph;
use crate::layout::{self, Hit, Placed, Rect, Tab, TileKind};
use crate::textinput::TextField;
use std::collections::HashMap;
use std::sync::Arc;
use crate::media::{Cover, COVER_PX};
use crate::theme::{Rgba, Theme};
use crate::weather::Icon;
use std::f32::consts::{PI, TAU};
use windows::core::{w, Interface, Result};
use windows_numerics::{Matrix3x2, Vector2};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

/// Island outline, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
}

/// One frame's worth of state.
pub struct Scene<'a> {
    pub shape: Shape,
    /// Signed amplitude of the liquid bottom edge, in pixels.
    pub wobble: f32,
    /// Wave phase, in radians.
    pub phase: f32,
    pub theme: &'a Theme,
    /// 0..1, fades the Home content in while the island grows.
    pub content_alpha: f32,
    /// Top-left of the Home panel, in pixels; tiles are laid out from here.
    pub origin: (f32, f32),
    pub scale: f32,
    pub tiles: &'a [Placed],
    pub hovered: Option<usize>,
    /// Home panel size in DIPs (expanded width x tile-derived height).
    pub panel_size: (f32, f32),
    /// What the collapsed island shows.
    pub compact: &'a Compact,
    /// 0..1, the compact content fades out as the island expands.
    pub compact_alpha: f32,
    /// Collapsed island size in DIPs.
    pub compact_size: (f32, f32),
    /// The player tile's contents, if anything is loaded.
    pub media: Option<&'a MediaView>,
    /// Cover art pixels for `media` / `compact.media`.
    pub cover: Option<&'a Cover>,
    /// Only the liquid edge is moving: ~30 fps is enough and halves the cost.
    pub relaxed: bool,
    /// Newest clipboard history rows.
    pub clip_rows: &'a [ClipRow],
    pub dock: &'a [DockIcon],
    /// Clickable thing under the cursor, highlighted.
    pub hot: Option<Hit>,
    /// Blend the island's fill toward this color by the given amount.
    pub tint: Option<(Rgba, f32)>,
    /// Icons needed besides the dock's (active app, toasts).
    pub extra_icons: &'a [DockIcon],
    /// (camera, microphone) in use by some app: shown as small dots.
    pub privacy: (bool, bool),
    /// The active window tile.
    pub window: Option<&'a WindowView>,
    /// Which view of the open island is shown.
    pub tab: Tab,
    pub notes: Option<&'a NotesView>,
    /// Rows of the Clipboard tab.
    pub clip_page: &'a [ClipRow],
    pub search: Option<&'a SearchView>,
    /// Files on the shelf (glyph, name, folder).
    pub shelf: &'a [ClipRow],
    pub text: Option<&'a TextView>,
    pub control: Option<&'a ControlView>,
    /// A screen recording runs (the Tools button turns into Стоп).
    pub recording: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CtlIcon {
    /// Wi-Fi signal bars (1..4) and whether the network is secured.
    Wifi(u32, bool),
    Headphones,
    Input,
    Phone,
    Device,
    Vpn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CtlRow {
    pub icon: CtlIcon,
    pub title: String,
    pub sub: String,
    pub action: Option<String>,
    pub active: bool,
}

/// The Control tab; part of the panel cache key.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlView {
    pub segment: usize,
    pub rows: Vec<CtlRow>,
    pub empty: String,
    pub message: String,
    pub busy: bool,
    /// Row asking for a Wi-Fi password: (row, masked field, focused).
    pub password: Option<(usize, TextField, bool)>,
    /// Power button waiting for a second click.
    pub confirm: Option<usize>,
}

/// The Text tab; part of the panel cache key.
#[derive(Debug, Clone, PartialEq)]
pub struct TextView {
    pub field: TextField,
    pub focus: bool,
    pub stats: String,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchRow {
    pub badge: Badge,
    pub title: String,
    pub subtitle: String,
}

/// The Alt+Space search; part of the panel cache key.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchView {
    pub field: TextField,
    pub rows: Vec<SearchRow>,
    pub selected: usize,
    /// Shown when there are no rows.
    pub hint: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimerUi {
    Idle,
    Running,
    Paused,
    Done,
}

/// Everything the Notes tab shows; part of the panel cache key.
#[derive(Debug, Clone, PartialEq)]
pub struct NotesView {
    pub note: TextField,
    pub note_focus: bool,
    pub remind: TextField,
    pub remind_focus: bool,
    /// Under the reminder field: an example, a preview or "не понял".
    pub remind_hint: String,
    /// (when, text) of the next reminders.
    pub reminders: Vec<(String, String)>,
    pub timer_text: String,
    pub timer: TimerUi,
    /// Index of the preset matching the timer's length, if any.
    pub timer_preset: Option<usize>,
}

/// What the "active window" tile shows; part of the Home cache key.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowView {
    pub icon_id: Option<u64>,
    pub name: String,
    pub title: String,
    pub topmost: bool,
    /// 100 = opaque.
    pub opacity: u32,
}

/// The round badge at the left of a notice.
#[derive(Debug, Clone, PartialEq)]
pub enum Badge {
    Glyph(Glyph),
    /// An app icon from the icon cache.
    App(u64),
    /// Up to 2-3 characters: "RU", "A".
    Text(String),
    Volume,
    Muted,
    Brightness,
    Camera,
    Mic,
    Timer,
    Bell,
    /// Screen recording: a red dot.
    Record,
    /// A color swatch (eyedropper).
    Color([u8; 3]),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClipRow {
    pub glyph: Glyph,
    pub text: String,
    pub ago: String,
}

#[derive(Debug, Clone)]
pub struct DockIcon {
    pub id: u64,
    /// Premultiplied BGRA, dock::ICON_PX square.
    pub icon: Option<Arc<Vec<u8>>>,
}

/// A short notice that takes over the collapsed pill ("Скопировано").
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub badge: Badge,
    pub title: String,
    pub text: String,
    /// 0..1: draws a level bar instead of the second line of text.
    pub level: Option<f32>,
    /// Buttons at the right ("Ещё раз", "✓"); the last is a round check.
    pub actions: Vec<String>,
}

/// What the player shows; part of the Home layer's cache key.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaView {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub position: String,
    pub duration: String,
    /// 0..1000 so it compares and caches exactly.
    pub progress_permille: u32,
    pub cover_id: Option<u64>,
    /// 0..100, None when the player's audio session was not found.
    pub volume: Option<u32>,
    pub can_prev: bool,
    pub can_next: bool,
    /// Current lyric line, shown under the progress bar.
    pub lyric: Option<String>,
}

/// Contents of the collapsed island: weather icon, clock, temperature.
#[derive(Debug, Clone, PartialEq)]
pub struct Compact {
    pub time: String,
    /// (icon, is_day, temperature label)
    pub weather: Option<(Icon, bool, String)>,
    /// While music plays the pill widens and shows the track instead of
    /// the weather.
    pub media: Option<CompactMedia>,
    /// Shown instead of everything else while set.
    pub toast: Option<Toast>,
    /// Countdown shown at the right while a timer runs.
    pub timer: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompactMedia {
    pub title: String,
    /// The current lyric line when known, otherwise the artist.
    pub second_line: String,
    pub is_lyric: bool,
    pub cover_id: Option<u64>,
}

/// Vertical offset of the liquid edge at `t` (0..1 along the bottom edge).
/// The envelope pins both ends to the corners so the outline stays closed.
pub fn wave(t: f32, amplitude: f32, phase: f32) -> f32 {
    let envelope = (PI * t.clamp(0.0, 1.0)).sin();
    amplitude * envelope * (t * TAU * 1.5 + phase).sin()
}

const WAVE_SEGMENTS: usize = 40;

#[derive(PartialEq)]
struct HomeKey {
    control: Option<ControlView>,
    text: Option<TextView>,
    shelf: Vec<ClipRow>,
    search: Option<SearchView>,
    tab: Tab,
    notes: Option<NotesView>,
    clip_page: Vec<ClipRow>,
    window: Option<WindowView>,
    clip_rows: Vec<ClipRow>,
    dock: Vec<u64>,
    hot: Option<Hit>,
    media: Option<MediaView>,
    scale: f32,
    theme: Theme,
    hovered: Option<usize>,
    tiles: Vec<Placed>,
    panel: (f32, f32),
    recording: bool,
}

struct HomeLayer {
    key: HomeKey,
    bitmap: ID2D1Bitmap1,
}

#[derive(PartialEq)]
struct CompactKey {
    scale: f32,
    theme: Theme,
    size: (f32, f32),
    compact: Compact,
}

struct CompactLayer {
    key: CompactKey,
    bitmap: ID2D1Bitmap1,
}

const SUN: Rgba = [1.0, 0.784, 0.239, 1.0];
const RAIN: Rgba = [0.353, 0.663, 1.0, 1.0];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Transport {
    Play,
    Pause,
    Prev,
    Next,
}

struct Fonts {
    scale: f32,
    title: IDWriteTextFormat,
    hint: IDWriteTextFormat,
    compact: IDWriteTextFormat,
    compact_right: IDWriteTextFormat,
    /// Left-aligned, one line, ellipsis when too long.
    track_title: IDWriteTextFormat,
    track_artist: IDWriteTextFormat,
    pill_title: IDWriteTextFormat,
    pill_artist: IDWriteTextFormat,
    small: IDWriteTextFormat,
    small_right: IDWriteTextFormat,
    lyric: IDWriteTextFormat,
    small_center: IDWriteTextFormat,
    note: IDWriteTextFormat,
    big: IDWriteTextFormat,
    tab: IDWriteTextFormat,
}

pub struct Renderer {
    swap_chain: IDXGISwapChain1,
    dxgi: IDXGIDevice3,
    factory: ID2D1Factory1,
    dc: ID2D1DeviceContext,
    brush: ID2D1SolidColorBrush,
    target: Option<ID2D1Bitmap1>,
    dwrite: IDWriteFactory,
    fonts: Option<Fonts>,
    home_layer: Option<HomeLayer>,
    compact_layer: Option<CompactLayer>,
    cover: Option<(u64, ID2D1Bitmap1)>,
    dock_icons: HashMap<u64, ID2D1Bitmap1>,
    dcomp: IDCompositionDevice,
    _comp_target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    size: (u32, u32),
    /// Vblanks per presented frame; keeps animations near 60 fps on
    /// high-refresh monitors (144 Hz -> every 2nd vblank).
    sync_interval: u32,
}

/// WARP (software) is the default: the island draws a few shapes, and the
/// hardware user-mode driver alone costs ~50 MB of private memory, while WARP
/// costs ~2 MB. `gpu = true` in the config opts into the hardware device.
fn create_d3d_device(gpu: bool) -> Result<ID3D11Device> {
    let mut last_err = None;
    let drivers: &[D3D_DRIVER_TYPE] = if gpu {
        &[D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP]
    } else {
        &[D3D_DRIVER_TYPE_WARP]
    };
    for &driver in drivers {
        let mut device = None;
        let res = unsafe {
            D3D11CreateDevice(
                None,
                driver,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_PREVENT_INTERNAL_THREADING_OPTIMIZATIONS,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
        };
        match res {
            Ok(()) => return Ok(device.unwrap()),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap())
}

/// Refresh rate of the primary display, in Hz (60 if unknown).
fn refresh_rate() -> u32 {
    use windows::Win32::Graphics::Gdi::{EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS};
    let mut mode = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    let ok = unsafe { EnumDisplaySettingsW(None, ENUM_CURRENT_SETTINGS, &mut mode) }.as_bool();
    if ok && mode.dmDisplayFrequency > 1 { mode.dmDisplayFrequency } else { 60 }
}

/// Vblanks to wait per frame so animations run at roughly 60 fps.
pub fn sync_interval_for(refresh_hz: u32) -> u32 {
    ((refresh_hz as f32 / 60.0).round() as u32).clamp(1, 4)
}

fn color(c: Rgba, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c[0], g: c[1], b: c[2], a: c[3] * alpha }
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn arc(to: Vector2, r: f32) -> D2D1_ARC_SEGMENT {
    D2D1_ARC_SEGMENT {
        point: to,
        size: D2D_SIZE_F { width: r, height: r },
        rotationAngle: 0.0,
        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
        arcSize: D2D1_ARC_SIZE_SMALL,
    }
}

impl Renderer {
    pub fn new(hwnd: HWND, width: u32, height: u32, gpu: bool) -> Result<Self> {
        unsafe {
            let d3d = create_d3d_device(gpu)?;
            let dxgi: IDXGIDevice3 = d3d.cast()?;

            let dxgi_factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width.max(1),
                Height: height.max(1),
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap_chain = dxgi_factory.CreateSwapChainForComposition(&d3d, &desc, None)?;

            let factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&D2D1_FACTORY_OPTIONS::default()))?;
            let d2d_device = factory.CreateDevice(&dxgi)?;
            let dc = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            dc.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = dc.CreateSolidColorBrush(&color([0.0; 4], 1.0), None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;

            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let comp_target = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = dcomp.CreateVisual()?;
            visual.SetContent(&swap_chain)?;
            comp_target.SetRoot(&visual)?;
            dcomp.Commit()?;

            Ok(Self {
                swap_chain,
                dxgi,
                factory,
                dc,
                brush,
                target: None,
                dwrite,
                fonts: None,
                home_layer: None,
                compact_layer: None,
                cover: None,
                dock_icons: HashMap::new(),
                dcomp,
                _comp_target: comp_target,
                _visual: visual,
                size: (width.max(1), height.max(1)),
                sync_interval: sync_interval_for(refresh_rate()),
            })
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        let size = (width.max(1), height.max(1));
        if size == self.size {
            return Ok(());
        }
        unsafe {
            self.dc.SetTarget(None);
            self.target = None;
            self.swap_chain.ResizeBuffers(0, size.0, size.1, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
            self.dcomp.Commit()?;
        }
        self.size = size;
        Ok(())
    }

    /// Hands driver scratch memory back to the system once the island has
    /// settled collapsed (~5 MB with WARP). D2D's own ClearResources was
    /// measured and freed nothing worth keeping, so it is not called.
    pub fn trim(&mut self) {
        unsafe { self.dxgi.Trim() };
    }

    fn ensure_target(&mut self) -> Result<()> {
        if self.target.is_some() {
            return Ok(());
        }
        unsafe {
            // With the flip model on D3D11, buffer 0 always refers to the
            // current back buffer, so one bitmap wrapper can be reused.
            let surface: IDXGISurface = self.swap_chain.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            let bitmap = self.dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.dc.SetTarget(&bitmap);
            self.target = Some(bitmap);
        }
        Ok(())
    }

    /// Drawn widths (DIPs) of a notice's title and text lines in the
    /// collapsed pill; None before the first frame made the fonts.
    pub fn pill_text_widths(&self, title: &str, text: &str) -> Option<(f32, f32)> {
        let fonts = self.fonts.as_ref()?;
        let width = |s: &str, fmt: &IDWriteTextFormat| -> Option<f32> {
            let wide: Vec<u16> = s.encode_utf16().collect();
            unsafe {
                let layout = self.dwrite.CreateTextLayout(&wide, fmt, 100_000.0, 1000.0).ok()?;
                let mut m = DWRITE_TEXT_METRICS::default();
                layout.GetMetrics(&mut m).ok()?;
                (m.lineCount == 1).then_some(m.widthIncludingTrailingWhitespace / fonts.scale)
            }
        };
        Some((width(title, &fonts.pill_title)?, width(text, &fonts.pill_artist)?))
    }

    fn ensure_fonts(&mut self, scale: f32) -> Result<()> {
        if self.fonts.as_ref().is_some_and(|f| f.scale == scale) {
            return Ok(());
        }
        unsafe {
            let make = |size: f32, weight: DWRITE_FONT_WEIGHT| -> Result<IDWriteTextFormat> {
                let f = self.dwrite.CreateTextFormat(
                    w!("Segoe UI Variable Text"),
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size * scale,
                    w!("ru-ru"),
                )?;
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                Ok(f)
            };
            let title = make(12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let hint = make(13.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            hint.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            hint.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let compact = make(13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            compact.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            compact.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let compact_right = make(13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            compact_right.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
            compact_right.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let ellipsized = |size: f32, weight: DWRITE_FONT_WEIGHT| -> Result<IDWriteTextFormat> {
                let f = make(size, weight)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                let sign = self.dwrite.CreateEllipsisTrimmingSign(&f)?;
                let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                f.SetTrimming(&trimming, &sign)?;
                Ok(f)
            };
            let track_title = ellipsized(13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let track_artist = ellipsized(12.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            let pill_title = ellipsized(12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let pill_artist = ellipsized(11.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            let small = make(10.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            let small_right = make(10.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            small_right.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
            let lyric = ellipsized(11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            lyric.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            let note = self.dwrite.CreateTextFormat(
                w!("Segoe UI Variable Text"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                13.0 * scale,
                w!("ru-ru"),
            )?;
            note.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            let big = make(30.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            big.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let tab = make(12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            tab.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let small_center = make(10.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            small_center.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            small_center.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            self.fonts = Some(Fonts {
                small_center,
                note,
                big,
                tab,
                scale,
                title,
                hint,
                compact,
                compact_right,
                track_title,
                track_artist,
                pill_title,
                pill_artist,
                small,
                small_right,
                lyric,
            });
        }
        Ok(())
    }

    /// Island outline with a liquid bottom edge.
    fn island_geometry(&self, s: &Shape, wobble: f32, phase: f32) -> Result<ID2D1PathGeometry1> {
        let (l, t, r, b) = (s.left, s.top, s.left + s.width, s.top + s.height);
        let rad = s.radius.min(s.height / 2.0).min(s.width / 2.0);
        unsafe {
            let geo = self.factory.CreatePathGeometry()?;
            let sink = geo.Open()?;
            sink.BeginFigure(pt(l + rad, t), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(pt(r - rad, t));
            sink.AddArc(&arc(pt(r, t + rad), rad));
            sink.AddLine(pt(r, b - rad));
            sink.AddArc(&arc(pt(r - rad, b), rad));
            let span = (r - rad) - (l + rad);
            for i in 1..=WAVE_SEGMENTS {
                let k = i as f32 / WAVE_SEGMENTS as f32;
                sink.AddLine(pt(r - rad - span * k, b + wave(k, wobble, phase)));
            }
            sink.AddArc(&arc(pt(l, b - rad), rad));
            sink.AddLine(pt(l, t + rad));
            sink.AddArc(&arc(pt(l + rad, t), rad));
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            Ok(geo)
        }
    }

    /// Draws the frame and presents. `Present` waits for vsync, which is
    /// what paces the animation loop.
    pub fn draw(&mut self, scene: &Scene) -> Result<bool> {
        self.ensure_target()?;
        self.ensure_fonts(scene.scale)?;
        self.ensure_cover(scene.cover)?;
        self.ensure_dock_icons(scene.dock, scene.extra_icons)?;
        let mut rebuilt = false;
        if scene.content_alpha > 0.01 {
            rebuilt |= self.ensure_home_layer(scene)?;
        }
        if scene.compact_alpha > 0.01 {
            rebuilt |= self.ensure_compact_layer(scene)?;
        }
        let s = &scene.shape;
        let th = scene.theme;
        unsafe {
            let dc = &self.dc;
            dc.BeginDraw();
            dc.Clear(Some(&color([0.0; 4], 0.0)));

            let mut fill = th.background;
            if let Some((tint, amount)) = scene.tint {
                for i in 0..3 {
                    fill[i] += (tint[i] - fill[i]) * amount.clamp(0.0, 1.0);
                }
            }
            self.brush.SetColor(&color(fill, 1.0));
            if scene.wobble.abs() < 0.05 {
                let rad = s.radius.min(s.height / 2.0);
                let rr = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F { left: s.left, top: s.top, right: s.left + s.width, bottom: s.top + s.height },
                    radiusX: rad,
                    radiusY: rad,
                };
                dc.FillRoundedRectangle(&rr, &self.brush);
            } else {
                let geo = self.island_geometry(s, scene.wobble, scene.phase)?;
                dc.FillGeometry(&geo, &self.brush, None);
            }

            if let (true, Some(layer)) = (scene.compact_alpha > 0.01, self.compact_layer.as_ref()) {
                let size = layer.bitmap.GetPixelSize();
                let left = (s.left + (s.width - size.width as f32) / 2.0).round();
                let top = (s.top + (s.height - size.height as f32) / 2.0).round();
                let dest = D2D_RECT_F { left, top, right: left + size.width as f32, bottom: top + size.height as f32 };
                dc.DrawBitmap(&layer.bitmap, Some(&dest), scene.compact_alpha, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None, None);
            }

            if let (true, Some(layer)) = (scene.content_alpha > 0.01, self.home_layer.as_ref()) {
                let clip = D2D_RECT_F { left: s.left, top: s.top, right: s.left + s.width, bottom: s.top + s.height };
                dc.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
                // Whole-pixel placement keeps the cached text crisp.
                let (ox, oy) = (scene.origin.0.round(), scene.origin.1.round());
                let size = layer.bitmap.GetPixelSize();
                let dest = D2D_RECT_F { left: ox, top: oy, right: ox + size.width as f32, bottom: oy + size.height as f32 };
                dc.DrawBitmap(&layer.bitmap, Some(&dest), scene.content_alpha, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None, None);
                dc.PopAxisAlignedClip();
            }

            self.paint_privacy(scene);

            dc.EndDraw(None, None)?;
            let interval = if scene.relaxed { (self.sync_interval * 2).min(4) } else { self.sync_interval };
            self.swap_chain.Present(interval, DXGI_PRESENT(0)).ok()?;
        }
        Ok(rebuilt)
    }

    /// A premultiplied BGRA bitmap usable as a D2D target and as a source.
    /// `reuse` is kept when it already has the right size.
    fn layer_bitmap(&self, reuse: Option<ID2D1Bitmap1>, size: D2D_SIZE_U) -> Result<ID2D1Bitmap1> {
        unsafe {
            if let Some(b) = reuse {
                if b.GetPixelSize() == size {
                    return Ok(b);
                }
            }
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                ..Default::default()
            };
            self.dc.CreateBitmap(size, None, 0, &props)
        }
    }

    /// Clears `bitmap` and paints into it, then points the context back at
    /// the swap chain.
    fn paint_layer(&self, bitmap: &ID2D1Bitmap1, paint: impl FnOnce(&Self)) -> Result<()> {
        unsafe {
            self.dc.SetTarget(bitmap);
            self.dc.BeginDraw();
            self.dc.Clear(Some(&color([0.0; 4], 0.0)));
            paint(self);
            let res = self.dc.EndDraw(None, None);
            match self.target.as_ref() {
                Some(t) => self.dc.SetTarget(t),
                None => self.dc.SetTarget(None),
            }
            res
        }
    }

    /// Caches the collapsed island's content; it changes once a minute.
    fn ensure_compact_layer(&mut self, scene: &Scene) -> Result<bool> {
        let key = CompactKey {
            scale: scene.scale,
            theme: *scene.theme,
            size: scene.compact_size,
            compact: scene.compact.clone(),
        };
        if self.compact_layer.as_ref().is_some_and(|l| l.key == key) {
            return Ok(false);
        }
        let k = scene.scale;
        let size = D2D_SIZE_U {
            width: (scene.compact_size.0 * k).round().max(1.0) as u32,
            height: (scene.compact_size.1 * k).round().max(1.0) as u32,
        };
        let old = self.compact_layer.take().map(|l| l.bitmap);
        let bitmap = self.layer_bitmap(old, size)?;
        self.paint_layer(&bitmap, |r| r.paint_compact(scene, size))?;
        self.compact_layer = Some(CompactLayer { key, bitmap });
        Ok(true)
    }

    fn paint_compact(&self, scene: &Scene, size: D2D_SIZE_U) {
        let fonts = self.fonts.as_ref().unwrap();
        let k = scene.scale;
        let th = scene.theme;
        let (w, h) = (size.width as f32, size.height as f32);
        let pad = 14.0 * k;
        let full = D2D_RECT_F { left: 0.0, top: 0.0, right: w, bottom: h };
        unsafe {
            if let Some(t) = &scene.compact.toast {
                // [glyph] title / text
                let side = h - 10.0 * k;
                let badge = D2D_RECT_F { left: 5.0 * k, top: 5.0 * k, right: 5.0 * k + side, bottom: 5.0 * k + side };
                self.paint_badge(&t.badge, badge, k, th);
                let text_l = badge.right + 9.0 * k;
                let buttons = layout::toast_actions(w / k, h / k, &t.actions);
                let text_r = buttons.first().map_or(w - pad, |b| b.x * k - 8.0 * k);
                for (b, label) in buttons.iter().zip(&t.actions) {
                    let r = D2D_RECT_F { left: b.x * k, top: b.y * k, right: (b.x + b.w) * k, bottom: (b.y + b.h) * k };
                    let check = label == "✓";
                    self.fill_rect(if check { th.accent } else { th.tile_hover }, r, (r.bottom - r.top) / 2.0);
                    if check {
                        // Check mark.
                        let (cx, cy) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
                        self.line([1.0; 4], cx - 5.0 * k, cy, cx - 1.5 * k, cy + 3.5 * k, 2.0 * k);
                        self.line([1.0; 4], cx - 1.5 * k, cy + 3.5 * k, cx + 5.5 * k, cy - 4.0 * k, 2.0 * k);
                    } else {
                        self.brush.SetColor(&color(th.text, 1.0));
                        let l: Vec<u16> = label.encode_utf16().collect();
                        self.dc.DrawText(&l, &fonts.lyric, &r, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                    }
                }
                let top = D2D_RECT_F { left: text_l, top: 3.0 * k, right: text_r, bottom: h / 2.0 + 1.0 * k };
                let bottom = D2D_RECT_F { left: text_l, top: h / 2.0 - 1.0 * k, right: text_r, bottom: h - 3.0 * k };
                self.brush.SetColor(&color(th.text, 1.0));
                let title: Vec<u16> = t.title.encode_utf16().collect();
                self.dc.DrawText(&title, &fonts.pill_title, &top, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                if let Some(level) = t.level {
                    // Level bar with the percentage after it.
                    let cy = (bottom.top + bottom.bottom) / 2.0 + 1.0 * k;
                    let pct = D2D_RECT_F { left: bottom.right - 34.0 * k, ..bottom };
                    let bar = D2D_RECT_F { left: bottom.left, top: cy - 2.0 * k, right: pct.left - 8.0 * k, bottom: cy + 2.0 * k };
                    let mut track = th.text;
                    track[3] = 0.18;
                    self.fill_rect(track, bar, 2.0 * k);
                    let filled = bar.left + (bar.right - bar.left) * level.clamp(0.0, 1.0);
                    if filled > bar.left + 0.5 {
                        self.fill_rect(th.accent, D2D_RECT_F { right: filled, ..bar }, 2.0 * k);
                    }
                    self.brush.SetColor(&color(th.subtext, 1.0));
                    let p: Vec<u16> = format!("{}%", (level * 100.0).round() as u32).encode_utf16().collect();
                    self.dc.DrawText(&p, &fonts.small_right, &pct, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                } else {
                    self.brush.SetColor(&color(th.subtext, 1.0));
                    let text: Vec<u16> = t.text.encode_utf16().collect();
                    self.dc.DrawText(&text, &fonts.pill_artist, &bottom, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                }
                return;
            }
            if let Some(m) = &scene.compact.media {
                let (title, artist) = (&m.title, &m.second_line);
                // [cover] title / artist ............ 14:43
                let side = h - 10.0 * k;
                let cover = D2D_RECT_F { left: 5.0 * k, top: 5.0 * k, right: 5.0 * k + side, bottom: 5.0 * k + side };
                self.paint_cover(&cover, side / 2.0, th);
                let clock = D2D_RECT_F { left: 0.0, top: 0.0, right: w - pad, bottom: h };
                self.brush.SetColor(&color(if scene.compact.timer.is_some() { th.accent } else { th.text }, 1.0));
                let time: Vec<u16> = scene.compact.timer.as_deref().unwrap_or(&scene.compact.time).encode_utf16().collect();
                self.dc.DrawText(&time, &fonts.compact_right, &clock, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                let text_l = cover.right + 9.0 * k;
                let text_r = w - pad - 48.0 * k;
                let t = D2D_RECT_F { left: text_l, top: 3.0 * k, right: text_r, bottom: h / 2.0 + 1.0 * k };
                let a = D2D_RECT_F { left: text_l, top: h / 2.0 - 1.0 * k, right: text_r, bottom: h - 3.0 * k };
                let title: Vec<u16> = title.encode_utf16().collect();
                // The countdown above is drawn in the accent; the title is not.
                self.brush.SetColor(&color(th.text, 1.0));
                self.dc.DrawText(&title, &fonts.pill_title, &t, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                self.brush.SetColor(&color(if m.is_lyric { th.text } else { th.subtext }, 1.0));
                let artist: Vec<u16> = artist.encode_utf16().collect();
                self.dc.DrawText(&artist, &fonts.pill_artist, &a, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                return;
            }
            self.brush.SetColor(&color(th.text, 1.0));
            let time: Vec<u16> = scene.compact.time.encode_utf16().collect();
            self.dc.DrawText(&time, &fonts.compact, &full, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            if let Some(left) = &scene.compact.timer {
                let right = D2D_RECT_F { left: 0.0, top: 0.0, right: w - pad, bottom: h };
                self.brush.SetColor(&color(th.accent, 1.0));
                let t: Vec<u16> = left.encode_utf16().collect();
                self.dc.DrawText(&t, &fonts.compact_right, &right, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                self.paint_badge(&Badge::Timer, D2D_RECT_F { left: pad - 4.0 * k, top: h / 2.0 - 12.0 * k, right: pad + 20.0 * k, bottom: h / 2.0 + 12.0 * k }, k, th);
            } else if let Some((icon, day, temp)) = &scene.compact.weather {
                let right = D2D_RECT_F { left: 0.0, top: 0.0, right: w - pad, bottom: h };
                let temp: Vec<u16> = temp.encode_utf16().collect();
                self.dc.DrawText(&temp, &fonts.compact_right, &right, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                self.paint_icon(*icon, *day, pad + 8.0 * k, h / 2.0, k, th);
            }
        }
    }

    /// Uploads the cover pixels once per track.
    fn ensure_cover(&mut self, cover: Option<&Cover>) -> Result<()> {
        let Some(cover) = cover else {
            self.cover = None;
            return Ok(());
        };
        if self.cover.as_ref().is_some_and(|(id, _)| *id == cover.id) {
            return Ok(());
        }
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let size = D2D_SIZE_U { width: COVER_PX, height: COVER_PX };
        let bitmap = unsafe {
            self.dc.CreateBitmap(size, Some(cover.pixels.as_ptr() as *const _), COVER_PX * 4, &props)?
        };
        self.cover = Some((cover.id, bitmap));
        Ok(())
    }

    /// Cover art in a rounded rectangle; a neutral square when there is none.
    fn paint_cover(&self, rect: &D2D_RECT_F, radius: f32, th: &Theme) {
        let rr = D2D1_ROUNDED_RECT { rect: *rect, radiusX: radius, radiusY: radius };
        unsafe {
            if let Some((_, bitmap)) = &self.cover {
                let props = D2D1_BITMAP_BRUSH_PROPERTIES1 {
                    extendModeX: D2D1_EXTEND_MODE_CLAMP,
                    extendModeY: D2D1_EXTEND_MODE_CLAMP,
                    interpolationMode: D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                };
                if let Ok(brush) = self.dc.CreateBitmapBrush(bitmap, Some(&props), None) {
                    let sx = (rect.right - rect.left) / COVER_PX as f32;
                    let sy = (rect.bottom - rect.top) / COVER_PX as f32;
                    brush.SetTransform(&Matrix3x2 { M11: sx, M12: 0.0, M21: 0.0, M22: sy, M31: rect.left, M32: rect.top });
                    self.dc.FillRoundedRectangle(&rr, &brush);
                    return;
                }
            }
            self.brush.SetColor(&color(th.tile_hover, 1.0));
            self.dc.FillRoundedRectangle(&rr, &self.brush);
            // A small note glyph so the placeholder reads as "music".
            let (cx, cy) = ((rect.left + rect.right) / 2.0, (rect.top + rect.bottom) / 2.0);
            let k = (rect.right - rect.left) / 64.0;
            self.fill_circle(th.subtext, cx - 4.0 * k, cy + 7.0 * k, 5.0 * k);
            self.line(th.subtext, cx + 0.5 * k, cy + 7.0 * k, cx + 0.5 * k, cy - 12.0 * k, 2.2 * k);
            self.line(th.subtext, cx, cy - 11.0 * k, cx + 9.0 * k, cy - 7.0 * k, 2.2 * k);
        }
    }

    fn fill_polygon(&self, c: Rgba, points: &[(f32, f32)]) {
        unsafe {
            let Ok(geo) = self.factory.CreatePathGeometry() else { return };
            let Ok(sink) = geo.Open() else { return };
            sink.BeginFigure(pt(points[0].0, points[0].1), D2D1_FIGURE_BEGIN_FILLED);
            for &(x, y) in &points[1..] {
                sink.AddLine(pt(x, y));
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            if sink.Close().is_ok() {
                self.brush.SetColor(&color(c, 1.0));
                self.dc.FillGeometry(&geo, &self.brush, None);
            }
        }
    }

    fn fill_rect(&self, c: Rgba, r: D2D_RECT_F, radius: f32) {
        unsafe {
            self.brush.SetColor(&color(c, 1.0));
            self.dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }, &self.brush);
        }
    }

    /// Play/pause, previous and next glyphs centered in `r` (pixels).
    fn paint_transport(&self, glyph: Transport, r: D2D_RECT_F, k: f32, c: Rgba) {
        let (cx, cy) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
        let tri = |dir: f32, x: f32, size: f32| {
            vec![(x - dir * size * 0.5, cy - size * 0.6), (x + dir * size * 0.6, cy), (x - dir * size * 0.5, cy + size * 0.6)]
        };
        match glyph {
            Transport::Play => self.fill_polygon(c, &tri(1.0, cx + 1.0 * k, 14.0 * k)),
            Transport::Pause => {
                for dx in [-3.5, 3.5] {
                    let x = cx + dx * k;
                    self.fill_rect(c, D2D_RECT_F { left: x - 2.0 * k, top: cy - 7.0 * k, right: x + 2.0 * k, bottom: cy + 7.0 * k }, 1.0 * k);
                }
            }
            Transport::Next | Transport::Prev => {
                let dir = if glyph == Transport::Next { 1.0 } else { -1.0 };
                self.fill_polygon(c, &tri(dir, cx - dir * 2.0 * k, 10.0 * k));
                let bar = cx + dir * 5.0 * k;
                self.fill_rect(c, D2D_RECT_F { left: bar - 1.2 * k, top: cy - 6.0 * k, right: bar + 1.2 * k, bottom: cy + 6.0 * k }, 0.8 * k);
            }
        }
    }

    fn paint_player(&self, tile: &Placed, media: &MediaView, hot: Option<Hit>, k: f32, th: &Theme) {
        let fonts = self.fonts.as_ref().unwrap();
        let r = layout::media_regions(tile);
        let px = |rc: Rect| D2D_RECT_F { left: rc.x * k, top: rc.y * k, right: (rc.x + rc.w) * k, bottom: (rc.y + rc.h) * k };
        let text = |s: &str, fmt: &IDWriteTextFormat, rc: Rect, c: Rgba| unsafe {
            self.brush.SetColor(&color(c, 1.0));
            let w: Vec<u16> = s.encode_utf16().collect();
            self.dc.DrawText(&w, fmt, &px(rc), &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        };

        self.paint_cover(&px(r.cover), 10.0 * k, th);
        let title = if media.title.is_empty() { "Без названия" } else { &media.title };
        text(title, &fonts.track_title, r.title, th.text);
        text(&media.artist, &fonts.track_artist, r.artist, th.subtext);

        let dim = |on: bool| if on { th.text } else { th.subtext };
        for (hit, rc) in [(Hit::MediaPrev, r.prev), (Hit::MediaPlay, r.play), (Hit::MediaNext, r.next)] {
            if hot == Some(hit) {
                let b = px(rc);
                self.fill_circle(th.tile_hover, (b.left + b.right) / 2.0, (b.top + b.bottom) / 2.0, 16.0 * k);
            }
        }
        self.paint_transport(Transport::Prev, px(r.prev), k, dim(media.can_prev));
        self.paint_transport(if media.playing { Transport::Pause } else { Transport::Play }, px(r.play), k, th.text);
        self.paint_transport(Transport::Next, px(r.next), k, dim(media.can_next));

        let mut track = th.text;
        track[3] = 0.18;
        let bar = px(r.progress);
        self.fill_rect(track, bar, 2.0 * k);
        let filled = bar.left + (bar.right - bar.left) * media.progress_permille as f32 / 1000.0;
        if filled > bar.left + 0.5 {
            self.fill_rect(th.accent, D2D_RECT_F { right: filled, ..bar }, 2.0 * k);
        }
        text(&media.position, &fonts.small, r.times, th.subtext);
        if let Some(lyric) = media.lyric.as_deref().filter(|l| !l.is_empty()) {
            // Between the two time labels.
            let mid = Rect { x: r.times.x + 34.0, w: r.times.w - 68.0, ..r.times };
            text(lyric, &fonts.lyric, mid, th.text);
        }
        unsafe {
            self.brush.SetColor(&color(th.subtext, 1.0));
            let w: Vec<u16> = media.duration.encode_utf16().collect();
            self.dc.DrawText(&w, &fonts.small_right, &px(r.times), &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }

        if let Some(vol) = media.volume {
            // Speaker: body + cone.
            let sp = px(r.speaker);
            let (sx, sy) = (sp.left, (sp.top + sp.bottom) / 2.0);
            self.fill_rect(th.subtext, D2D_RECT_F { left: sx, top: sy - 2.5 * k, right: sx + 4.0 * k, bottom: sy + 2.5 * k }, 0.5 * k);
            self.fill_polygon(th.subtext, &[(sx + 3.0 * k, sy - 2.5 * k), (sx + 8.0 * k, sy - 6.0 * k), (sx + 8.0 * k, sy + 6.0 * k), (sx + 3.0 * k, sy + 2.5 * k)]);
            let vbar = px(r.volume);
            self.fill_rect(track, vbar, 2.0 * k);
            let x = vbar.left + (vbar.right - vbar.left) * vol as f32 / 100.0;
            self.fill_rect(th.text, D2D_RECT_F { right: x.max(vbar.left + 1.0), ..vbar }, 2.0 * k);
            self.fill_circle(th.text, x, (vbar.top + vbar.bottom) / 2.0, 5.0 * k);
        }
    }

    /// Uploads dock icons once; forgets icons of unpinned apps.
    fn ensure_dock_icons(&mut self, dock: &[DockIcon], extra: &[DockIcon]) -> Result<()> {
        self.dock_icons.retain(|id, _| dock.iter().chain(extra).any(|d| d.id == *id));
        for d in dock.iter().chain(extra) {
            let (Some(pixels), false) = (&d.icon, self.dock_icons.contains_key(&d.id)) else { continue };
            let px = crate::dock::ICON_PX;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let bitmap = unsafe {
                self.dc.CreateBitmap(D2D_SIZE_U { width: px, height: px }, Some(pixels.as_ptr() as *const _), px * 4, &props)?
            };
            self.dock_icons.insert(d.id, bitmap);
        }
        Ok(())
    }

    /// Small monochrome glyph (about 14 DIPs) centered at (x, y).
    fn paint_glyph(&self, glyph: Glyph, x: f32, y: f32, k: f32, th: &Theme) {
        let c = th.text;
        match glyph {
            Glyph::Text => {
                for (i, w) in [12.0, 12.0, 8.0].iter().enumerate() {
                    let yy = y - 4.0 * k + i as f32 * 4.0 * k;
                    self.line(c, x - 6.0 * k, yy, x - 6.0 * k + w * k, yy, 1.5 * k);
                }
            }
            Glyph::Link => {
                // Two interlocking rounded links, drawn as outlines.
                unsafe {
                    self.brush.SetColor(&color(c, 1.0));
                    for (dx, dy) in [(-2.5, 2.5), (2.5, -2.5)] {
                        let rr = D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F { left: x + (dx - 4.5) * k, top: y + (dy - 2.5) * k, right: x + (dx + 4.5) * k, bottom: y + (dy + 2.5) * k },
                            radiusX: 2.5 * k,
                            radiusY: 2.5 * k,
                        };
                        self.dc.DrawRoundedRectangle(&rr, &self.brush, 1.4 * k, None);
                    }
                }
            }
            Glyph::Image => {
                unsafe {
                    self.brush.SetColor(&color(c, 1.0));
                    let frame = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F { left: x - 7.0 * k, top: y - 5.5 * k, right: x + 7.0 * k, bottom: y + 5.5 * k },
                        radiusX: 1.5 * k,
                        radiusY: 1.5 * k,
                    };
                    self.dc.DrawRoundedRectangle(&frame, &self.brush, 1.4 * k, None);
                }
                self.fill_polygon(c, &[(x - 5.0 * k, y + 4.0 * k), (x - 1.0 * k, y - 1.0 * k), (x + 2.0 * k, y + 2.0 * k), (x + 3.5 * k, y + 0.5 * k), (x + 5.5 * k, y + 4.0 * k)]);
                self.fill_circle(c, x + 3.0 * k, y - 2.5 * k, 1.3 * k);
            }
            Glyph::Files => {
                self.fill_polygon(
                    c,
                    &[(x - 7.0 * k, y - 5.0 * k), (x - 2.0 * k, y - 5.0 * k), (x, y - 3.0 * k), (x + 7.0 * k, y - 3.0 * k), (x + 7.0 * k, y + 5.5 * k), (x - 7.0 * k, y + 5.5 * k)],
                );
            }
        }
    }

    fn paint_clipboard(&self, tile: &Placed, scene: &Scene, k: f32, th: &Theme) {
        let fonts = self.fonts.as_ref().unwrap();
        let rows = layout::clip_rows(tile);
        let px = |rc: Rect| D2D_RECT_F { left: rc.x * k, top: rc.y * k, right: (rc.x + rc.w) * k, bottom: (rc.y + rc.h) * k };
        unsafe {
            if scene.clip_rows.is_empty() {
                let area = Rect { x: tile.x, y: tile.y + 28.0, w: tile.w, h: tile.h - 28.0 };
                self.brush.SetColor(&color(th.subtext, 1.0));
                let w: Vec<u16> = "Скопируйте что-нибудь — оно появится здесь".encode_utf16().collect();
                self.dc.DrawText(&w, &fonts.lyric, &px(area), &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                return;
            }
            for (i, (row, slot)) in scene.clip_rows.iter().zip(rows).enumerate() {
                let r = px(slot);
                if scene.hot == Some(Hit::Clip(i)) {
                    self.fill_rect(th.tile_hover, r, 8.0 * k);
                }
                let cy = (r.top + r.bottom) / 2.0;
                self.paint_glyph(row.glyph, r.left + 14.0 * k, cy, k * 0.9, th);
                let ago = D2D_RECT_F { left: r.right - 60.0 * k, right: r.right - 8.0 * k, ..r };
                self.brush.SetColor(&color(th.subtext, 1.0));
                let a: Vec<u16> = row.ago.encode_utf16().collect();
                let ago_mid = D2D_RECT_F { top: cy - 7.0 * k, bottom: cy + 7.0 * k, ..ago };
                self.dc.DrawText(&a, &fonts.small_right, &ago_mid, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                let text = D2D_RECT_F { left: r.left + 28.0 * k, right: ago.left - 4.0 * k, ..r };
                self.brush.SetColor(&color(th.text, 1.0));
                let t: Vec<u16> = row.text.encode_utf16().collect();
                self.dc.DrawText(&t, &fonts.track_artist, &text, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            }
        }
    }

    fn paint_dock(&self, tile: &Placed, scene: &Scene, k: f32, th: &Theme) {
        let (slots, add) = layout::dock_slots(tile, scene.dock.len());
        let px = |rc: Rect| D2D_RECT_F { left: rc.x * k, top: rc.y * k, right: (rc.x + rc.w) * k, bottom: (rc.y + rc.h) * k };
        for (i, (app, slot)) in scene.dock.iter().zip(slots).enumerate() {
            let r = px(slot);
            if scene.hot == Some(Hit::Dock(i)) {
                let pad = 4.0 * k;
                self.fill_rect(th.tile_hover, D2D_RECT_F { left: r.left - pad, top: r.top - pad, right: r.right + pad, bottom: r.bottom + pad }, 10.0 * k);
            }
            match self.dock_icons.get(&app.id) {
                Some(bitmap) => unsafe {
                    self.dc.DrawBitmap(bitmap, Some(&r), 1.0, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None);
                },
                None => self.fill_rect(th.tile_hover, r, 10.0 * k),
            }
        }
        let r = px(add);
        let fill = if scene.hot == Some(Hit::DockAdd) { th.tile_hover } else { th.tile };
        self.fill_rect(fill, r, 10.0 * k);
        let (cx, cy) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
        self.line(th.subtext, cx - 7.0 * k, cy, cx + 7.0 * k, cy, 2.0 * k);
        self.line(th.subtext, cx, cy - 7.0 * k, cx, cy + 7.0 * k, 2.0 * k);
    }

    /// Badge of a notice: a round chip with an icon, glyph or short text.
    fn paint_badge(&self, badge: &Badge, r: D2D_RECT_F, k: f32, th: &Theme) {
        let (cx, cy) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
        let side = r.right - r.left;
        if let Badge::App(id) = badge {
            if let Some(bitmap) = self.dock_icons.get(id) {
                let pad = 2.0 * k;
                let dest = D2D_RECT_F { left: r.left + pad, top: r.top + pad, right: r.right - pad, bottom: r.bottom - pad };
                unsafe {
                    self.dc.DrawBitmap(bitmap, Some(&dest), 1.0, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None);
                }
                return;
            }
        }
        let chip = match badge {
            Badge::Camera => [0.204, 0.78, 0.349, 1.0],
            Badge::Mic => [1.0, 0.624, 0.039, 1.0],
            Badge::Record => [1.0, 0.231, 0.188, 1.0],
            _ => th.tile_hover,
        };
        self.fill_rect(chip, r, side / 2.0);
        let ink = if matches!(badge, Badge::Camera | Badge::Mic) { [1.0, 1.0, 1.0, 1.0] } else { th.text };
        match badge {
            Badge::Glyph(g) => self.paint_glyph(*g, cx, cy, k, th),
            Badge::App(_) => self.paint_glyph(Glyph::Files, cx, cy, k, th),
            Badge::Text(t) => unsafe {
                let fonts = self.fonts.as_ref().unwrap();
                self.brush.SetColor(&color(ink, 1.0));
                let w: Vec<u16> = t.encode_utf16().collect();
                self.dc.DrawText(&w, &fonts.lyric, &r, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            },
            Badge::Volume | Badge::Muted => {
                let (sx, sy) = (cx - 6.0 * k, cy);
                self.fill_rect(ink, D2D_RECT_F { left: sx, top: sy - 2.5 * k, right: sx + 3.5 * k, bottom: sy + 2.5 * k }, 0.5 * k);
                self.fill_polygon(ink, &[(sx + 2.5 * k, sy - 2.5 * k), (sx + 7.0 * k, sy - 6.0 * k), (sx + 7.0 * k, sy + 6.0 * k), (sx + 2.5 * k, sy + 2.5 * k)]);
                if *badge == Badge::Muted {
                    self.line(ink, sx + 9.0 * k, sy - 3.0 * k, sx + 14.0 * k, sy + 3.0 * k, 1.4 * k);
                    self.line(ink, sx + 14.0 * k, sy - 3.0 * k, sx + 9.0 * k, sy + 3.0 * k, 1.4 * k);
                } else {
                    self.line(ink, sx + 9.5 * k, sy - 2.5 * k, sx + 9.5 * k, sy + 2.5 * k, 1.4 * k);
                    self.line(ink, sx + 12.5 * k, sy - 4.5 * k, sx + 12.5 * k, sy + 4.5 * k, 1.4 * k);
                }
            }
            Badge::Brightness => self.paint_sun(cx, cy, k * 0.9),
            Badge::Timer => {
                unsafe {
                    self.brush.SetColor(&color(ink, 1.0));
                    let e = D2D1_ELLIPSE { point: pt(cx, cy + 1.0 * k), radiusX: 6.0 * k, radiusY: 6.0 * k };
                    self.dc.DrawEllipse(&e, &self.brush, 1.5 * k, None);
                }
                self.line(ink, cx, cy + 1.0 * k, cx, cy - 2.5 * k, 1.5 * k);
                self.line(ink, cx - 2.0 * k, cy - 7.0 * k, cx + 2.0 * k, cy - 7.0 * k, 1.5 * k);
            }
            Badge::Bell => {
                self.fill_polygon(
                    ink,
                    &[(cx - 5.5 * k, cy + 3.5 * k), (cx - 4.0 * k, cy + 1.0 * k), (cx - 4.0 * k, cy - 2.5 * k), (cx - 2.0 * k, cy - 5.5 * k), (cx + 2.0 * k, cy - 5.5 * k), (cx + 4.0 * k, cy - 2.5 * k), (cx + 4.0 * k, cy + 1.0 * k), (cx + 5.5 * k, cy + 3.5 * k)],
                );
                self.fill_circle(ink, cx, cy + 5.5 * k, 1.6 * k);
            }
            Badge::Camera => {
                self.fill_rect(ink, D2D_RECT_F { left: cx - 6.5 * k, top: cy - 4.0 * k, right: cx + 2.5 * k, bottom: cy + 4.0 * k }, 1.5 * k);
                self.fill_polygon(ink, &[(cx + 3.0 * k, cy - 1.0 * k), (cx + 7.0 * k, cy - 4.0 * k), (cx + 7.0 * k, cy + 4.0 * k), (cx + 3.0 * k, cy + 1.0 * k)]);
            }
            Badge::Color(c) => {
                let fill = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0];
                self.fill_circle(fill, cx, cy, side / 2.0 - 4.0 * k);
            }
            Badge::Record => self.fill_circle([1.0; 4], cx, cy, 4.0 * k),
            Badge::Mic => {
                self.fill_rect(ink, D2D_RECT_F { left: cx - 2.5 * k, top: cy - 7.0 * k, right: cx + 2.5 * k, bottom: cy + 2.0 * k }, 2.5 * k);
                self.line(ink, cx - 5.0 * k, cy - 1.0 * k, cx - 5.0 * k + 0.01, cy + 1.5 * k, 1.3 * k);
                self.line(ink, cx + 5.0 * k, cy - 1.0 * k, cx + 5.0 * k + 0.01, cy + 1.5 * k, 1.3 * k);
                self.line(ink, cx - 5.0 * k, cy + 1.5 * k, cx - 2.0 * k, cy + 4.5 * k, 1.3 * k);
                self.line(ink, cx + 5.0 * k, cy + 1.5 * k, cx + 2.0 * k, cy + 4.5 * k, 1.3 * k);
                self.line(ink, cx, cy + 4.5 * k, cx, cy + 7.0 * k, 1.3 * k);
            }
        }
    }

    /// Green (camera) and orange (microphone) dots, like on a phone.
    fn paint_privacy(&self, scene: &Scene) {
        let (cam, mic) = scene.privacy;
        if !cam && !mic {
            return;
        }
        let s = &scene.shape;
        let k = scene.scale;
        let r = 2.8 * k;
        let colors: Vec<Rgba> = [(cam, [0.204, 0.78, 0.349, 1.0]), (mic, [1.0, 0.624, 0.039, 1.0])]
            .into_iter()
            .filter(|(on, _)| *on)
            .map(|(_, c)| c)
            .collect();
        // Open panel: a row in the top-right corner. Pill: stacked at its
        // right end. Thin handle: a row inside its right end.
        let (open, thin) = (s.height > 60.0 * k, s.height < 12.0 * k);
        for (i, c) in colors.iter().enumerate() {
            let i = i as f32;
            let (x, y, radius) = if open {
                (s.left + s.width - 18.0 * k - i * 8.0 * k, s.top + 14.0 * k, r)
            } else if thin {
                (s.left + s.width - 4.0 * k - i * 7.0 * k, s.top + s.height / 2.0, (s.height / 2.0 - 0.5).clamp(1.0, r))
            } else {
                let spread = (colors.len() as f32 - 1.0) * 3.5 * k;
                (s.left + s.width - 9.0 * k, s.top + s.height / 2.0 - spread + i * 7.0 * k, r)
            };
            self.fill_circle(*c, x, y, radius);
        }
    }

    fn paint_window_tile(&self, tile: &Placed, scene: &Scene, k: f32, th: &Theme) {
        let fonts = self.fonts.as_ref().unwrap();
        let r = layout::window_regions(tile);
        let px = |rc: Rect| D2D_RECT_F { left: rc.x * k, top: rc.y * k, right: (rc.x + rc.w) * k, bottom: (rc.y + rc.h) * k };
        let Some(win) = scene.window else {
            unsafe {
                self.brush.SetColor(&color(th.subtext, 1.0));
                let w: Vec<u16> = "Нет активного окна".encode_utf16().collect();
                self.dc.DrawText(&w, &fonts.lyric, &px(Rect { x: tile.x, y: tile.y, w: tile.w, h: tile.h }), &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            }
            return;
        };
        match win.icon_id.and_then(|id| self.dock_icons.get(&id)) {
            Some(bitmap) => unsafe { self.dc.DrawBitmap(bitmap, Some(&px(r.icon)), 1.0, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None) },
            None => self.fill_rect(th.tile_hover, px(r.icon), 10.0 * k),
        }
        unsafe {
            self.brush.SetColor(&color(th.text, 1.0));
            let n: Vec<u16> = win.name.encode_utf16().collect();
            self.dc.DrawText(&n, &fonts.track_title, &px(r.name), &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            self.brush.SetColor(&color(th.subtext, 1.0));
            let t: Vec<u16> = win.title.encode_utf16().collect();
            self.dc.DrawText(&t, &fonts.track_artist, &px(r.title), &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }
        for (hit, rc, active) in [
            (Hit::WinPin, r.pin, win.topmost),
            (Hit::WinOpacity, r.opacity, win.opacity < 100),
            (Hit::WinMinimize, r.minimize, false),
        ] {
            let b = px(rc);
            let fill = if active { th.accent } else if scene.hot == Some(hit) { th.tile_hover } else { th.tile };
            self.fill_rect(fill, b, 9.0 * k);
            let ink = if active { [1.0, 1.0, 1.0, 1.0] } else { th.text };
            let (cx, cy) = ((b.left + b.right) / 2.0, (b.top + b.bottom) / 2.0);
            match hit {
                Hit::WinPin => {
                    // Push-pin: head, body, needle.
                    self.fill_rect(ink, D2D_RECT_F { left: cx - 4.0 * k, top: cy - 7.0 * k, right: cx + 4.0 * k, bottom: cy - 5.0 * k }, 1.0 * k);
                    self.fill_polygon(ink, &[(cx - 2.5 * k, cy - 5.0 * k), (cx + 2.5 * k, cy - 5.0 * k), (cx + 5.0 * k, cy + 1.0 * k), (cx - 5.0 * k, cy + 1.0 * k)]);
                    self.line(ink, cx, cy + 1.0 * k, cx, cy + 7.0 * k, 1.4 * k);
                }
                Hit::WinOpacity => {
                    // Half-filled drop.
                    unsafe {
                        self.brush.SetColor(&color(ink, 1.0));
                        let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: 6.0 * k, radiusY: 6.0 * k };
                        self.dc.DrawEllipse(&e, &self.brush, 1.4 * k, None);
                    }
                    self.fill_polygon(ink, &[(cx, cy - 6.0 * k), (cx + 4.3 * k, cy - 4.2 * k), (cx + 6.0 * k, cy), (cx + 4.3 * k, cy + 4.2 * k), (cx, cy + 6.0 * k)]);
                }
                _ => self.line(ink, cx - 6.0 * k, cy + 3.0 * k, cx + 6.0 * k, cy + 3.0 * k, 1.8 * k),
            }
        }
    }

    fn fill_circle(&self, c: Rgba, x: f32, y: f32, r: f32) {
        unsafe {
            self.brush.SetColor(&color(c, 1.0));
            self.dc.FillEllipse(&D2D1_ELLIPSE { point: pt(x, y), radiusX: r, radiusY: r }, &self.brush);
        }
    }

    fn line(&self, c: Rgba, x0: f32, y0: f32, x1: f32, y1: f32, width: f32) {
        unsafe {
            self.brush.SetColor(&color(c, 1.0));
            self.dc.DrawLine(pt(x0, y0), pt(x1, y1), &self.brush, width, None);
        }
    }

    fn paint_sun(&self, x: f32, y: f32, k: f32) {
        self.fill_circle(SUN, x, y, 3.6 * k);
        for i in 0..8 {
            let a = i as f32 * TAU / 8.0;
            let (sin, cos) = a.sin_cos();
            self.line(SUN, x + cos * 5.6 * k, y + sin * 5.6 * k, x + cos * 7.4 * k, y + sin * 7.4 * k, 1.4 * k);
        }
    }

    fn paint_moon(&self, x: f32, y: f32, k: f32, th: &Theme) {
        let moon = if th.light { [0.45, 0.45, 0.55, 1.0] } else { [0.91, 0.90, 0.94, 1.0] };
        self.fill_circle(moon, x, y, 5.5 * k);
        // Bite out of the disc in the island's own color.
        let mut bg = th.background;
        bg[3] = 1.0;
        self.fill_circle(bg, x + 3.0 * k, y - 2.5 * k, 4.6 * k);
    }

    fn paint_cloud(&self, x: f32, y: f32, k: f32, th: &Theme) {
        let mut c = th.text;
        c[3] = 0.9;
        self.fill_circle(c, x - 3.0 * k, y + 1.2 * k, 3.4 * k);
        self.fill_circle(c, x + 1.0 * k, y - 1.2 * k, 4.4 * k);
        self.fill_circle(c, x + 4.4 * k, y + 1.4 * k, 3.1 * k);
        unsafe {
            self.brush.SetColor(&color(c, 1.0));
            let base = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: x - 6.4 * k, top: y + 0.6 * k, right: x + 7.5 * k, bottom: y + 4.6 * k },
                radiusX: 2.0 * k,
                radiusY: 2.0 * k,
            };
            self.dc.FillRoundedRectangle(&base, &self.brush);
        }
    }

    /// Weather glyph centered at (x, y), about 16 DIPs across.
    fn paint_icon(&self, icon: Icon, day: bool, x: f32, y: f32, k: f32, th: &Theme) {
        match icon {
            Icon::Clear if day => self.paint_sun(x, y, k),
            Icon::Clear => self.paint_moon(x, y, k, th),
            Icon::PartlyCloudy => {
                if day {
                    self.paint_sun(x + 3.0 * k, y - 3.0 * k, k * 0.75);
                } else {
                    self.paint_moon(x + 3.0 * k, y - 3.0 * k, k * 0.75, th);
                }
                self.paint_cloud(x - 1.0 * k, y + 1.5 * k, k * 0.85, th);
            }
            Icon::Cloudy => self.paint_cloud(x, y, k, th),
            Icon::Fog => {
                let mut c = th.text;
                c[3] = 0.8;
                for (i, dx) in [(0, 0.0), (1, 2.0), (2, -1.5)] {
                    let yy = y - 4.0 * k + i as f32 * 4.0 * k;
                    self.line(c, x - 6.5 * k + dx * k, yy, x + 6.5 * k + dx * k, yy, 1.6 * k);
                }
            }
            Icon::Drizzle | Icon::Rain => {
                self.paint_cloud(x, y - 2.5 * k, k * 0.9, th);
                let drops: &[f32] = if icon == Icon::Rain { &[-4.0, 0.0, 4.0] } else { &[-2.0, 2.5] };
                for dx in drops {
                    let x0 = x + dx * k;
                    self.line(RAIN, x0, y + 4.0 * k, x0 - 1.5 * k, y + 7.5 * k, 1.4 * k);
                }
            }
            Icon::Snow => {
                self.paint_cloud(x, y - 2.5 * k, k * 0.9, th);
                for (dx, dy) in [(-4.0, 5.0), (0.0, 7.0), (4.0, 5.0)] {
                    self.fill_circle(th.text, x + dx * k, y + dy * k, 1.2 * k);
                }
            }
            Icon::Thunder => {
                self.paint_cloud(x, y - 2.5 * k, k * 0.9, th);
                let pts = [(1.0, 2.5), (-1.5, 6.0), (1.0, 6.0), (-1.0, 9.0)];
                for pair in pts.windows(2) {
                    let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
                    self.line(SUN, x + x0 * k, y + y0 * k, x + x1 * k, y + y1 * k, 1.5 * k);
                }
            }
        }
    }

    /// Renders the Home tiles into an offscreen bitmap once, and again only
    /// when what they show changes (theme, DPI, layout, hovered tile).
    /// Animation frames then blit this one bitmap instead of re-drawing
    /// every tile and glyph, which is what made WARP expensive.
    fn ensure_home_layer(&mut self, scene: &Scene) -> Result<bool> {
        let key = HomeKey {
            recording: scene.recording,
            control: if scene.tab == Tab::Control { scene.control.cloned() } else { None },
            text: if scene.tab == Tab::Text { scene.text.cloned() } else { None },
            shelf: if scene.tab == Tab::Shelf { scene.shelf.to_vec() } else { Vec::new() },
            search: if scene.tab == Tab::Search { scene.search.cloned() } else { None },
            tab: scene.tab,
            notes: if scene.tab == Tab::Notes { scene.notes.cloned() } else { None },
            clip_page: if scene.tab == Tab::Clipboard { scene.clip_page.to_vec() } else { Vec::new() },
            window: scene.window.cloned(),
            clip_rows: scene.clip_rows.to_vec(),
            dock: scene.dock.iter().map(|d| d.id).collect(),
            hot: scene.hot,
            media: scene.media.cloned(),
            scale: scene.scale,
            theme: *scene.theme,
            hovered: scene.hovered,
            tiles: scene.tiles.to_vec(),
            panel: scene.panel_size,
        };
        if self.home_layer.as_ref().is_some_and(|l| l.key == key) {
            return Ok(false);
        }
        let k = scene.scale;
        let size = D2D_SIZE_U {
            width: (scene.panel_size.0 * k).ceil().max(1.0) as u32,
            height: (scene.panel_size.1 * k).ceil().max(1.0) as u32,
        };
        // Reuse the bitmap when only its contents change.
        let old = self.home_layer.take().map(|l| l.bitmap);
        let bitmap = self.layer_bitmap(old, size)?;
        self.paint_layer(&bitmap, |r| r.paint_home(scene))?;
        self.home_layer = Some(HomeLayer { key, bitmap });
        Ok(true)
    }

    /// Paints the Home view with its top-left at (0, 0), fully opaque.
    fn paint_home(&self, scene: &Scene) {
        if scene.tab == Tab::Search {
            if let Some(v) = scene.search {
                self.paint_search(scene, v);
            }
            return;
        }
        self.paint_tabs(scene);
        match scene.tab {
            Tab::Search => {}
            Tab::Home => self.paint_tiles(scene),
            Tab::Clipboard => self.paint_list(scene, scene.clip_page, Hit::Clip, "История пуста — скопируйте что-нибудь"),
            Tab::Shelf => self.paint_list(scene, scene.shelf, Hit::ShelfRow, "Перетащите файлы на остров — они полежат здесь"),
            Tab::Text => {
                if let Some(t) = scene.text {
                    self.paint_text_page(scene, t);
                }
            }
            Tab::Control => {
                if let Some(c) = scene.control {
                    self.paint_control(scene, c);
                }
            }
            Tab::Notes => {
                if let Some(n) = scene.notes {
                    self.paint_notes(scene, n);
                }
            }
        }
    }

    fn paint_tabs(&self, scene: &Scene) {
        let fonts = self.fonts.as_ref().unwrap();
        let (k, th) = (scene.scale, scene.theme);
        let px = |rc: Rect| D2D_RECT_F { left: rc.x * k, top: rc.y * k, right: (rc.x + rc.w) * k, bottom: (rc.y + rc.h) * k };
        for (tab, rc) in layout::tab_buttons(scene.tab) {
            let r = px(rc);
            let active = tab == scene.tab;
            if active || scene.hot == Some(Hit::Tab(tab)) {
                self.fill_rect(if active { th.tile_hover } else { th.tile }, r, 14.0 * k);
            }
            let icon = D2D_RECT_F { right: r.left + 34.0 * k, ..r };
            let (cx, cy) = ((icon.left + icon.right) / 2.0, (icon.top + icon.bottom) / 2.0);
            let ink = if active { th.text } else { th.subtext };
            match tab {
                Tab::Home => {
                    self.fill_polygon(ink, &[(cx - 6.0 * k, cy - 0.5 * k), (cx, cy - 6.0 * k), (cx + 6.0 * k, cy - 0.5 * k)]);
                    self.fill_rect(ink, D2D_RECT_F { left: cx - 4.5 * k, top: cy - 1.0 * k, right: cx + 4.5 * k, bottom: cy + 5.5 * k }, 1.0 * k);
                }
                Tab::Clipboard => {
                    unsafe {
                        self.brush.SetColor(&color(ink, 1.0));
                        let rr = D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F { left: cx - 5.0 * k, top: cy - 5.0 * k, right: cx + 5.0 * k, bottom: cy + 6.5 * k },
                            radiusX: 1.5 * k,
                            radiusY: 1.5 * k,
                        };
                        self.dc.DrawRoundedRectangle(&rr, &self.brush, 1.4 * k, None);
                    }
                    self.fill_rect(ink, D2D_RECT_F { left: cx - 2.5 * k, top: cy - 7.0 * k, right: cx + 2.5 * k, bottom: cy - 4.0 * k }, 1.0 * k);
                }
                Tab::Search => {}
                Tab::Control => {
                    // Two sliders.
                    for (dy, knob) in [(-3.0, 2.5), (3.5, -2.5)] {
                        let y = cy + dy * k;
                        self.line(ink, cx - 6.0 * k, y, cx + 6.0 * k, y, 1.5 * k);
                        self.fill_circle(ink, cx + knob * k, y, 2.2 * k);
                    }
                }
                Tab::Text => {
                    self.line(ink, cx - 5.5 * k, cy - 5.5 * k, cx + 5.5 * k, cy - 5.5 * k, 1.8 * k);
                    self.line(ink, cx, cy - 5.5 * k, cx, cy + 6.0 * k, 1.8 * k);
                }
                Tab::Shelf => {
                    // An open tray.
                    self.line(ink, cx - 6.0 * k, cy - 1.0 * k, cx - 6.0 * k, cy + 5.0 * k, 1.5 * k);
                    self.line(ink, cx - 6.0 * k, cy + 5.0 * k, cx + 6.0 * k, cy + 5.0 * k, 1.5 * k);
                    self.line(ink, cx + 6.0 * k, cy + 5.0 * k, cx + 6.0 * k, cy - 1.0 * k, 1.5 * k);
                    self.line(ink, cx, cy - 6.0 * k, cx, cy + 1.5 * k, 1.5 * k);
                    self.line(ink, cx - 3.0 * k, cy - 1.5 * k, cx, cy + 1.5 * k, 1.5 * k);
                    self.line(ink, cx + 3.0 * k, cy - 1.5 * k, cx, cy + 1.5 * k, 1.5 * k);
                }
                Tab::Notes => {
                    for (i, w) in [9.0, 9.0, 6.0].iter().enumerate() {
                        let y = cy - 4.0 * k + i as f32 * 4.0 * k;
                        self.line(ink, cx - 5.0 * k, y, cx - 5.0 * k + w * k, y, 1.5 * k);
                    }
                }
            }
            if active {
                unsafe {
                    self.brush.SetColor(&color(th.text, 1.0));
                    let label = D2D_RECT_F { left: icon.right - 4.0 * k, ..r };
                    let w: Vec<u16> = tab.title().encode_utf16().collect();
                    self.dc.DrawText(&w, &fonts.tab, &label, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                }
            }
        }
    }

    fn card(&self, rc: Rect, title: &str, k: f32, th: &Theme) {
        let fonts = self.fonts.as_ref().unwrap();
        let r = D2D_RECT_F { left: rc.x * k, top: rc.y * k, right: (rc.x + rc.w) * k, bottom: (rc.y + rc.h) * k };
        self.fill_rect(th.tile, r, 14.0 * k);
        unsafe {
            self.brush.SetColor(&color(th.subtext, 1.0));
            let t = D2D_RECT_F { left: r.left + 12.0 * k, top: r.top + 9.0 * k, right: r.right - 12.0 * k, bottom: r.top + 28.0 * k };
            let w: Vec<u16> = title.encode_utf16().collect();
            self.dc.DrawText(&w, &fonts.title, &t, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    /// An input field: text (or a dim placeholder), selection and caret.
    fn paint_field(&self, field: &TextField, rc: Rect, focused: bool, placeholder: &str, fmt: &IDWriteTextFormat, boxed: bool, k: f32, th: &Theme) {
        let r = D2D_RECT_F { left: rc.x * k, top: rc.y * k, right: (rc.x + rc.w) * k, bottom: (rc.y + rc.h) * k };
        let pad = if boxed { 8.0 * k } else { 0.0 };
        if boxed {
            self.fill_rect(if focused { th.tile_hover } else { th.tile }, r, 8.0 * k);
            if focused {
                unsafe {
                    self.brush.SetColor(&color(th.accent, 0.8));
                    let rr = D2D1_ROUNDED_RECT { rect: r, radiusX: 8.0 * k, radiusY: 8.0 * k };
                    self.dc.DrawRoundedRectangle(&rr, &self.brush, 1.2 * k, None);
                }
            }
        }
        let inner_w = (r.right - r.left - 2.0 * pad).max(1.0);
        let inner_h = (r.bottom - r.top).max(1.0);
        let shown = if field.is_empty() { placeholder } else { &field.text };
        let wide: Vec<u16> = shown.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.dwrite.CreateTextLayout(&wide, fmt, inner_w, inner_h) else { return };
            let origin = pt(r.left + pad, r.top);
            self.dc.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED);
            if focused && !field.is_empty() {
                if let Some((start, len)) = field.selection_utf16() {
                    let mut count = 0u32;
                    let _ = layout.HitTestTextRange(start, len, 0.0, 0.0, None, &mut count);
                    let mut metrics = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
                    if count > 0 && layout.HitTestTextRange(start, len, 0.0, 0.0, Some(&mut metrics), &mut count).is_ok() {
                        let mut sel = th.accent;
                        sel[3] = 0.35;
                        for m in &metrics {
                            self.fill_rect(sel, D2D_RECT_F { left: origin.X + m.left, top: origin.Y + m.top, right: origin.X + m.left + m.width, bottom: origin.Y + m.top + m.height }, 0.0);
                        }
                    }
                }
            }
            self.brush.SetColor(&color(if field.is_empty() { th.subtext } else { th.text }, 1.0));
            self.dc.DrawTextLayout(origin, &layout, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
            if focused {
                let pos = if field.is_empty() { 0 } else { field.caret_utf16() };
                let (mut x, mut y) = (0.0f32, 0.0f32);
                let mut m = DWRITE_HIT_TEST_METRICS::default();
                if layout.HitTestTextPosition(pos, false, &mut x, &mut y, &mut m).is_ok() {
                    let line_h = if m.height > 0.0 { m.height } else { 16.0 * k };
                    self.fill_rect(th.accent, D2D_RECT_F { left: origin.X + x, top: origin.Y + y, right: origin.X + x + 1.5 * k, bottom: origin.Y + y + line_h }, 0.0);
                }
            }
            self.dc.PopAxisAlignedClip();
        }
    }

    fn paint_notes(&self, scene: &Scene, n: &NotesView) {
        let fonts = self.fonts.as_ref().unwrap();
        let (k, th) = (scene.scale, scene.theme);
        let r = layout::notes_regions(scene.panel_size.0);
        let px = |rc: Rect| D2D_RECT_F { left: rc.x * k, top: rc.y * k, right: (rc.x + rc.w) * k, bottom: (rc.y + rc.h) * k };
        let text = |s: &str, fmt: &IDWriteTextFormat, rc: Rect, c: Rgba| unsafe {
            self.brush.SetColor(&color(c, 1.0));
            let w: Vec<u16> = s.encode_utf16().collect();
            self.dc.DrawText(&w, fmt, &px(rc), &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        };

        // Note.
        self.card(r.note_card, "Заметка", k, th);
        self.paint_field(&n.note, r.note, n.note_focus, "Кликните и пишите — сохраняется само", &fonts.note, false, k, th);

        // Timer.
        let title = match n.timer {
            TimerUi::Done => "Таймер · время вышло",
            TimerUi::Paused => "Таймер · пауза",
            _ => "Таймер",
        };
        self.card(r.timer_card, title, k, th);
        let ink = if n.timer == TimerUi::Running { th.accent } else { th.text };
        text(&n.timer_text, &fonts.big, r.timer_text, ink);
        for (hit, rc) in [(Hit::TimerToggle, r.toggle), (Hit::TimerReset, r.reset)] {
            let b = px(rc);
            let fill = if scene.hot == Some(hit) { th.tile_hover } else { th.tile };
            self.fill_rect(fill, b, (b.bottom - b.top) / 2.0);
            let (cx, cy) = ((b.left + b.right) / 2.0, (b.top + b.bottom) / 2.0);
            match hit {
                Hit::TimerToggle => {
                    let glyph = if n.timer == TimerUi::Running { Transport::Pause } else { Transport::Play };
                    self.paint_transport(glyph, b, k * 0.85, th.text);
                }
                _ => self.fill_rect(th.text, D2D_RECT_F { left: cx - 4.5 * k, top: cy - 4.5 * k, right: cx + 4.5 * k, bottom: cy + 4.5 * k }, 1.5 * k),
            }
        }
        for (i, rc) in r.presets.iter().enumerate() {
            let b = px(*rc);
            let on = n.timer_preset == Some(i);
            let fill = if on { th.accent } else if scene.hot == Some(Hit::TimerPreset(i)) { th.tile_hover } else { th.tile_hover };
            let mut fill = fill;
            if !on {
                fill[3] *= 0.7;
            }
            self.fill_rect(fill, b, 8.0 * k);
            let label = crate::timer::length_label(std::time::Duration::from_secs(layout::TIMER_PRESETS[i]));
            text(&label, &fonts.lyric, *rc, if on { [1.0; 4] } else { th.text });
        }

        // Reminders.
        self.card(r.remind_card, "Напоминания", k, th);
        self.paint_field(&n.remind, r.remind, n.remind_focus, "через 15 мин купить хлеб", &fonts.track_artist, true, k, th);
        text(&n.remind_hint, &fonts.small, r.remind_hint, th.subtext);
        if n.reminders.is_empty() {
            let first = r.rows[0];
            text("Пока ничего не запланировано", &fonts.small, Rect { x: first.x + 6.0, ..first }, th.subtext);
        }
        for (i, ((when, what), (row, rm))) in n.reminders.iter().zip(r.rows.iter().zip(&r.removes)).enumerate() {
            if scene.hot == Some(Hit::RemindRemove(i)) {
                self.fill_rect(th.tile_hover, px(*row), 6.0 * k);
            }
            let w = Rect { x: row.x + 6.0, w: 86.0, ..*row };
            text(when, &fonts.small, w, th.accent);
            let t = Rect { x: row.x + 96.0, w: row.w - 96.0 - 26.0, ..*row };
            text(what, &fonts.track_artist, t, th.text);
            let b = px(*rm);
            let (cx, cy) = ((b.left + b.right) / 2.0, (b.top + b.bottom) / 2.0);
            self.line(th.subtext, cx - 3.5 * k, cy - 3.5 * k, cx + 3.5 * k, cy + 3.5 * k, 1.3 * k);
            self.line(th.subtext, cx + 3.5 * k, cy - 3.5 * k, cx - 3.5 * k, cy + 3.5 * k, 1.3 * k);
        }
    }

    fn paint_search(&self, scene: &Scene, v: &SearchView) {
        let fonts = self.fonts.as_ref().unwrap();
        let (k, th) = (scene.scale, scene.theme);
        let w = scene.panel_size.0;
        let px = |rc: Rect| D2D_RECT_F { left: rc.x * k, top: rc.y * k, right: (rc.x + rc.w) * k, bottom: (rc.y + rc.h) * k };
        let field = layout::search_field(w);
        // Magnifier, then the text.
        let f = px(field);
        self.fill_rect(th.tile_hover, f, 12.0 * k);
        let (gx, gy) = (f.left + 18.0 * k, (f.top + f.bottom) / 2.0);
        unsafe {
            self.brush.SetColor(&color(th.subtext, 1.0));
            let e = D2D1_ELLIPSE { point: pt(gx - 1.5 * k, gy - 1.5 * k), radiusX: 5.5 * k, radiusY: 5.5 * k };
            self.dc.DrawEllipse(&e, &self.brush, 1.6 * k, None);
        }
        self.line(th.subtext, gx + 2.5 * k, gy + 2.5 * k, gx + 6.5 * k, gy + 6.5 * k, 1.8 * k);
        let text_rc = Rect { x: field.x + 34.0, y: field.y + 10.0, w: field.w - 120.0, h: 22.0 };
        self.paint_field(&v.field, text_rc, true, "Приложения, файлы, 2+2…", &fonts.track_title, false, k, th);
        unsafe {
            self.brush.SetColor(&color(th.subtext, 1.0));
            let hint = D2D_RECT_F { left: f.right - 90.0 * k, top: f.top, right: f.right - 12.0 * k, bottom: f.bottom };
            let s: Vec<u16> = "Alt+Space".encode_utf16().collect();
            let r = D2D_RECT_F { top: (f.top + f.bottom) / 2.0 - 7.0 * k, bottom: (f.top + f.bottom) / 2.0 + 7.0 * k, ..hint };
            self.dc.DrawText(&s, &fonts.small_right, &r, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }
        let rows = layout::search_rows(w);
        if v.rows.is_empty() {
            unsafe {
                self.brush.SetColor(&color(th.subtext, 1.0));
                let s: Vec<u16> = v.hint.encode_utf16().collect();
                let r = px(Rect { x: rows[0].x, y: rows[0].y, w: rows[0].w, h: 36.0 * 2.0 });
                self.dc.DrawText(&s, &fonts.lyric, &r, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            }
            return;
        }
        for (i, (row, rc)) in v.rows.iter().zip(rows).enumerate() {
            let r = px(rc);
            if i == v.selected {
                self.fill_rect(th.tile_hover, r, 10.0 * k);
            } else if scene.hot == Some(Hit::SearchRow(i)) {
                self.fill_rect(th.tile, r, 10.0 * k);
            }
            let side = 28.0 * k;
            let cy = (r.top + r.bottom) / 2.0;
            let badge = D2D_RECT_F { left: r.left + 6.0 * k, top: cy - side / 2.0, right: r.left + 6.0 * k + side, bottom: cy + side / 2.0 };
            self.paint_badge(&row.badge, badge, k, th);
            unsafe {
                let sub_w = 220.0 * k;
                let title = D2D_RECT_F { left: badge.right + 10.0 * k, top: r.top, right: r.right - sub_w - 8.0 * k, bottom: r.bottom };
                self.brush.SetColor(&color(th.text, 1.0));
                let t: Vec<u16> = row.title.encode_utf16().collect();
                self.dc.DrawText(&t, &fonts.track_title, &title, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                let sub = D2D_RECT_F { left: r.right - sub_w, top: cy - 7.0 * k, right: r.right - 10.0 * k, bottom: cy + 7.0 * k };
                self.brush.SetColor(&color(th.subtext, 1.0));
                let s: Vec<u16> = row.subtitle.encode_utf16().collect();
                self.dc.DrawText(&s, &fonts.small_right, &sub, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            }
        }
    }

    fn paint_tools(&self, tile: &Placed, scene: &Scene, k: f32, th: &Theme) {
        let fonts = self.fonts.as_ref().unwrap();
        for (i, rc) in layout::tool_buttons(tile).into_iter().enumerate() {
            let r = D2D_RECT_F { left: rc.x * k, top: rc.y * k, right: (rc.x + rc.w) * k, bottom: (rc.y + rc.h) * k };
            if scene.hot == Some(Hit::Tool(i)) {
                self.fill_rect(th.tile_hover, r, 10.0 * k);
            }
            let (cx, cy) = ((r.left + r.right) / 2.0, r.top + 16.0 * k);
            let ink = th.text;
            match i {
                0 => {
                    // Area: corner brackets.
                    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                        let (x, y) = (cx + sx * 7.0 * k, cy + sy * 6.0 * k);
                        self.line(ink, x, y, x - sx * 4.0 * k, y, 1.6 * k);
                        self.line(ink, x, y, x, y - sy * 4.0 * k, 1.6 * k);
                    }
                }
                1 => unsafe {
                    self.brush.SetColor(&color(ink, 1.0));
                    let rr = D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: cx - 8.0 * k, top: cy - 6.0 * k, right: cx + 8.0 * k, bottom: cy + 4.0 * k }, radiusX: 1.5 * k, radiusY: 1.5 * k };
                    self.dc.DrawRoundedRectangle(&rr, &self.brush, 1.5 * k, None);
                    self.line(ink, cx - 4.0 * k, cy + 7.0 * k, cx + 4.0 * k, cy + 7.0 * k, 1.5 * k);
                },
                2 => {
                    // Text from screen: T inside brackets.
                    self.line(ink, cx - 4.0 * k, cy - 4.0 * k, cx + 4.0 * k, cy - 4.0 * k, 1.6 * k);
                    self.line(ink, cx, cy - 4.0 * k, cx, cy + 5.0 * k, 1.6 * k);
                    for sx in [-1.0f32, 1.0] {
                        let x = cx + sx * 8.0 * k;
                        self.line(ink, x, cy - 6.0 * k, x, cy + 6.0 * k, 1.4 * k);
                    }
                }
                3 => {
                    for (dx, dy) in [(-4.5, -4.5), (4.5, -4.5), (-4.5, 4.5)] {
                        let (x, y) = (cx + dx * k, cy + dy * k);
                        unsafe {
                            self.brush.SetColor(&color(ink, 1.0));
                            let rr = D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: x - 3.0 * k, top: y - 3.0 * k, right: x + 3.0 * k, bottom: y + 3.0 * k }, radiusX: 0.5 * k, radiusY: 0.5 * k };
                            self.dc.DrawRoundedRectangle(&rr, &self.brush, 1.3 * k, None);
                        }
                    }
                    self.fill_rect(ink, D2D_RECT_F { left: cx + 2.5 * k, top: cy + 2.5 * k, right: cx + 6.5 * k, bottom: cy + 6.5 * k }, 0.5 * k);
                }
                4 => {
                    // Dropper.
                    self.line(ink, cx - 6.0 * k, cy + 6.0 * k, cx + 2.0 * k, cy - 2.0 * k, 2.0 * k);
                    self.fill_circle(ink, cx + 4.0 * k, cy - 4.0 * k, 3.2 * k);
                }
                5 => {
                    self.line(ink, cx - 5.0 * k, cy - 5.0 * k, cx + 5.0 * k, cy - 5.0 * k, 1.6 * k);
                    self.line(ink, cx, cy - 5.0 * k, cx, cy + 6.0 * k, 1.6 * k);
                    self.line(ink, cx - 7.0 * k, cy + 6.0 * k, cx + 7.0 * k, cy - 7.0 * k, 1.3 * k);
                }
                layout::TOOL_RECORD => {
                    // Record: a red dot in a ring; a red square while recording.
                    let red = [1.0, 0.231, 0.188, 1.0];
                    if scene.recording {
                        self.fill_rect(red, D2D_RECT_F { left: cx - 5.0 * k, top: cy - 5.0 * k, right: cx + 5.0 * k, bottom: cy + 5.0 * k }, 1.5 * k);
                    } else {
                        unsafe {
                            self.brush.SetColor(&color(ink, 1.0));
                            let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: 7.0 * k, radiusY: 7.0 * k };
                            self.dc.DrawEllipse(&e, &self.brush, 1.5 * k, None);
                        }
                        self.fill_circle(red, cx, cy, 4.0 * k);
                    }
                }
                _ => {
                    unsafe {
                        self.brush.SetColor(&color(ink, 1.0));
                        let e = D2D1_ELLIPSE { point: pt(cx - 1.5 * k, cy - 1.5 * k), radiusX: 5.0 * k, radiusY: 5.0 * k };
                        self.dc.DrawEllipse(&e, &self.brush, 1.5 * k, None);
                    }
                    self.line(ink, cx + 2.0 * k, cy + 2.0 * k, cx + 6.0 * k, cy + 6.0 * k, 1.7 * k);
                }
            }
            unsafe {
                self.brush.SetColor(&color(th.subtext, 1.0));
                let label = D2D_RECT_F { left: r.left, top: r.top + 30.0 * k, right: r.right, bottom: r.bottom - 2.0 * k };
                let name = if i == layout::TOOL_RECORD && scene.recording { "Стоп" } else { layout::TOOLS[i] };
                let s: Vec<u16> = name.encode_utf16().collect();
                self.dc.DrawText(&s, &fonts.small_center, &label, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            }
        }
    }

    fn paint_ctl_icon(&self, icon: CtlIcon, cx: f32, cy: f32, k: f32, th: &Theme) {
        let ink = th.text;
        match icon {
            CtlIcon::Wifi(bars, secured) => {
                for i in 0..4u32 {
                    let h = (3.0 + i as f32 * 3.0) * k;
                    let x = cx - 7.0 * k + i as f32 * 4.0 * k;
                    let mut c = ink;
                    if i >= bars {
                        c[3] = 0.25;
                    }
                    self.fill_rect(c, D2D_RECT_F { left: x, top: cy + 6.0 * k - h, right: x + 2.6 * k, bottom: cy + 6.0 * k }, 0.6 * k);
                }
                if secured {
                    let (lx, ly) = (cx + 8.0 * k, cy + 3.0 * k);
                    self.fill_rect(th.subtext, D2D_RECT_F { left: lx - 2.5 * k, top: ly - 1.0 * k, right: lx + 2.5 * k, bottom: ly + 3.0 * k }, 0.6 * k);
                    self.line(th.subtext, lx - 1.5 * k, ly - 1.0 * k, lx - 1.5 * k, ly - 3.0 * k, 1.0 * k);
                    self.line(th.subtext, lx + 1.5 * k, ly - 1.0 * k, lx + 1.5 * k, ly - 3.0 * k, 1.0 * k);
                    self.line(th.subtext, lx - 1.5 * k, ly - 3.0 * k, lx + 1.5 * k, ly - 3.0 * k, 1.0 * k);
                }
            }
            CtlIcon::Headphones => {
                unsafe {
                    self.brush.SetColor(&color(ink, 1.0));
                    let arc = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: 6.0 * k, radiusY: 6.0 * k };
                    self.dc.PushAxisAlignedClip(&D2D_RECT_F { left: cx - 8.0 * k, top: cy - 8.0 * k, right: cx + 8.0 * k, bottom: cy + 1.0 * k }, D2D1_ANTIALIAS_MODE_ALIASED);
                    self.dc.DrawEllipse(&arc, &self.brush, 1.6 * k, None);
                    self.dc.PopAxisAlignedClip();
                }
                for sx in [-1.0f32, 1.0] {
                    self.fill_rect(ink, D2D_RECT_F { left: cx + sx * 6.0 * k - 1.8 * k, top: cy - 1.0 * k, right: cx + sx * 6.0 * k + 1.8 * k, bottom: cy + 5.5 * k }, 1.0 * k);
                }
            }
            CtlIcon::Input => unsafe {
                self.brush.SetColor(&color(ink, 1.0));
                let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: 4.5 * k, radiusY: 6.5 * k };
                self.dc.DrawEllipse(&e, &self.brush, 1.5 * k, None);
                self.line(ink, cx, cy - 6.0 * k, cx, cy - 2.0 * k, 1.3 * k);
            },
            CtlIcon::Phone => unsafe {
                self.brush.SetColor(&color(ink, 1.0));
                let rr = D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: cx - 4.0 * k, top: cy - 7.0 * k, right: cx + 4.0 * k, bottom: cy + 7.0 * k }, radiusX: 1.8 * k, radiusY: 1.8 * k };
                self.dc.DrawRoundedRectangle(&rr, &self.brush, 1.4 * k, None);
            },
            CtlIcon::Device => self.fill_circle(ink, cx, cy, 3.0 * k),
            CtlIcon::Vpn => {
                self.fill_polygon(ink, &[(cx, cy - 7.0 * k), (cx + 6.0 * k, cy - 4.5 * k), (cx + 5.0 * k, cy + 2.5 * k), (cx, cy + 7.0 * k), (cx - 5.0 * k, cy + 2.5 * k), (cx - 6.0 * k, cy - 4.5 * k)]);
            }
        }
    }

    fn paint_control(&self, scene: &Scene, c: &ControlView) {
        let fonts = self.fonts.as_ref().unwrap();
        let (k, th) = (scene.scale, scene.theme);
        let r = layout::control_regions(scene.panel_size.0);
        let px = |rc: Rect| D2D_RECT_F { left: rc.x * k, top: rc.y * k, right: (rc.x + rc.w) * k, bottom: (rc.y + rc.h) * k };
        let text = |s: &str, fmt: &IDWriteTextFormat, rc: Rect, col: Rgba| unsafe {
            self.brush.SetColor(&color(col, 1.0));
            let w: Vec<u16> = s.encode_utf16().collect();
            self.dc.DrawText(&w, fmt, &px(rc), &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        };
        for (i, (seg, label)) in r.segments.iter().zip(["Wi‑Fi", "Bluetooth", "VPN"]).enumerate() {
            let on = i == c.segment;
            let fill = if on { th.tile_hover } else if scene.hot == Some(Hit::CtlSegment(i)) { th.tile } else { [0.0; 4] };
            self.fill_rect(fill, px(*seg), 14.0 * k);
            text(label, &fonts.lyric, *seg, if on { th.text } else { th.subtext });
        }
        // Refresh: a circular arrow.
        let rb = px(r.refresh);
        if scene.hot == Some(Hit::CtlRefresh) {
            self.fill_rect(th.tile, rb, 14.0 * k);
        }
        let (cx, cy) = ((rb.left + rb.right) / 2.0, (rb.top + rb.bottom) / 2.0);
        unsafe {
            self.brush.SetColor(&color(th.subtext, 1.0));
            self.dc.PushAxisAlignedClip(&D2D_RECT_F { left: cx - 8.0 * k, top: cy - 8.0 * k, right: cx + 8.0 * k, bottom: cy + 8.0 * k }, D2D1_ANTIALIAS_MODE_ALIASED);
            let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: 5.5 * k, radiusY: 5.5 * k };
            self.dc.DrawEllipse(&e, &self.brush, 1.5 * k, None);
            self.dc.PopAxisAlignedClip();
        }
        self.fill_polygon(th.subtext, &[(cx + 5.5 * k, cy - 4.0 * k), (cx + 8.0 * k, cy + 0.5 * k), (cx + 3.0 * k, cy + 0.5 * k)]);
        let sb = px(r.settings);
        self.fill_rect(if scene.hot == Some(Hit::CtlSettings) { th.tile_hover } else { th.tile }, sb, 14.0 * k);
        text("Параметры", &fonts.lyric, r.settings, th.subtext);

        if c.rows.is_empty() {
            text(&c.empty, &fonts.lyric, Rect { h: 30.0 * 3.0, ..r.rows[0] }, th.subtext);
        }
        for (i, (row, rc)) in c.rows.iter().zip(&r.rows).enumerate() {
            let b = px(*rc);
            self.fill_rect(if row.active { th.tile_hover } else { th.tile }, b, 10.0 * k);
            self.paint_ctl_icon(row.icon, b.left + 18.0 * k, (b.top + b.bottom) / 2.0, k, th);
            let title = Rect { x: rc.x + 36.0, w: rc.w * 0.45 - 40.0, ..*rc };
            text(&row.title, &fonts.track_artist, title, th.text);
            match &c.password {
                Some((pr, field, focus)) if *pr == i => {
                    let (f, go) = layout::password_parts(rc);
                    self.paint_field(field, f, *focus, "Пароль", &fonts.track_artist, true, k, th);
                    self.fill_rect(th.accent, px(go), 8.0 * k);
                    text("Войти", &fonts.lyric, go, [1.0; 4]);
                }
                _ => {
                    let sub = Rect { x: rc.x + rc.w * 0.45, w: rc.w * 0.55 - 112.0, ..*rc };
                    text(&row.sub, &fonts.small, Rect { y: rc.y + 7.0, h: 14.0, ..sub }, th.subtext);
                    if let Some(a) = &row.action {
                        let ab = r.actions[i];
                        let fill = if scene.hot == Some(Hit::CtlAction(i)) { th.accent } else { th.tile_hover };
                        self.fill_rect(fill, px(ab), 8.0 * k);
                        text(a, &fonts.lyric, ab, th.text);
                    }
                }
            }
        }
        let mut msg = th.accent;
        if c.busy {
            msg = th.subtext;
        }
        text(&c.message, &fonts.small, r.message, msg);

        for (i, (p, rc)) in crate::control::POWER.iter().zip(&r.power).enumerate() {
            let b = px(*rc);
            let asking = c.confirm == Some(i);
            let fill = if asking { [0.86, 0.24, 0.24, 1.0] } else if scene.hot == Some(Hit::Power(i)) { th.tile_hover } else { th.tile };
            self.fill_rect(fill, b, 12.0 * k);
            let (cx, cy) = ((b.left + b.right) / 2.0, b.top + 22.0 * k);
            let ink = if asking { [1.0; 4] } else { th.text };
            match p {
                crate::control::Power::Lock => {
                    self.fill_rect(ink, D2D_RECT_F { left: cx - 5.0 * k, top: cy - 1.0 * k, right: cx + 5.0 * k, bottom: cy + 6.0 * k }, 1.0 * k);
                    unsafe {
                        self.brush.SetColor(&color(ink, 1.0));
                        let rr = D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: cx - 3.2 * k, top: cy - 7.0 * k, right: cx + 3.2 * k, bottom: cy + 1.0 * k }, radiusX: 3.0 * k, radiusY: 3.0 * k };
                        self.dc.DrawRoundedRectangle(&rr, &self.brush, 1.5 * k, None);
                    }
                }
                crate::control::Power::Sleep => {
                    self.fill_circle(ink, cx, cy, 6.0 * k);
                    self.fill_circle(fill, cx + 3.0 * k, cy - 2.5 * k, 5.0 * k);
                }
                crate::control::Power::Restart => unsafe {
                    self.brush.SetColor(&color(ink, 1.0));
                    let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: 6.0 * k, radiusY: 6.0 * k };
                    self.dc.DrawEllipse(&e, &self.brush, 1.6 * k, None);
                    self.fill_polygon(fill, &[(cx + 2.0 * k, cy - 8.0 * k), (cx + 8.0 * k, cy - 8.0 * k), (cx + 8.0 * k, cy - 1.0 * k)]);
                    self.fill_polygon(ink, &[(cx + 3.0 * k, cy - 7.5 * k), (cx + 7.5 * k, cy - 5.0 * k), (cx + 3.0 * k, cy - 2.5 * k)]);
                },
                crate::control::Power::Shutdown => unsafe {
                    self.brush.SetColor(&color(ink, 1.0));
                    self.dc.PushAxisAlignedClip(&D2D_RECT_F { left: cx - 8.0 * k, top: cy - 3.0 * k, right: cx + 8.0 * k, bottom: cy + 8.0 * k }, D2D1_ANTIALIAS_MODE_ALIASED);
                    let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: 6.0 * k, radiusY: 6.0 * k };
                    self.dc.DrawEllipse(&e, &self.brush, 1.6 * k, None);
                    self.dc.PopAxisAlignedClip();
                    self.line(ink, cx, cy - 7.5 * k, cx, cy - 0.5 * k, 1.6 * k);
                },
            }
            let label = if asking { "Точно?" } else { p.label() };
            text(label, &fonts.small_center, Rect { y: rc.y + 38.0, h: 16.0, ..*rc }, ink);
        }
    }

    fn paint_text_page(&self, scene: &Scene, t: &TextView) {
        let fonts = self.fonts.as_ref().unwrap();
        let (k, th) = (scene.scale, scene.theme);
        let r = layout::text_regions(scene.panel_size.0);
        let px = |rc: Rect| D2D_RECT_F { left: rc.x * k, top: rc.y * k, right: (rc.x + rc.w) * k, bottom: (rc.y + rc.h) * k };
        self.fill_rect(th.tile, px(r.card), 14.0 * k);
        self.paint_field(&t.field, r.field, t.focus, "Вставьте текст или распознайте его с экрана", &fonts.note, false, k, th);
        let text = |s: &str, fmt: &IDWriteTextFormat, rc: Rect, c: Rgba| unsafe {
            self.brush.SetColor(&color(c, 1.0));
            let w: Vec<u16> = s.encode_utf16().collect();
            self.dc.DrawText(&w, fmt, &px(rc), &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        };
        text(&t.stats, &fonts.small, r.stats, th.subtext);
        text(&t.status, &fonts.small, r.status, th.accent);
        let copy = px(r.copy);
        self.fill_rect(if scene.hot == Some(Hit::TextCopy) { th.tile_hover } else { th.tile_hover }, copy, 8.0 * k);
        text("Копировать", &fonts.lyric, r.copy, th.text);
        for (i, rc) in r.ops.iter().enumerate() {
            let b = px(*rc);
            let fill = if scene.hot == Some(Hit::TextOp(i)) { th.tile_hover } else { th.tile };
            self.fill_rect(fill, b, 8.0 * k);
            text(crate::text_tools::OPS[i].label(), &fonts.lyric, *rc, th.text);
        }
        let tr = px(r.translate);
        let mut fill = th.accent;
        if t.busy {
            fill[3] = 0.5;
        }
        self.fill_rect(fill, tr, 10.0 * k);
        text(if t.busy { "Перевожу…" } else { "Перевести" }, &fonts.lyric, r.translate, [1.0; 4]);
    }

    /// A page of rows (Clipboard tab, Shelf tab).
    fn paint_list(&self, scene: &Scene, list: &[ClipRow], hit: fn(usize) -> Hit, empty: &str) {
        let fonts = self.fonts.as_ref().unwrap();
        let (k, th) = (scene.scale, scene.theme);
        let w = scene.panel_size.0;
        if list.is_empty() {
            let area = Rect { x: layout::PAD, y: layout::TAB_BAR_H, w: w - 2.0 * layout::PAD, h: layout::PAGE_H - layout::PAD };
            unsafe {
                self.brush.SetColor(&color(th.subtext, 1.0));
                let s: Vec<u16> = empty.encode_utf16().collect();
                let r = D2D_RECT_F { left: area.x * k, top: area.y * k, right: (area.x + area.w) * k, bottom: (area.y + area.h) * k };
                self.dc.DrawText(&s, &fonts.lyric, &r, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            }
            return;
        }
        for (i, (row, slot)) in list.iter().zip(layout::clip_page_rows()).enumerate() {
            let rc = Rect { w: w - 2.0 * layout::PAD, ..slot };
            let r = D2D_RECT_F { left: rc.x * k, top: rc.y * k, right: (rc.x + rc.w) * k, bottom: (rc.y + rc.h) * k };
            self.fill_rect(if scene.hot == Some(hit(i)) { th.tile_hover } else { th.tile }, r, 10.0 * k);
            let cy = (r.top + r.bottom) / 2.0;
            self.paint_glyph(row.glyph, r.left + 18.0 * k, cy, k * 0.9, th);
            unsafe {
                let ago = D2D_RECT_F { left: r.right - 70.0 * k, right: r.right - 12.0 * k, top: cy - 7.0 * k, bottom: cy + 7.0 * k };
                self.brush.SetColor(&color(th.subtext, 1.0));
                let a: Vec<u16> = row.ago.encode_utf16().collect();
                self.dc.DrawText(&a, &fonts.small_right, &ago, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                let t = D2D_RECT_F { left: r.left + 36.0 * k, right: ago.left - 6.0 * k, ..r };
                self.brush.SetColor(&color(th.text, 1.0));
                let s: Vec<u16> = row.text.encode_utf16().collect();
                self.dc.DrawText(&s, &fonts.track_artist, &t, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            }
        }
    }

    fn paint_tiles(&self, scene: &Scene) {
        let fonts = self.fonts.as_ref().unwrap();
        let k = scene.scale;
        let th = scene.theme;
        unsafe {
            if scene.tiles.is_empty() {
                let rect = D2D_RECT_F { left: 0.0, top: 0.0, right: scene.panel_size.0 * k, bottom: scene.panel_size.1 * k };
                self.brush.SetColor(&color(th.subtext, 1.0));
                let text: Vec<u16> = "Главная пуста — добавьте плитки в настройках".encode_utf16().collect();
                self.dc.DrawText(&text, &fonts.hint, &rect, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                return;
            }
            for (i, tile) in scene.tiles.iter().enumerate() {
                let rect = D2D_RECT_F {
                    left: tile.x * k,
                    top: tile.y * k,
                    right: (tile.x + tile.w) * k,
                    bottom: (tile.y + tile.h) * k,
                };
                let fill = if scene.hovered == Some(i) { th.tile_hover } else { th.tile };
                self.brush.SetColor(&color(fill, 1.0));
                let rr = D2D1_ROUNDED_RECT { rect, radiusX: 14.0 * k, radiusY: 14.0 * k };
                self.dc.FillRoundedRectangle(&rr, &self.brush);

                if tile.kind == TileKind::Media {
                    if let Some(media) = scene.media {
                        self.paint_player(tile, media, scene.hot, k, th);
                        continue;
                    }
                }
                if tile.kind == TileKind::Window {
                    self.paint_window_tile(tile, scene, k, th);
                    continue;
                }
                if tile.kind == TileKind::Tools {
                    self.paint_tools(tile, scene, k, th);
                    continue;
                }
                if tile.kind == TileKind::Apps {
                    self.paint_dock(tile, scene, k, th);
                    continue;
                }
                if tile.kind == TileKind::Clipboard {
                    self.paint_clipboard(tile, scene, k, th);
                }

                let title_rect = D2D_RECT_F {
                    left: rect.left + 12.0 * k,
                    top: rect.top + 9.0 * k,
                    right: rect.right - 12.0 * k,
                    bottom: rect.top + 28.0 * k,
                };
                self.brush.SetColor(&color(th.subtext, 1.0));
                let text: Vec<u16> = tile.kind.title().encode_utf16().collect();
                self.dc.DrawText(&text, &fonts.title, &title_rect, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);

                // Accent marker so the theme's accent is visible until real
                // tile content arrives in later stages.
                self.brush.SetColor(&color(th.accent, 1.0));
                let dot = D2D1_ELLIPSE { point: pt(rect.right - 16.0 * k, rect.top + 18.0 * k), radiusX: 3.0 * k, radiusY: 3.0 * k };
                self.dc.FillEllipse(&dot, &self.brush);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_pacing_targets_60fps() {
        assert_eq!(sync_interval_for(60), 1);
        assert_eq!(sync_interval_for(75), 1);
        assert_eq!(sync_interval_for(120), 2);
        assert_eq!(sync_interval_for(144), 2);
        assert_eq!(sync_interval_for(240), 4);
        assert_eq!(sync_interval_for(30), 1);
    }

    #[test]
    fn wave_is_pinned_at_corners() {
        for phase in [0.0, 1.0, 2.5] {
            assert!(wave(0.0, 10.0, phase).abs() < 1e-4);
            assert!(wave(1.0, 10.0, phase).abs() < 1e-4);
        }
    }

    #[test]
    fn wave_never_exceeds_amplitude() {
        for i in 0..=100 {
            assert!(wave(i as f32 / 100.0, 6.0, 0.7).abs() <= 6.0 + 1e-4);
        }
    }
}
