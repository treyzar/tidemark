//! Renderer: D3D11 device -> DXGI composition swap chain -> DirectComposition
//! visual on the island window, drawn with Direct2D/DirectWrite. Nothing is
//! drawn unless the island changes, so an idle island costs no CPU time.

use crate::clip_history::Glyph;
use crate::layout::{self, Hit, Placed, Rect, TileKind};
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
            self.fonts = Some(Fonts {
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
                let top = D2D_RECT_F { left: text_l, top: 3.0 * k, right: w - pad, bottom: h / 2.0 + 1.0 * k };
                let bottom = D2D_RECT_F { left: text_l, top: h / 2.0 - 1.0 * k, right: w - pad, bottom: h - 3.0 * k };
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
                self.brush.SetColor(&color(th.text, 1.0));
                let time: Vec<u16> = scene.compact.time.encode_utf16().collect();
                self.dc.DrawText(&time, &fonts.compact_right, &clock, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                let text_l = cover.right + 9.0 * k;
                let text_r = w - pad - 48.0 * k;
                let t = D2D_RECT_F { left: text_l, top: 3.0 * k, right: text_r, bottom: h / 2.0 + 1.0 * k };
                let a = D2D_RECT_F { left: text_l, top: h / 2.0 - 1.0 * k, right: text_r, bottom: h - 3.0 * k };
                let title: Vec<u16> = title.encode_utf16().collect();
                self.dc.DrawText(&title, &fonts.pill_title, &t, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                self.brush.SetColor(&color(if m.is_lyric { th.text } else { th.subtext }, 1.0));
                let artist: Vec<u16> = artist.encode_utf16().collect();
                self.dc.DrawText(&artist, &fonts.pill_artist, &a, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                return;
            }
            self.brush.SetColor(&color(th.text, 1.0));
            let time: Vec<u16> = scene.compact.time.encode_utf16().collect();
            self.dc.DrawText(&time, &fonts.compact, &full, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            if let Some((icon, day, temp)) = &scene.compact.weather {
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
            Badge::Camera => {
                self.fill_rect(ink, D2D_RECT_F { left: cx - 6.5 * k, top: cy - 4.0 * k, right: cx + 2.5 * k, bottom: cy + 4.0 * k }, 1.5 * k);
                self.fill_polygon(ink, &[(cx + 3.0 * k, cy - 1.0 * k), (cx + 7.0 * k, cy - 4.0 * k), (cx + 7.0 * k, cy + 4.0 * k), (cx + 3.0 * k, cy + 1.0 * k)]);
            }
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
