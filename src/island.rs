//! The island window: placement, hover handling, animation state and the
//! window procedure.
//!
//! The window is sized for the largest island state and stays put; the
//! visible shape animates inside it. A window region matching the shape's
//! bounds keeps clicks outside the island going to the windows underneath.

use crate::active_app::{self, ActiveApp};
use crate::agent;
use crate::anim::Spring;
use crate::privacy::{self, InUse, WM_PRIVACY};
use crate::signals::{self, Signals, WM_VOLUME};
use crate::beat::{self, WM_BEAT};
use crate::config::Config;
use crate::clip_history::{self, Entry, History};
use crate::layout::{self, Hit, Placed, Rect, Tab, TileKind};
use crate::launcher::{self, WM_INDEX};
use crate::notes::{self, Reminder};
use crate::textinput::TextField;
use crate::timer::{self, Timer};
use crate::fullscreen::{self, ABN_FULLSCREENAPP, WM_APPBAR, WM_FOREGROUND};
use crate::overlay::{self, WM_OVERLAY_DONE};
use crate::recognize::{self, WM_OCR};
use crate::shelf::{self, WM_SHELF_DRAG, WM_SHELF_DROP};
use crate::translate::{self, WM_TRANSLATED};
use crate::control::{self, WM_CONTROL};
use crate::{capture, text_tools};
use std::sync::{Arc, Mutex};
use crate::{clipboard, dock};
use crate::media::{self, Command, MediaState, WM_MEDIA};
use crate::lyrics::{self, Lines, WM_LYRICS};
use crate::render::{Badge, ClipRow, Compact, CompactMedia, DockIcon, MediaView, NotesView, Renderer, Scene, SearchRow, SearchView, Shape, TextView, TimerUi, Toast, WindowView};
use crate::render::{ControlView, CtlIcon, CtlRow};
use crate::stats::Stats;
use crate::theme::Theme;
use crate::tray::{self, TrayAction};
use crate::weather::{self, WM_WEATHER};
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Extra room around the expanded island for the liquid edge, in DIPs.
const MARGIN: f32 = 16.0;
const STIFFNESS: f32 = 320.0;
const DAMPING: f32 = 26.0;
/// Bouncy, slowly decaying spring for the liquid edge.
const WOBBLE_STIFFNESS: f32 = 140.0;
const WOBBLE_DAMPING: f32 = 7.0;
/// Wave travel speed, radians per second.
const WAVE_SPEED: f32 = 7.0;
/// Delay before collapsing after the cursor leaves, so brushing past the
/// island does not make it flicker.
const COLLAPSE_DELAY_MS: u32 = 300;

const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_COLLAPSE: usize = 1;
const TIMER_CLOCK: usize = 2;
/// Ticks the progress bar once a second while the player is visible.
const TIMER_MEDIA: usize = 3;
/// One-shot, armed for the moment the next lyric line starts.
const TIMER_LYRIC: usize = 4;
/// Renderer memory is trimmed only after this long without drawing: trimming
/// after every redraw made WARP recompile its shaders on each lyric line.
const TIMER_TRIM: usize = 5;
const TRIM_AFTER_IDLE_MS: u32 = 10_000;
/// Clears the "Скопировано" notice from the pill.
const TIMER_TOAST: usize = 6;
const TOAST_MS: u32 = 2200;
const APP_TOAST_MS: u32 = 1300;
const LEVEL_TOAST_MS: u32 = 1500;
const KEY_TOAST_MS: u32 = 900;
const PRIVACY_TOAST_MS: u32 = 2500;
/// Waits for the foreground window to settle before announcing it.
const TIMER_APP: usize = 10;
const APP_SETTLE_MS: u32 = 250;
const PBT_POWERSETTINGCHANGE: u32 = 0x8013;
/// One layout check shortly after a layout chord is released.
const TIMER_LAYOUT: usize = 11;
const LAYOUT_CHECK_MS: u32 = 120;
/// Ticks the countdown once a second while a timer runs.
const TIMER_COUNTDOWN: usize = 12;
/// One-shot at the moment the timer ends.
const TIMER_TIMER_END: usize = 13;
/// One-shot at the next reminder (re-armed at most an hour ahead).
const TIMER_REMINDER: usize = 14;
/// Bounces the island while an alarm waits to be dismissed.
const TIMER_ALARM_PULSE: usize = 15;
const ALARM_PULSE_MS: u32 = 1100;
const ALARM_PULSES: u32 = 20;
/// Saves the note shortly after typing stops.
const TIMER_NOTE_SAVE: usize = 16;
/// Home tiles start below the tab row.
/// Clears a power button waiting for its second click.
const TIMER_CONFIRM: usize = 17;
const HOME_OFFSET: f32 = layout::TAB_BAR_H - layout::PAD;
const WM_CLIPBOARDUPDATE: u32 = 0x031D;
const MENU_REMOVE: usize = 1;
const MENU_CLEAR: usize = 2;
/// While a full-screen app hides the island: watch for the cursor at the
/// top edge. 10 checks a second, only in that state.
const TIMER_EDGE: usize = 7;
const EDGE_POLL_MS: u32 = 100;
/// A revealed island nobody hovers goes back into hiding after this.
const REVEAL_GRACE: Duration = Duration::from_millis(1200);
/// Hover must last this long before the island expands (config).
const TIMER_DWELL: usize = 8;
/// Re-checks whether a maximized window sits under the island. Maximizing
/// the active window raises no cheap event, so this looks every 0.5 s: one
/// GetForegroundWindow + GetWindowRect.
const TIMER_DOCK: usize = 9;
const DOCK_POLL_MS: u32 = 500;
/// The thin handle the island becomes over maximized windows, in DIPs.
const HANDLE_W: f32 = 100.0;
const HANDLE_H: f32 = 6.0;
/// Volume changes are sent at most this often while dragging.
const VOLUME_THROTTLE: Duration = Duration::from_millis(40);
const HOTKEY_THEME: i32 = 1;
const HOTKEY_LAUNCHER: i32 = 2;

pub struct Island {
    hwnd: HWND,
    cfg: Config,
    theme: Theme,
    tiles: Vec<Placed>,
    panel_height: f32,
    renderer: Option<Renderer>,
    width: Spring,
    height: Spring,
    radius: Spring,
    wobble: Spring,
    phase: f32,
    scale: f32,
    win_size: (i32, i32),
    region: Option<RECT>,
    hovered: bool,
    hovered_tile: Option<usize>,
    dirty: bool,
    last_frame: Option<Instant>,
    stats: Stats,
    compact: Compact,
    weather: Option<weather::Service>,
    media_service: Option<media::Service>,
    media: Option<MediaState>,
    media_view: Option<MediaView>,
    dragging_volume: bool,
    last_volume_sent: Option<Instant>,
    lyrics_service: Option<lyrics::Service>,
    /// Track key the lyrics were requested for, and the lines once known.
    lyrics_key: Option<u64>,
    lyrics: Option<Lines>,
    beat: Option<beat::Capture>,
    history: History,
    clip_rows: Vec<ClipRow>,
    /// Our own clipboard write is about to come back as an update.
    ignore_next_clip: bool,
    dock_apps: Vec<dock::App>,
    dock_icons: Vec<DockIcon>,
    /// Clickable thing under the cursor.
    hot: Option<Hit>,
    fs_watcher: Option<fullscreen::Watcher>,
    /// A full-screen app covers the island's monitor.
    fullscreen: bool,
    /// 0 = in place, 100 = slid up out of view (percent, so the springs'
    /// settle threshold is fine-grained enough).
    hide: Spring,
    /// The window itself is hidden (after the slide-out finished).
    window_hidden: bool,
    /// Last time the cursor was at the top edge or on the island.
    last_near: Instant,
    /// A maximized (or top-snapped) window sits under the island: shrink to
    /// a thin handle so its tabs and title bar stay usable.
    docked: bool,
    /// Gap between the screen edge and the island, in DIPs: the config's
    /// offset normally, 0 while docked so the island stays reachable from
    /// the very edge when it opens.
    top_gap: Spring,
    signals: Option<Signals>,
    privacy: Option<privacy::Watcher>,
    in_use: InUse,
    /// The window the user works in (never the island itself).
    active: Option<ActiveApp>,
    window_view: Option<WindowView>,
    /// Icons of apps seen in notices / the window tile, by exe path.
    app_icons: Vec<(String, DockIcon)>,
    extra_icons: Vec<DockIcon>,
    /// (foreground window, its keyboard layout) at the last check.
    layout: (isize, isize),
    tab: Tab,
    /// Content height of the Home tab (tiles + tab row).
    home_height: f32,
    store: notes::Store,
    note: TextField,
    remind: TextField,
    focus: Focus,
    timer: Timer,
    notes_view: Option<NotesView>,
    clip_page: Vec<ClipRow>,
    /// A timer or reminder alarm waits in the pill until dismissed.
    alarm: Option<Alarm>,
    alarm_pulses: u32,
    /// When the current alarm appeared: clicks right after a new agent card
    /// (meant for the previous one) are ignored.
    alarm_since: Instant,
    /// High surrogate of a character arriving in two WM_CHARs.
    pending_surrogate: Option<u16>,
    /// The user's window before a field took the keyboard; it gets the
    /// keyboard back when typing ends with Esc or Enter.
    prev_foreground: isize,
    index: launcher::Index,
    search: TextField,
    search_hits: Vec<SearchHit>,
    search_sel: usize,
    search_view: Option<SearchView>,
    /// Tab to go back to when the search closes.
    tab_before_search: Tab,
    shelf: Vec<String>,
    shelf_rows: Vec<ClipRow>,
    shelf_target: Option<shelf::Target>,
    /// Shelf row pressed at (x, y): becomes a drag-out once the cursor moves.
    shelf_press: Option<(usize, i32, i32)>,
    /// The Text tab's contents.
    text: TextField,
    text_status: String,
    text_view: Option<TextView>,
    ocr_slot: Arc<Mutex<Option<String>>>,
    translate_slot: Arc<Mutex<Option<Option<String>>>>,
    translating: bool,
    /// The next clipboard change is ours and already announced: record it
    /// in the history without the generic "Скопировано" notice.
    quiet_next_clip: bool,
    control: Option<control::Service>,
    ctl: control::State,
    ctl_segment: usize,
    /// Wi-Fi network asking for a password, and the password typed so far.
    ctl_pw_ssid: Option<String>,
    ctl_pw: TextField,
    power_confirm: Option<usize>,
    ctl_view: Option<ControlView>,
    agent_inbox: Option<agent::Inbox>,
    /// Agent cards waiting for an answer, oldest first; the first one is
    /// shown while no other alarm is up.
    agent_cards: Vec<AgentCard>,
    /// Drawn width of the shown agent card's longer line, DIPs.
    agent_text_w: f32,
}

struct AgentCard {
    id: u64,
    msg: agent::Message,
    reply: std::sync::mpsc::Sender<Option<usize>>,
}

/// Coding agents' notices and cards.
const WM_AGENT: u32 = WM_APP + 18;
const AGENT_TOAST_MS: u32 = 6000;
const AGENT_CLICK_GUARD: Duration = Duration::from_millis(700);

#[derive(Debug, Clone)]
enum SearchHit {
    Item(launcher::Item),
    /// Calculator result, ready to copy.
    Calc(String),
    /// Search the web for the query.
    Web(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    None,
    Note,
    Remind,
    Search,
    Text,
    Password,
}

#[derive(Debug, Clone, PartialEq)]
enum Alarm {
    Timer,
    Reminder(String),
    /// A coding agent's card (`agent_cards` id).
    Agent(u64),
}

thread_local! {
    static ISLAND: RefCell<Option<Island>> = const { RefCell::new(None) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    static TRACKING_LEAVE: Cell<bool> = const { Cell::new(false) };
    static DWELLING: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f` on the island unless it is already borrowed (re-entrant window
/// messages sent while the main loop is drawing fall back to DefWindowProc).
fn with_island<R>(f: impl FnOnce(&mut Island) -> R) -> Option<R> {
    ISLAND.with(|cell| cell.try_borrow_mut().ok().and_then(|mut i| i.as_mut().map(f)))
}

pub fn is_animating() -> bool {
    with_island(|i| i.is_animating()).unwrap_or(false)
}

pub fn frame() {
    with_island(|i| i.frame());
}

pub fn create(cfg: Config) -> Result<HWND> {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("WinIslandWindow"),
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(windows::core::Error::from_win32());
        }

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
            w!("WinIslandWindow"),
            w!("Win Island"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )?;

        let mut island = Island {
            hwnd,
            theme: Theme::resolve(&cfg),
            tiles: Vec::new(),
            panel_height: 0.0,
            renderer: None,
            width: Spring::new(cfg.collapsed_width, STIFFNESS, DAMPING),
            height: Spring::new(cfg.collapsed_height, STIFFNESS, DAMPING),
            radius: Spring::new(cfg.collapsed_height / 2.0, STIFFNESS, DAMPING),
            wobble: Spring::new(0.0, WOBBLE_STIFFNESS, WOBBLE_DAMPING),
            phase: 0.0,
            cfg,
            scale: 1.0,
            win_size: (0, 0),
            region: None,
            hovered: false,
            hovered_tile: None,
            dirty: true,
            last_frame: None,
            stats: Stats::new(),
            compact: Compact { time: clock_text(), weather: None, media: None, toast: None, timer: None },
            weather: None,
            media_service: None,
            media: None,
            media_view: None,
            dragging_volume: false,
            last_volume_sent: None,
            lyrics_service: None,
            lyrics_key: None,
            lyrics: None,
            beat: None,
            history: History::load(),
            clip_rows: Vec::new(),
            ignore_next_clip: false,
            dock_apps: Vec::new(),
            dock_icons: Vec::new(),
            hot: None,
            fs_watcher: None,
            fullscreen: false,
            hide: Spring::new(0.0, STIFFNESS, DAMPING),
            window_hidden: false,
            last_near: Instant::now(),
            docked: false,
            signals: None,
            privacy: None,
            in_use: InUse::default(),
            active: None,
            window_view: None,
            app_icons: Vec::new(),
            extra_icons: Vec::new(),
            layout: signals::foreground_layout(),
            tab: Tab::Home,
            home_height: 0.0,
            store: notes::Store::load(),
            note: TextField::new(true),
            remind: TextField::new(false),
            focus: Focus::None,
            timer: Timer::default(),
            notes_view: None,
            clip_page: Vec::new(),
            alarm: None,
            alarm_pulses: 0,
            alarm_since: Instant::now(),
            pending_surrogate: None,
            prev_foreground: 0,
            index: launcher::Index::new(),
            search: TextField::new(false),
            search_hits: Vec::new(),
            search_sel: 0,
            search_view: None,
            tab_before_search: Tab::Home,
            shelf: shelf::load(),
            shelf_rows: Vec::new(),
            shelf_target: None,
            shelf_press: None,
            text: TextField::new(true),
            text_status: String::new(),
            text_view: None,
            ocr_slot: Arc::new(Mutex::new(None)),
            translate_slot: Arc::new(Mutex::new(None)),
            translating: false,
            quiet_next_clip: false,
            control: None,
            ctl: control::State::default(),
            ctl_segment: 0,
            ctl_pw_ssid: None,
            ctl_pw: TextField::new(false),
            power_confirm: None,
            ctl_view: None,
            agent_inbox: None,
            agent_cards: Vec::new(),
            agent_text_w: 0.0,
            top_gap: Spring::new(0.0, STIFFNESS, DAMPING),
        };
        island.top_gap = Spring::new(island.cfg.top_offset, STIFFNESS, DAMPING);
        if island.cfg.dock_apps.is_none() {
            island.cfg.dock_apps = Some(dock::default_apps());
            island.cfg.save();
        }
        island.load_dock();
        island.refresh_shelf_rows();
        island.shelf_target = shelf::Target::register(hwnd);
        island.signals = Some(Signals::start(hwnd));
        island.agent_inbox = Some(agent::start(hwnd.0 as isize, WM_AGENT));
        if island.cfg.privacy_indicator {
            island.privacy = privacy::Watcher::start(hwnd);
            island.in_use = island.privacy.as_ref().map(|p| p.get()).unwrap_or_default();
        }
        island.note = TextField::with_text(&island.store.note, true);
        island.note.caret = island.note.len();
        island.refresh_notes_view();
        island.arm_reminders();
        island.active = active_app::current();
        island.refresh_window_view();
        island.refresh_clip_rows();
        island.layout();
        island.renderer = Some(Renderer::new(hwnd, island.win_size.0 as u32, island.win_size.1 as u32, island.cfg.gpu)?);
        island.weather = weather::Service::start(hwnd, &island.cfg.weather_city);
        island.media_service = media::Service::start(hwnd);
        island.lyrics_service = lyrics::Service::start(hwnd);
        if island.cfg.clipboard_history {
            clipboard::listen(hwnd);
        }
        if island.cfg.hide_in_fullscreen {
            island.fs_watcher = Some(fullscreen::Watcher::start(hwnd));
        }
        if island.cfg.shrink_under_windows {
            SetTimer(Some(hwnd), TIMER_DOCK, DOCK_POLL_MS, None);
        }
        ISLAND.with(|cell| *cell.borrow_mut() = Some(island));
        arm_clock(hwnd);

        TASKBAR_CREATED.with(|c| c.set(RegisterWindowMessageW(w!("TaskbarCreated"))));
        tray::add(hwnd);
        let _ = RegisterHotKey(Some(hwnd), HOTKEY_THEME, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 'T' as u32);
        if with_island(|i| i.cfg.launcher) == Some(true) {
            let _ = RegisterHotKey(Some(hwnd), HOTKEY_LAUNCHER, MOD_ALT | MOD_NOREPEAT, VK_SPACE.0 as u32);
        }
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Ok(hwnd)
    }
}

impl Island {
    fn is_animating(&self) -> bool {
        !self.window_hidden
            && (self.dirty
                || !(self.width.is_settled()
                    && self.height.is_settled()
                    && self.radius.is_settled()
                    && self.wobble.is_settled()
                    && self.hide.is_settled()
                    && self.top_gap.is_settled()))
    }

    /// Re-evaluates whether a full-screen app covers the island.
    fn check_fullscreen(&mut self) {
        let fs = self.cfg.hide_in_fullscreen && fullscreen::active(self.hwnd);
        crate::stats::log_line(&format!("fullscreen check: {fs} (was {})", self.fullscreen));
        if fs == self.fullscreen {
            return;
        }
        self.fullscreen = fs;
        self.check_docked();
        if fs {
            self.hide_toast();
            if !self.hovered {
                self.slide_out();
            }
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_EDGE, EDGE_POLL_MS, None);
            }
        } else {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_EDGE);
            }
            self.slide_in();
        }
    }

    fn slide_out(&mut self) {
        if self.hide.target != 100.0 {
            self.hide.target = 100.0;
            self.last_frame = None;
            self.dirty = true;
        }
    }

    fn slide_in(&mut self) {
        if self.window_hidden {
            self.window_hidden = false;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                // Full-screen apps are often topmost too: get back on top.
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
        if self.hide.target != 0.0 {
            self.hide.target = 0.0;
            self.wobble.velocity += 40.0;
            self.last_frame = None;
            self.dirty = true;
        }
        self.last_near = Instant::now();
    }

    /// 10 Hz while a full-screen app is up: reveal at the top edge, hide
    /// again once the cursor has moved away.
    fn edge_tick(&mut self) {
        if !self.fullscreen {
            return;
        }
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut rect);
        }
        let at_top = fullscreen::cursor_at_top(rect.left, rect.right);
        if at_top || self.hovered || self.dragging_volume {
            self.last_near = Instant::now();
        }
        if self.hide.target == 100.0 {
            if at_top {
                self.slide_in();
            }
        } else if self.last_near.elapsed() >= REVEAL_GRACE {
            self.slide_out();
        }
    }

    /// Re-packs the Home tiles and places the window at the top center of
    /// the primary monitor, sized for the expanded state at the current DPI.
    fn layout(&mut self) {
        let kinds = layout::tiles_from_ids(&self.cfg.home_tiles);
        let (mut tiles, home_height) = layout::pack(&kinds, self.cfg.expanded_width);
        for t in &mut tiles {
            t.y += HOME_OFFSET;
        }
        self.tiles = tiles;
        self.home_height = home_height + HOME_OFFSET;
        self.panel_height = self.tab_height(self.tab);
        let panel_height = self.home_height.max(layout::TAB_BAR_H + layout::PAGE_H);
        self.hovered_tile = None;
        if self.hovered {
            self.height.target = self.panel_height;
        }
        unsafe {
            let dpi = GetDpiForWindow(self.hwnd).max(96);
            self.scale = dpi as f32 / 96.0;
            let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let mon = info.rcMonitor;

            let tallest = panel_height.max(self.cfg.collapsed_height);
            let w = ((self.cfg.expanded_width + MARGIN * 2.0) * self.scale).ceil() as i32;
            let h = ((tallest + self.cfg.top_offset + MARGIN) * self.scale).ceil() as i32;
            let x = mon.left + (mon.right - mon.left - w) / 2;
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, mon.top, w, h, SWP_NOACTIVATE);
            self.win_size = (w, h);
            if let Some(r) = self.renderer.as_mut() {
                let _ = r.resize(w as u32, h as u32);
            }
        }
        self.region = None;
        self.dirty = true;
    }

    fn reload(&mut self, cfg: Config) {
        let gpu_changed = cfg.gpu != self.cfg.gpu;
        if cfg.weather_city.trim() != self.cfg.weather_city.trim() {
            self.weather = weather::Service::start(self.hwnd, &cfg.weather_city);
            self.compact.weather = None;
        }
        self.cfg = cfg;
        self.theme = Theme::resolve(&self.cfg);
        if gpu_changed {
            self.renderer = None;
        }
        // Snap the collapsed geometry to the new config.
        self.retarget_collapsed();
        self.top_gap.target = if self.docked { 0.0 } else { self.cfg.top_offset };
        unsafe {
            if self.cfg.shrink_under_windows {
                SetTimer(Some(self.hwnd), TIMER_DOCK, DOCK_POLL_MS, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_DOCK);
            }
        }
        self.check_docked();
        self.layout();
        self.update_beat();
        self.load_dock();
        match (self.cfg.hide_in_fullscreen, self.fs_watcher.is_some()) {
            (true, false) => self.fs_watcher = Some(fullscreen::Watcher::start(self.hwnd)),
            (false, true) => self.fs_watcher = None,
            _ => {}
        }
        self.check_fullscreen();
        if self.cfg.clipboard_history {
            clipboard::listen(self.hwnd);
        } else {
            clipboard::stop_listening(self.hwnd);
        }
        if self.renderer.is_none() {
            self.renderer =
                Renderer::new(self.hwnd, self.win_size.0 as u32, self.win_size.1 as u32, self.cfg.gpu).ok();
        }
    }

    fn toggle_theme(&mut self) {
        self.cfg.theme = if self.theme.light { "dark" } else { "light" }.into();
        self.cfg.save();
        self.theme = Theme::resolve(&self.cfg);
        self.dirty = true;
        self.wobble.velocity += 40.0;
    }

    fn set_hover(&mut self, hovered: bool) {
        if self.hovered == hovered || (!hovered && self.focus != Focus::None) {
            return;
        }
        self.hovered = hovered;
        let c = &self.cfg;
        if hovered {
            self.width.target = c.expanded_width;
            self.height.target = self.panel_height;
            self.radius.target = c.expanded_radius;
            self.wobble.velocity += 90.0;
            if let Some(app) = active_app::current() {
                self.set_active(app, false);
            }
        } else {
            self.hovered = false;
            self.retarget_collapsed();
            self.wobble.velocity -= 60.0;
            self.hovered_tile = None;
            self.hot = None;
        }
        self.last_frame = None;
        self.update_media_timer();
    }

    /// Over a maximized window the collapsed island is a thin handle
    /// (unless a notice like "Скопировано" is showing).
    fn handle_mode(&self) -> bool {
        self.docked && self.compact.toast.is_none()
    }

    /// Points the size springs at the current collapsed look.
    fn retarget_collapsed(&mut self) {
        if self.hovered {
            return;
        }
        if self.handle_mode() {
            self.width.target = HANDLE_W;
            self.height.target = HANDLE_H;
            self.radius.target = HANDLE_H / 2.0;
        } else {
            self.width.target = self.collapsed_width();
            self.height.target = self.cfg.collapsed_height;
            self.radius.target = self.cfg.collapsed_height / 2.0;
        }
        self.last_frame = None;
    }

    fn check_docked(&mut self) {
        let center = {
            let mut r = RECT::default();
            unsafe {
                let _ = GetWindowRect(self.hwnd, &mut r);
            }
            (r.left + r.right) / 2
        };
        let docked = self.cfg.shrink_under_windows && !self.fullscreen && fullscreen::window_under(self.hwnd, center);
        if docked != self.docked {
            crate::stats::log_line(&format!("docked -> {docked}, fg = {}", fullscreen::foreground_debug()));
            self.docked = docked;
            self.top_gap.target = if docked { 0.0 } else { self.cfg.top_offset };
            self.retarget_collapsed();
            self.wobble.velocity += 30.0;
            self.dirty = true;
        }
    }

    /// 1 for the full pill, 0 for the thin handle (follows the animation).
    fn pill_amount(&self) -> f32 {
        ((self.height.value - HANDLE_H) / (self.cfg.collapsed_height - HANDLE_H)).clamp(0.0, 1.0)
    }

    /// Pill width taken by everything but the text: badge, gaps, buttons
    /// (matches `Renderer::paint_compact`).
    fn agent_card_chrome(&self, actions: &[String]) -> f32 {
        let h = self.cfg.collapsed_height;
        let buttons = layout::toast_actions(1000.0, h, actions);
        let used = 1000.0 - buttons.first().map_or(1000.0, |b| b.x);
        h + 4.0 + 8.0 + used
    }

    /// Widest an agent card gets: the window's width. Longer text loses
    /// the ✓ button.
    fn agent_card_max_w(&self) -> f32 {
        self.cfg.expanded_width.max(self.cfg.collapsed_media_width)
    }

    fn collapsed_width(&self) -> f32 {
        if let Some(t) = self.compact.toast.as_ref().filter(|t| !t.actions.is_empty()) {
            // Room for the badge, the text (an agent card's in full) and
            // every button.
            let text_w = if matches!(self.alarm, Some(Alarm::Agent(_))) { self.agent_text_w + 2.0 } else { 170.0 };
            return (self.agent_card_chrome(&t.actions) + text_w).clamp(self.cfg.collapsed_media_width, self.agent_card_max_w());
        }
        if self.compact.media.is_some() || self.compact.toast.is_some() {
            self.cfg.collapsed_media_width
        } else {
            self.cfg.collapsed_width
        }
    }

    fn media_arrived(&mut self) {
        self.media = self.media_service.as_ref().and_then(|s| s.get());
        self.request_lyrics();
        self.refresh_media_view();
        self.refresh_compact_media();
        self.update_media_timer();
        self.arm_lyric_timer();
        self.update_beat();
    }

    /// Loopback capture runs only while music plays and the edge effect is on.
    fn update_beat(&mut self) {
        let want = self.cfg.beat_edge && self.media.as_ref().is_some_and(|m| m.playing);
        if want && self.beat.is_none() {
            self.beat = beat::Capture::start(self.hwnd);
        } else if !want {
            self.beat = None;
        }
    }

    /// A bass hit: push the liquid edge, harder for stronger hits.
    fn on_beat(&mut self, strength: f32) {
        if self.dragging_volume || self.hide.target == 100.0 || (self.handle_mode() && !self.hovered) {
            return; // No dancing while dragging, hidden, or shrunk to the handle.
        }
        if !self.is_animating() {
            self.last_frame = None;
        }
        self.wobble.velocity = (self.wobble.velocity + 30.0 + 50.0 * strength).clamp(-150.0, 150.0);
    }

    /// Asks for lyrics once per track; drops the previous track's lines.
    fn request_lyrics(&mut self) {
        let Some(m) = self.media.as_ref().filter(|m| !m.title.is_empty()) else {
            self.lyrics_key = None;
            self.lyrics = None;
            return;
        };
        let key = lyrics::track_key(&m.artist, &m.title);
        if self.lyrics_key == Some(key) {
            return;
        }
        self.lyrics = None;
        self.lyrics_key = Some(key);
        if let Some(svc) = &self.lyrics_service {
            svc.request(&m.artist, &m.title, m.duration);
        }
    }

    fn lyrics_arrived(&mut self) {
        let Some((key, lines)) = self.lyrics_service.as_ref().and_then(|s| s.get()) else { return };
        if Some(key) != self.lyrics_key {
            return; // A late answer for a previous track.
        }
        self.lyrics = (!lines.is_empty()).then_some(lines);
        self.refresh_media_view();
        self.refresh_compact_media();
        self.arm_lyric_timer();
    }

    /// The line being sung right now, if lyrics are known.
    fn current_lyric(&self) -> Option<String> {
        let m = self.media.as_ref()?;
        let lines = self.lyrics.as_ref()?;
        lyrics::line_at(lines, m.position_now()).map(|i| lines[i].text.clone())
    }

    /// Sleeps until the next lyric line instead of polling.
    fn arm_lyric_timer(&self) {
        let next = self
            .media
            .as_ref()
            .filter(|m| m.playing)
            .zip(self.lyrics.as_ref())
            .and_then(|(m, lines)| lyrics::until_next(lines, m.position_now()));
        unsafe {
            match next {
                Some(d) => {
                    SetTimer(Some(self.hwnd), TIMER_LYRIC, (d.as_millis() as u32).saturating_add(30).max(50), None);
                }
                None => {
                    let _ = KillTimer(Some(self.hwnd), TIMER_LYRIC);
                }
            }
        }
    }

    fn lyric_tick(&mut self) {
        self.refresh_media_view();
        self.refresh_compact_media();
        self.arm_lyric_timer();
    }

    fn refresh_compact_media(&mut self) {
        let lyric = self.current_lyric().filter(|l| !l.is_empty());
        let compact = self.media.as_ref().filter(|m| m.playing).map(|m| CompactMedia {
            title: m.title.clone(),
            is_lyric: lyric.is_some(),
            second_line: lyric.clone().unwrap_or_else(|| m.artist.clone()),
            cover_id: m.cover.as_ref().map(|c| c.id),
        });
        if compact != self.compact.media {
            let mode_changed = compact.is_some() != self.compact.media.is_some();
            self.compact.media = compact;
            self.dirty = true;
            if mode_changed && !self.hovered {
                self.retarget_collapsed();
                if !self.handle_mode() {
                    self.wobble.velocity += 50.0;
                }
            }
        }
    }

    /// Re-derives what the player tile shows (position moves every second).
    fn refresh_media_view(&mut self) {
        let view = self.media.as_ref().map(|m| {
            let pos = m.position_now();
            let permille = if m.duration.is_zero() {
                0
            } else {
                ((pos.as_secs_f64() / m.duration.as_secs_f64()) * 1000.0).round().clamp(0.0, 1000.0) as u32
            };
            MediaView {
                title: m.title.clone(),
                artist: m.artist.clone(),
                playing: m.playing,
                position: media::format_time(pos),
                duration: if m.duration.is_zero() { String::new() } else { media::format_time(m.duration) },
                progress_permille: permille,
                cover_id: m.cover.as_ref().map(|c| c.id),
                volume: m.volume.map(|v| (v * 100.0).round() as u32),
                can_prev: m.can_prev,
                can_next: m.can_next,
                lyric: None,
            }
        });
        let lyric = self.current_lyric();
        let view = view.map(|v| MediaView { lyric, ..v });
        if view != self.media_view {
            self.media_view = view;
            self.dirty = true;
        }
    }

    /// A 1 s tick only while the player is on screen and moving.
    fn update_media_timer(&self) {
        let needed = self.hovered && self.media.as_ref().is_some_and(|m| m.playing);
        unsafe {
            if needed {
                SetTimer(Some(self.hwnd), TIMER_MEDIA, 1000, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_MEDIA);
            }
        }
    }

    /// The clickable thing under (x, y) client pixels, if any.
    fn hit_at(&self, x: i32, y: i32) -> Option<Hit> {
        if self.content_alpha() < 0.9 {
            return None;
        }
        let shape = self.shape();
        let (ox, oy) = self.panel_origin(&shape);
        let (dx, dy) = ((x as f32 - ox) / self.scale, (y as f32 - oy) / self.scale);
        if self.tab == Tab::Search {
            let w = self.cfg.expanded_width;
            if layout::search_field(w).contains(dx, dy) {
                return Some(Hit::SearchField);
            }
            let n = self.search_view.as_ref().map_or(0, |v| v.rows.len());
            return layout::search_rows(w).iter().take(n).position(|r| r.contains(dx, dy)).map(Hit::SearchRow);
        }
        if dy < layout::TAB_BAR_H {
            return layout::tab_buttons(self.tab).into_iter().find(|(_, r)| r.contains(dx, dy)).map(|(t, _)| Hit::Tab(t));
        }
        match self.tab {
            Tab::Home | Tab::Search => {}
            Tab::Clipboard => {
                let w = self.cfg.expanded_width - 2.0 * layout::PAD;
                return layout::clip_page_rows()
                    .iter()
                    .take(self.clip_page.len())
                    .position(|r| Rect { w, ..*r }.contains(dx, dy))
                    .map(Hit::Clip);
            }
            Tab::Shelf => {
                let w = self.cfg.expanded_width - 2.0 * layout::PAD;
                return layout::clip_page_rows()
                    .iter()
                    .take(self.shelf_rows.len())
                    .position(|r| Rect { w, ..*r }.contains(dx, dy))
                    .map(Hit::ShelfRow);
            }
            Tab::Control => {
                let r = layout::control_regions(self.cfg.expanded_width);
                if let Some(i) = r.segments.iter().position(|s| s.contains(dx, dy)) {
                    return Some(Hit::CtlSegment(i));
                }
                if r.refresh.contains(dx, dy) {
                    return Some(Hit::CtlRefresh);
                }
                if r.settings.contains(dx, dy) {
                    return Some(Hit::CtlSettings);
                }
                if let Some(i) = r.power.iter().position(|p| p.contains(dx, dy)) {
                    return Some(Hit::Power(i));
                }
                let view = self.ctl_view.as_ref()?;
                if let Some((row, _, _)) = &view.password {
                    let (field, go) = layout::password_parts(&r.rows[*row]);
                    if field.contains(dx, dy) {
                        return Some(Hit::PasswordField);
                    }
                    if go.contains(dx, dy) {
                        return Some(Hit::PasswordGo);
                    }
                }
                return r
                    .actions
                    .iter()
                    .enumerate()
                    .take(view.rows.len())
                    .find(|(i, a)| a.contains(dx, dy) && view.rows[*i].action.is_some() && view.password.as_ref().is_none_or(|p| p.0 != *i))
                    .map(|(i, _)| Hit::CtlAction(i));
            }
            Tab::Text => {
                let r = layout::text_regions(self.cfg.expanded_width);
                if r.copy.contains(dx, dy) {
                    return Some(Hit::TextCopy);
                }
                if r.translate.contains(dx, dy) {
                    return Some(Hit::TextTranslate);
                }
                if let Some(i) = r.ops.iter().position(|o| o.contains(dx, dy)) {
                    return Some(Hit::TextOp(i));
                }
                return r.card.contains(dx, dy).then_some(Hit::TextField);
            }
            Tab::Notes => {
                let r = layout::notes_regions(self.cfg.expanded_width);
                if r.toggle.contains(dx, dy) {
                    return Some(Hit::TimerToggle);
                }
                if r.reset.contains(dx, dy) {
                    return Some(Hit::TimerReset);
                }
                if let Some(i) = r.presets.iter().position(|p| p.contains(dx, dy)) {
                    return Some(Hit::TimerPreset(i));
                }
                if r.remind.contains(dx, dy) {
                    return Some(Hit::RemindField);
                }
                if let Some(i) = r.removes.iter().take(self.store.reminders.len()).position(|p| p.contains(dx, dy)) {
                    return Some(Hit::RemindRemove(i));
                }
                if r.note_card.contains(dx, dy) {
                    return Some(Hit::NoteField);
                }
                return None;
            }
        }
        let tile = self.tiles.iter().find(|t| t.contains(dx, dy))?;
        match tile.kind {
            TileKind::Media => {
                let m = self.media.as_ref()?;
                let r = layout::media_regions(tile);
                [(Hit::MediaPrev, r.prev), (Hit::MediaPlay, r.play), (Hit::MediaNext, r.next)]
                    .into_iter()
                    .find(|(_, rc)| rc.contains(dx, dy))
                    .map(|(h, _)| h)
                    .or_else(|| (m.volume.is_some() && r.volume_hit.contains(dx, dy)).then_some(Hit::MediaVolume))
            }
            TileKind::Clipboard => layout::clip_rows(tile)
                .iter()
                .take(self.clip_rows.len())
                .position(|r| r.contains(dx, dy))
                .map(Hit::Clip),
            TileKind::Tools => layout::tool_buttons(tile).iter().position(|r| r.contains(dx, dy)).map(Hit::Tool),
            TileKind::Apps => {
                let (slots, add) = layout::dock_slots(tile, self.dock_icons.len());
                slots.iter().position(|r| r.contains(dx, dy)).map(Hit::Dock).or_else(|| add.contains(dx, dy).then_some(Hit::DockAdd))
            }
            _ => None,
        }
    }

    fn is_clickable(&self, x: i32, y: i32) -> bool {
        self.hit_at(x, y).is_some()
    }

    /// Handles a left click; returns true when a volume drag started.
    fn click(&mut self, x: i32, y: i32) -> bool {
        let send = |i: &Self, c: Command| {
            if let Some(svc) = &i.media_service {
                svc.send(c);
            }
        };
        match self.hit_at(x, y) {
            Some(Hit::MediaPlay) => {
                // Flip the icon now; the player's own event confirms it.
                if let Some(m) = self.media.as_mut() {
                    m.position = m.position_now();
                    m.sampled_at = Instant::now();
                    m.playing = !m.playing;
                }
                self.refresh_media_view();
                self.arm_lyric_timer();
                send(self, Command::PlayPause);
            }
            Some(Hit::MediaNext) => send(self, Command::Next),
            Some(Hit::MediaPrev) => send(self, Command::Prev),
            Some(Hit::MediaVolume) => {
                self.dragging_volume = true;
                self.drag_volume(x, true);
                return true;
            }
            Some(Hit::Clip(i)) => self.recopy(i),
            Some(Hit::Tab(t)) => self.switch_tab(t),
            Some(Hit::NoteField) => self.set_focus(Focus::Note),
            Some(Hit::SearchField) => self.set_focus(Focus::Search),
            Some(Hit::SearchRow(i)) => self.open_hit(i, false),
            Some(Hit::Tool(i)) => self.run_tool(i),
            Some(Hit::CtlSegment(i)) => {
                self.ctl_segment = i;
                self.ctl_pw_ssid = None;
                self.refresh_control_view();
            }
            Some(Hit::CtlRefresh) => {
                if let Some(c) = &self.control {
                    c.send(control::Command::Refresh);
                }
            }
            Some(Hit::CtlSettings) => {
                let page = ["ms-settings:network-wifi", "ms-settings:bluetooth", "ms-settings:network-vpn"][self.ctl_segment.min(2)];
                control::open_settings(page);
            }
            Some(Hit::CtlAction(i)) => self.control_action(i),
            Some(Hit::PasswordField) => self.set_focus(Focus::Password),
            Some(Hit::PasswordGo) => self.submit_password(),
            Some(Hit::Power(i)) => self.power_click(i),
            Some(Hit::TextField) => self.set_focus(Focus::Text),
            Some(Hit::TextOp(i)) => self.text_op(i),
            Some(Hit::TextTranslate) => self.translate_text(),
            Some(Hit::TextCopy) => {
                let text = self.text.selected_text().unwrap_or_else(|| self.text.text.clone());
                if !text.is_empty() {
                    self.quiet_next_clip = true;
                    clipboard::write(self.hwnd, &Entry::Text { text });
                    self.text_status = "Скопировано".into();
                    self.refresh_text_view();
                }
            }
            Some(Hit::ShelfRow(i)) => {
                // Open on release unless it turns into a drag.
                self.shelf_press = Some((i, x, y));
                return true;
            }
            Some(Hit::RemindField) => self.set_focus(Focus::Remind),
            Some(Hit::TimerPreset(i)) => {
                self.timer.start(Duration::from_secs(layout::TIMER_PRESETS[i]), Instant::now());
                self.timer_changed();
            }
            Some(Hit::TimerToggle) => {
                if self.alarm == Some(Alarm::Timer) {
                    self.dismiss_alarm();
                }
                self.timer.toggle(Instant::now());
                self.timer_changed();
            }
            Some(Hit::TimerReset) => {
                if self.alarm == Some(Alarm::Timer) {
                    self.dismiss_alarm();
                }
                self.timer.reset();
                self.timer_changed();
            }
            Some(Hit::RemindRemove(i)) => {
                if i < self.store.reminders.len() {
                    self.store.reminders.remove(i);
                    self.store.save();
                    self.arm_reminders();
                    self.refresh_notes_view();
                }
            }
            Some(Hit::Dock(i)) => {
                if let Some(app) = self.dock_apps.get(i) {
                    dock::launch(&app.path);
                }
            }
            Some(Hit::WinPin) => {
                if let Some(app) = self.active.as_mut() {
                    app.topmost = active_app::toggle_topmost(app);
                }
                self.refresh_window_view();
            }
            Some(Hit::WinOpacity) => {
                if let Some(app) = self.active.as_mut() {
                    app.alpha = active_app::cycle_opacity(app);
                }
                self.refresh_window_view();
            }
            Some(Hit::WinMinimize) => {
                if let Some(app) = &self.active {
                    active_app::minimize(app);
                }
            }
            Some(Hit::DockAdd) => {
                if let Some(path) = dock::pick(self.hwnd) {
                    if let Some(list) = self.cfg.dock_apps.as_mut() {
                        if !list.iter().any(|p| dock::expand(p).eq_ignore_ascii_case(&path)) {
                            list.push(path);
                        }
                    }
                    self.cfg.save();
                    self.load_dock();
                }
            }
            None => {
                // A click elsewhere ends typing.
                self.set_focus(Focus::None);
            }
        }
        false
    }

    fn tab_height(&self, tab: Tab) -> f32 {
        match tab {
            Tab::Home => self.home_height,
            Tab::Search => layout::SEARCH_H,
            _ => layout::TAB_BAR_H + layout::PAGE_H,
        }
    }

    fn switch_tab(&mut self, tab: Tab) {
        if tab == self.tab {
            return;
        }
        self.set_focus(Focus::None);
        self.tab = tab;
        self.panel_height = self.tab_height(tab);
        if self.hovered {
            self.height.target = self.panel_height;
        }
        self.hot = None;
        self.wobble.velocity += 25.0;
        self.dirty = true;
        self.last_frame = None;
        if tab == Tab::Clipboard {
            self.refresh_clip_rows();
        }
        if tab == Tab::Control {
            // Started on first use; refreshed (and Wi-Fi rescanned) on each visit.
            if self.control.is_none() {
                self.control = control::Service::start(self.hwnd);
            }
            if let Some(c) = &self.control {
                c.send(control::Command::Refresh);
            }
            self.refresh_control_view();
        }
    }

    fn set_focus(&mut self, focus: Focus) {
        if focus == self.focus {
            return;
        }
        if self.focus == Focus::Note {
            self.save_note();
        }
        let was_typing = self.focus != Focus::None;
        unsafe {
            if !was_typing && focus != Focus::None {
                let fg = GetForegroundWindow();
                self.prev_foreground = if fg == self.hwnd { 0 } else { fg.0 as isize };
            } else if was_typing && focus == Focus::None && GetForegroundWindow() == self.hwnd {
                let prev = HWND(self.prev_foreground as *mut _);
                if self.prev_foreground != 0 && IsWindow(Some(prev)).as_bool() {
                    let _ = SetForegroundWindow(prev);
                }
            }
        }
        self.focus = focus;
        self.refresh_text_view();
        if focus != Focus::None {
            // Typing needs the keyboard: make the island the active window.
            unsafe {
                let _ = SetForegroundWindow(self.hwnd);
                let _ = SetFocus(Some(self.hwnd));
            }
        }
        self.refresh_notes_view();
    }

    fn field(&mut self) -> Option<&mut TextField> {
        match self.focus {
            Focus::Note => Some(&mut self.note),
            Focus::Remind => Some(&mut self.remind),
            Focus::Search => Some(&mut self.search),
            Focus::Text => Some(&mut self.text),
            Focus::Password => Some(&mut self.ctl_pw),
            Focus::None => None,
        }
    }

    fn after_edit(&mut self) {
        if self.focus == Focus::Password {
            self.refresh_control_view();
            return;
        }
        if self.focus == Focus::Text {
            self.text_status.clear();
            self.refresh_text_view();
            return;
        }
        if self.focus == Focus::Search {
            self.run_search();
            return;
        }
        if self.focus == Focus::Note {
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_NOTE_SAVE, 800, None);
            }
        }
        self.refresh_notes_view();
    }

    fn save_note(&mut self) {
        if self.store.note != self.note.text {
            self.store.note = self.note.text.clone();
            self.store.save();
        }
    }

    /// A typed character (WM_CHAR).
    fn char_input(&mut self, unit: u16) {
        if self.focus == Focus::None {
            return;
        }
        let ch = if (0xD800..0xDC00).contains(&unit) {
            self.pending_surrogate = Some(unit);
            return;
        } else if (0xDC00..0xE000).contains(&unit) {
            let Some(high) = self.pending_surrogate.take() else { return };
            char::decode_utf16([high, unit]).next().and_then(|r| r.ok())
        } else {
            char::from_u32(unit as u32)
        };
        let Some(ch) = ch.filter(|c| !c.is_control()) else { return };
        if let Some(f) = self.field() {
            f.insert(&ch.to_string());
        }
        self.after_edit();
    }

    /// Editing keys (WM_KEYDOWN). Returns true if handled.
    fn key_down(&mut self, vk: u16) -> bool {
        if self.focus == Focus::None {
            return false;
        }
        let down = |k: VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) } < 0;
        let (ctrl, shift) = (down(VK_CONTROL), down(VK_SHIFT));
        let hwnd = self.hwnd;
        let multiline = matches!(self.focus, Focus::Note | Focus::Text);
        if self.focus == Focus::Search {
            let n = self.search_hits.len();
            match VIRTUAL_KEY(vk) {
                VK_ESCAPE => {
                    self.close_search();
                    return true;
                }
                VK_RETURN => {
                    self.open_hit(self.search_sel, ctrl);
                    return true;
                }
                VK_DOWN if n > 0 => {
                    self.search_sel = (self.search_sel + 1) % n;
                    self.refresh_search_view();
                    return true;
                }
                VK_UP if n > 0 => {
                    self.search_sel = (self.search_sel + n - 1) % n;
                    self.refresh_search_view();
                    return true;
                }
                _ => {}
            }
        }
        if self.focus == Focus::Password && VIRTUAL_KEY(vk) == VK_ESCAPE {
            self.ctl_pw_ssid = None;
            self.ctl_pw.clear();
            self.set_focus(Focus::None);
            self.refresh_control_view();
            return true;
        }
        match VIRTUAL_KEY(vk) {
            VK_ESCAPE => {
                self.set_focus(Focus::None);
                unsafe {
                    SetTimer(Some(hwnd), TIMER_COLLAPSE, COLLAPSE_DELAY_MS, None);
                }
                return true;
            }
            VK_RETURN if !multiline && self.focus == Focus::Password => {
                self.submit_password();
                return true;
            }
            VK_RETURN if !multiline => {
                self.submit_reminder();
                return true;
            }
            _ => {}
        }
        let Some(f) = self.field() else { return false };
        match VIRTUAL_KEY(vk) {
            VK_RETURN => f.insert("\n"),
            VK_BACK => f.backspace(ctrl),
            VK_DELETE => f.delete(ctrl),
            VK_LEFT => f.left(ctrl, shift),
            VK_RIGHT => f.right(ctrl, shift),
            VK_HOME => f.home(shift),
            VK_END => f.end(shift),
            VK_UP if multiline => f.vertical(false, shift),
            VK_DOWN if multiline => f.vertical(true, shift),
            VK_A if ctrl => f.select_all(),
            VK_C | VK_X if ctrl => {
                if let Some(text) = f.selected_text() {
                    if vk == VK_X.0 {
                        f.delete(false);
                    }
                    clipboard::write(hwnd, &Entry::Text { text });
                }
            }
            VK_V if ctrl => {
                if let Some(text) = clipboard::read_text(hwnd) {
                    f.insert(&text);
                }
            }
            _ => return false,
        }
        self.after_edit();
        true
    }

    /// Alt+Space: the island opens straight into the search.
    fn open_search(&mut self) {
        if self.tab != Tab::Search {
            self.tab_before_search = self.tab;
        }
        self.index.refresh(self.hwnd);
        if self.fullscreen {
            self.slide_in();
        }
        self.search.clear();
        self.search_hits.clear();
        self.search_sel = 0;
        self.tab = Tab::Search;
        self.panel_height = self.tab_height(Tab::Search);
        if self.hovered {
            self.height.target = self.panel_height;
            self.last_frame = None;
        } else {
            self.set_hover(true);
        }
        self.hot = None;
        self.set_focus(Focus::Search);
        self.refresh_search_view();
    }

    fn close_search(&mut self) {
        if self.tab != Tab::Search {
            return;
        }
        self.tab = self.tab_before_search;
        self.panel_height = self.tab_height(self.tab);
        self.set_focus(Focus::None);
        self.set_hover(false);
        self.dirty = true;
    }

    fn run_search(&mut self) {
        let q = self.search.text.trim().to_string();
        let mut hits = Vec::new();
        if let Some(v) = launcher::calc(&q) {
            hits.push(SearchHit::Calc(launcher::format_number(v)));
        }
        if !q.is_empty() {
            let items = self.index.snapshot();
            let room = layout::SEARCH_ROWS - hits.len() - 1;
            hits.extend(launcher::search(&items, &q, room).into_iter().map(|i| SearchHit::Item(items[i].clone())));
            hits.push(SearchHit::Web(q));
        }
        self.search_hits = hits;
        self.search_sel = 0;
        self.refresh_search_view();
    }

    fn refresh_search_view(&mut self) {
        let hits = self.search_hits.clone();
        let rows: Vec<SearchRow> = hits
            .iter()
            .map(|h| match h {
                SearchHit::Calc(v) => SearchRow { badge: Badge::Text("=".into()), title: format!("= {v}"), subtitle: "Enter — скопировать".into() },
                SearchHit::Web(q) => SearchRow { badge: Badge::Glyph(clip_history::Glyph::Link), title: format!("Искать «{q}» в интернете"), subtitle: "Браузер".into() },
                SearchHit::Item(it) => {
                    let badge = match it.kind {
                        launcher::Kind::App => self.app_icon(&it.path).map_or(Badge::Glyph(clip_history::Glyph::Files), Badge::App),
                        launcher::Kind::Folder => Badge::Glyph(clip_history::Glyph::Files),
                        _ => {
                            let lower = it.name.to_lowercase();
                            let image = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".heic"].iter().any(|e| lower.ends_with(e));
                            Badge::Glyph(if image { clip_history::Glyph::Image } else { clip_history::Glyph::Text })
                        }
                    };
                    let subtitle = match it.kind {
                        launcher::Kind::App => "Приложение".to_string(),
                        launcher::Kind::Recent => "Недавнее".to_string(),
                        launcher::Kind::Folder => "Папка".to_string(),
                        launcher::Kind::File if launcher::is_executable(&it.name) => "Программа · покажу в папке".to_string(),
                        launcher::Kind::File => std::path::Path::new(&it.path)
                            .parent()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    };
                    SearchRow { badge, title: it.name.clone(), subtitle }
                }
            })
            .collect();
        let indexed = !self.index.snapshot().is_empty();
        let hint = if self.search.is_empty() {
            "Приложения, файлы с рабочего стола, «Документов» и «Загрузок», калькулятор".to_string()
        } else if !indexed {
            "Индексирую файлы…".to_string()
        } else {
            "Ничего не нашлось".to_string()
        };
        let view = SearchView { field: self.search.clone(), rows, selected: self.search_sel.min(hits.len().saturating_sub(1)), hint };
        if Some(&view) != self.search_view.as_ref() {
            self.search_view = Some(view);
            self.dirty = true;
        }
    }

    /// Opens result `i`; with Ctrl, shows a file in its folder instead.
    fn open_hit(&mut self, i: usize, ctrl: bool) {
        let Some(hit) = self.search_hits.get(i).cloned() else { return };
        match hit {
            SearchHit::Item(it) => {
                // Programs found in user folders are shown, never started from
                // here: a downloaded "Telegram.exe" must not run on Enter.
                let reveal = ctrl || (it.kind == launcher::Kind::File && launcher::is_executable(&it.name));
                if reveal && matches!(it.kind, launcher::Kind::File | launcher::Kind::Folder) {
                    unsafe {
                        let _ = AllowSetForegroundWindow(ASFW_ANY);
                        windows::Win32::UI::Shell::ShellExecuteW(
                            None,
                            w!("open"),
                            w!("explorer.exe"),
                            &windows::core::HSTRING::from(format!("/select,\"{}\"", it.path)),
                            None,
                            SW_SHOWNORMAL,
                        );
                    }
                } else {
                    dock::launch(&it.path);
                }
            }
            SearchHit::Calc(v) => {
                clipboard::write(self.hwnd, &Entry::Text { text: v });
            }
            SearchHit::Web(q) => {
                let url = self.cfg.search_url.replace("{q}", &crate::http::encode(&q));
                dock::launch(&url);
            }
        }
        self.close_search();
    }

    fn refresh_shelf_rows(&mut self) {
        let rows: Vec<ClipRow> = self
            .shelf
            .iter()
            .map(|p| {
                let path = std::path::Path::new(p);
                let name = path.file_name().map_or(p.clone(), |n| n.to_string_lossy().into_owned());
                let lower = name.to_lowercase();
                let glyph = if path.is_dir() {
                    clip_history::Glyph::Files
                } else if [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".heic"].iter().any(|e| lower.ends_with(e)) {
                    clip_history::Glyph::Image
                } else {
                    clip_history::Glyph::Text
                };
                let folder = if path.exists() {
                    path.parent().and_then(|d| d.file_name()).map_or(String::new(), |n| n.to_string_lossy().into_owned())
                } else {
                    "нет файла".to_string()
                };
                ClipRow { glyph, text: name, ago: folder }
            })
            .collect();
        if rows != self.shelf_rows {
            self.shelf_rows = rows;
            self.dirty = true;
        }
    }

    /// Files are being dragged over the island: open it on the shelf.
    fn shelf_drag(&mut self, over: bool) {
        if over {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_COLLAPSE);
            }
            if self.tab != Tab::Shelf {
                self.set_focus(Focus::None);
                self.tab = Tab::Shelf;
                self.panel_height = self.tab_height(Tab::Shelf);
                self.dirty = true;
            }
            if self.hovered {
                self.height.target = self.panel_height;
                self.last_frame = None;
            } else {
                self.set_hover(true);
            }
        } else {
            // OLE owns the mouse during a drag, so no WM_MOUSELEAVE comes.
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_COLLAPSE, 1200, None);
            }
        }
    }

    fn shelf_dropped(&mut self) {
        let Some(files) = self.shelf_target.as_ref().map(|t| t.take()) else { return };
        if files.is_empty() {
            return;
        }
        shelf::add(&mut self.shelf, &files);
        shelf::save(&self.shelf);
        self.refresh_shelf_rows();
        self.wobble.velocity += 60.0;
        self.last_frame = None;
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_COLLAPSE, 1500, None);
        }
    }

    /// Tools tile button `i` (layout::TOOLS order).
    fn run_tool(&mut self, i: usize) {
        match i {
            0 => self.start_overlay(overlay::Mode::Snip),
            1 => self.full_screen_shot(),
            2 => self.start_overlay(overlay::Mode::Ocr),
            3 => self.start_overlay(overlay::Mode::Qr),
            4 => self.start_overlay(overlay::Mode::Picker),
            5 => self.plain_text(),
            _ => self.open_search(),
        }
    }

    /// Takes the island out of the picture, then grabs the screen.
    fn grab_without_island(&mut self, area: Option<RECT>) -> Option<capture::Shot> {
        self.set_focus(Focus::None);
        self.set_hover(false);
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        let shot = match area {
            Some(r) => capture::grab(r).map(|image| capture::Shot { x: r.left, y: r.top, image }),
            None => capture::shoot(),
        };
        unsafe {
            if !self.window_hidden {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
        }
        shot
    }

    fn start_overlay(&mut self, mode: overlay::Mode) {
        if overlay::is_open() {
            return;
        }
        if let Some(shot) = self.grab_without_island(None) {
            overlay::open(self.hwnd, mode, shot, self.scale);
        }
    }

    fn full_screen_shot(&mut self) {
        if let Some(shot) = self.grab_without_island(Some(capture::cursor_monitor())) {
            clipboard::write_image(self.hwnd, &shot.image);
        }
    }

    /// Re-puts the clipboard text without formatting.
    fn plain_text(&mut self) {
        let Some(text) = clipboard::read_text(self.hwnd).filter(|t| !t.is_empty()) else {
            self.show_toast(Toast { badge: Badge::Glyph(clip_history::Glyph::Text), title: "В буфере нет текста".into(), text: String::new(), level: None, actions: Vec::new() }, TOAST_MS);
            return;
        };
        self.quiet_next_clip = true;
        clipboard::write(self.hwnd, &Entry::Text { text: text.clone() });
        self.show_toast(Toast { badge: Badge::Glyph(clip_history::Glyph::Text), title: "Текст без оформления".into(), text, level: None, actions: Vec::new() }, TOAST_MS);
    }

    fn overlay_done(&mut self) {
        match overlay::take_result() {
            Some(overlay::Outcome::Snip(img)) => {
                // The clipboard listener records it and says "Скопировано".
                clipboard::write_image(self.hwnd, &img);
            }
            Some(overlay::Outcome::Area(overlay::Mode::Qr, img)) => match recognize::qr(&img) {
                Some(text) => {
                    self.quiet_next_clip = true;
                    clipboard::write(self.hwnd, &Entry::Text { text: text.clone() });
                    let link = text.starts_with("http://") || text.starts_with("https://");
                    let badge = Badge::Glyph(if link { clip_history::Glyph::Link } else { clip_history::Glyph::Text });
                    self.show_toast(Toast { badge, title: "QR-код скопирован".into(), text, level: None, actions: Vec::new() }, TOAST_MS);
                }
                None => {
                    self.show_toast(Toast { badge: Badge::Glyph(clip_history::Glyph::Image), title: "QR-код не найден".into(), text: "Выделите код целиком".into(), level: None, actions: Vec::new() }, TOAST_MS);
                }
            },
            Some(overlay::Outcome::Area(_, img)) => {
                self.text_status = "Распознаю…".into();
                recognize::spawn_ocr(self.hwnd, img, self.ocr_slot.clone());
            }
            Some(overlay::Outcome::Color(c)) => {
                let hex = format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
                self.quiet_next_clip = true;
                clipboard::write(self.hwnd, &Entry::Text { text: hex.clone() });
                let text = format!("{hex} · rgb({}, {}, {})", c[0], c[1], c[2]);
                self.show_toast(Toast { badge: Badge::Color(c), title: "Цвет скопирован".into(), text, level: None, actions: Vec::new() }, TOAST_MS);
            }
            Some(overlay::Outcome::Cancelled) | None => {}
        }
    }

    /// OCR finished: show the text in the Text tab, ready to edit.
    fn ocr_done(&mut self) {
        let Some(text) = self.ocr_slot.lock().unwrap().take() else { return };
        if text.trim().is_empty() {
            self.text_status.clear();
            self.show_toast(Toast { badge: Badge::Glyph(clip_history::Glyph::Text), title: "Текст не найден".into(), text: "Попробуйте выделить крупнее".into(), level: None, actions: Vec::new() }, TOAST_MS);
            return;
        }
        let lines = text.lines().count();
        self.text = TextField::with_text(&text, true);
        self.text_status = format!("Распознано строк: {lines}");
        self.show_text_tab();
    }

    fn show_text_tab(&mut self) {
        if self.tab != Tab::Text {
            self.tab = Tab::Text;
            self.panel_height = self.tab_height(Tab::Text);
            self.dirty = true;
        }
        if self.hovered {
            self.height.target = self.panel_height;
            self.last_frame = None;
        } else {
            self.set_hover(true);
        }
        // Keeps the island open (it has the keyboard) until clicked away.
        self.set_focus(Focus::Text);
        self.refresh_text_view();
    }

    fn text_op(&mut self, i: usize) {
        let op = text_tools::OPS[i];
        self.text_status = if self.text.transform(|part| text_tools::apply(op, part)) {
            op.label().to_string()
        } else {
            format!("«{}» не подходит к этому тексту", op.label())
        };
        self.refresh_text_view();
    }

    fn translate_text(&mut self) {
        if self.translating || self.text.text.trim().is_empty() {
            return;
        }
        self.translating = true;
        self.text_status = "Перевожу…".into();
        translate::spawn(self.hwnd, self.text.text.clone(), self.translate_slot.clone());
        self.refresh_text_view();
    }

    fn translated(&mut self) {
        self.translating = false;
        match self.translate_slot.lock().unwrap().take().flatten() {
            Some(t) => {
                self.text = TextField::with_text(&t, true);
                self.text_status = "Переведено".into();
            }
            None => self.text_status = "Перевод не удался — нет связи с сервисом".into(),
        }
        self.refresh_text_view();
    }

    fn refresh_text_view(&mut self) {
        let view = TextView {
            field: self.text.clone(),
            focus: self.focus == Focus::Text,
            stats: text_tools::stats(&self.text.text),
            status: self.text_status.clone(),
            busy: self.translating,
        };
        if Some(&view) != self.text_view.as_ref() {
            self.text_view = Some(view);
            self.dirty = true;
        }
    }

    fn control_arrived(&mut self) {
        if let Some(c) = &self.control {
            self.ctl = c.get();
        }
        self.refresh_control_view();
    }

    fn control_action(&mut self, row: usize) {
        let Some(c) = &self.control else { return };
        match self.ctl_segment {
            0 => {
                let Some(n) = self.ctl.networks.get(row).cloned() else { return };
                if n.connected {
                    c.send(control::Command::WifiDisconnect);
                } else if n.auth == control::Auth::Other {
                    control::open_settings("ms-settings:network-wifi");
                } else if n.saved || !n.secured {
                    c.send(control::Command::WifiConnect { ssid: n.ssid, password: None });
                } else {
                    // Ask for the password right in the row.
                    self.ctl_pw_ssid = Some(n.ssid);
                    self.ctl_pw.clear();
                    self.set_focus(Focus::Password);
                }
            }
            1 => {
                if let Some(d) = self.ctl.devices.get(row) {
                    c.send(control::Command::Bluetooth { address: d.address, connect: !d.connected });
                }
            }
            _ => {
                // Rows: [Throne], system proxy, then Windows VPNs.
                let throne_row = self.ctl.throne.installed as usize;
                if throne_row == 1 && row == 0 {
                    c.send(control::Command::ThroneOpen);
                    return;
                }
                if row == throne_row {
                    c.send(control::Command::Proxy(!self.ctl.throne.proxy_on));
                    self.refresh_control_view();
                    return;
                }
                let row = row - throne_row - 1;
                if let Some(v) = self.ctl.vpns.get(row) {
                    c.send(if v.connected { control::Command::VpnDisconnect(v.name.clone()) } else { control::Command::VpnConnect(v.name.clone()) });
                }
            }
        }
        self.refresh_control_view();
    }

    fn submit_password(&mut self) {
        let (Some(ssid), Some(c)) = (self.ctl_pw_ssid.take(), &self.control) else { return };
        let password = std::mem::take(&mut self.ctl_pw.text);
        self.ctl_pw.clear();
        c.send(control::Command::WifiConnect { ssid, password: Some(password) });
        self.set_focus(Focus::None);
        self.refresh_control_view();
    }

    /// Lock acts at once; sleep, restart and shutdown need a second click
    /// within 3 seconds.
    fn power_click(&mut self, i: usize) {
        let action = control::POWER[i];
        if action.needs_confirm() && self.power_confirm != Some(i) {
            self.power_confirm = Some(i);
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_CONFIRM, 3000, None);
            }
            self.refresh_control_view();
            return;
        }
        self.power_confirm = None;
        self.refresh_control_view();
        self.set_hover(false);
        control::power(action);
    }

    fn refresh_control_view(&mut self) {
        let ctl = &self.ctl;
        let (rows, empty): (Vec<CtlRow>, String) = match self.ctl_segment {
            0 => (
                ctl.networks
                    .iter()
                    .take(layout::CTL_ROWS)
                    .map(|n| CtlRow {
                        icon: CtlIcon::Wifi(control::bars(n.signal), n.secured),
                        title: n.ssid.clone(),
                        sub: if n.connected {
                            "Подключено".into()
                        } else if n.saved {
                            "Сохранена".into()
                        } else if n.secured {
                            "Защищена".into()
                        } else {
                            "Открытая".into()
                        },
                        action: Some(if n.connected {
                            "Отключить".into()
                        } else if n.auth == control::Auth::Other {
                            "Параметры".into()
                        } else {
                            "Подключить".into()
                        }),
                        active: n.connected,
                    })
                    .collect(),
                if ctl.wifi_present { "Ищу сети…".into() } else { "Нет адаптера Wi‑Fi".into() },
            ),
            1 => (
                ctl.devices
                    .iter()
                    .take(layout::CTL_ROWS)
                    .map(|d| {
                        let (icon, kind) = match d.kind {
                            control::DeviceKind::Audio => (CtlIcon::Headphones, "Аудио"),
                            control::DeviceKind::Input => (CtlIcon::Input, "Ввод"),
                            control::DeviceKind::Phone => (CtlIcon::Phone, "Телефон"),
                            control::DeviceKind::Other => (CtlIcon::Device, "Устройство"),
                        };
                        CtlRow {
                            icon,
                            title: d.name.clone(),
                            sub: format!("{kind} · {}", if d.connected { "подключено" } else { "не подключено" }),
                            action: (d.kind == control::DeviceKind::Audio).then(|| if d.connected { "Отключить".into() } else { "Подключить".into() }),
                            active: d.connected,
                        }
                    })
                    .collect(),
                "Нет сопряжённых устройств — добавьте их в «Параметрах»".into(),
            ),
            _ => (
                {
                    let t = &ctl.throne;
                    let mut rows = Vec::new();
                    if t.installed {
                        rows.push(CtlRow {
                            icon: CtlIcon::Vpn,
                            title: "Throne".into(),
                            sub: crate::throne::summary(t),
                            action: Some(if t.running { "Открыть".into() } else { "Запустить".into() }),
                            active: t.running && (t.tun_up || t.proxy_on),
                        });
                    }
                    rows.push(CtlRow {
                        icon: CtlIcon::Device,
                        title: "Системный прокси".into(),
                        sub: if t.proxy_on { format!("Включён · {}", t.proxy_server) } else { "Выключен".into() },
                        action: Some(if t.proxy_on { "Выключить".into() } else { "Включить".into() }),
                        active: t.proxy_on,
                    });
                    rows
                }
                .into_iter()
                .chain(ctl.vpns
                    .iter()
                    .map(|v| CtlRow {
                        icon: CtlIcon::Vpn,
                        title: v.name.clone(),
                        sub: if v.connected { "Подключён".into() } else { "Отключён".into() },
                        action: Some(if v.connected { "Отключить".into() } else { "Подключить".into() }),
                        active: v.connected,
                    }))
                .take(layout::CTL_ROWS)
                .collect(),
                "Нет VPN-подключений Windows — их добавляют в «Параметрах»".into(),
            ),
        };
        let password = match (&self.ctl_pw_ssid, self.ctl_segment) {
            (Some(ssid), 0) => ctl.networks.iter().take(layout::CTL_ROWS).position(|n| &n.ssid == ssid).map(|row| {
                // Shown as dots.
                let mut masked = self.ctl_pw.clone();
                masked.text = "•".repeat(self.ctl_pw.len());
                (row, masked, self.focus == Focus::Password)
            }),
            _ => None,
        };
        let view = ControlView { segment: self.ctl_segment, rows, empty, message: ctl.message.clone(), busy: ctl.busy, password, confirm: self.power_confirm };
        if Some(&view) != self.ctl_view.as_ref() {
            self.ctl_view = Some(view);
            self.dirty = true;
        }
    }

    fn since_midnight() -> u64 {
        let t = unsafe { GetLocalTime() };
        t.wHour as u64 * 3600 + t.wMinute as u64 * 60 + t.wSecond as u64
    }

    fn submit_reminder(&mut self) {
        let now = unix_now();
        if let Some((at, text)) = notes::parse(&self.remind.text, now, Self::since_midnight()) {
            self.store.add(Reminder { at, text });
            self.store.save();
            self.remind.clear();
            self.arm_reminders();
        }
        self.refresh_notes_view();
    }

    /// Wakes up at the next reminder (at most an hour ahead, then re-checks).
    fn arm_reminders(&self) {
        unsafe {
            match self.store.next_due() {
                Some(at) => {
                    let ms = at.saturating_sub(unix_now()).min(3600) * 1000 + 200;
                    SetTimer(Some(self.hwnd), TIMER_REMINDER, ms as u32, None);
                }
                None => {
                    let _ = KillTimer(Some(self.hwnd), TIMER_REMINDER);
                }
            }
        }
    }

    fn reminders_due(&mut self) {
        let due = self.store.take_due(unix_now());
        if !due.is_empty() {
            self.store.save();
            let text = if due.len() == 1 { due[0].text.clone() } else { format!("{} и ещё {}", due[0].text, due.len() - 1) };
            self.raise_alarm(Alarm::Reminder(text));
        }
        self.arm_reminders();
        self.refresh_notes_view();
    }

    fn timer_changed(&mut self) {
        unsafe {
            if let timer::State::Running { ends } = self.timer.state {
                let ms = ends.saturating_duration_since(Instant::now()).as_millis() as u32 + 30;
                SetTimer(Some(self.hwnd), TIMER_TIMER_END, ms, None);
                SetTimer(Some(self.hwnd), TIMER_COUNTDOWN, 1000, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_TIMER_END);
                let _ = KillTimer(Some(self.hwnd), TIMER_COUNTDOWN);
            }
        }
        self.countdown_tick();
    }

    /// Countdown text in the pill and the Notes tab.
    fn countdown_tick(&mut self) {
        let running = self.timer.running();
        let label = running.then(|| timer::clock(self.timer.left(Instant::now())));
        let width_change = label.is_some() != self.compact.timer.is_some();
        if label != self.compact.timer {
            self.compact.timer = label;
            self.dirty = true;
        }
        if width_change {
            self.retarget_collapsed();
        }
        self.refresh_notes_view();
    }

    fn timer_ended(&mut self) {
        if self.timer.check_done(Instant::now()) {
            self.timer_changed();
            self.raise_alarm(Alarm::Timer);
        } else {
            self.timer_changed(); // Fired early: re-arm for the rest.
        }
    }

    /// Shows an alarm in the pill until dismissed, beeps and bounces.
    fn raise_alarm(&mut self, alarm: Alarm) {
        let toast = match &alarm {
            Alarm::Timer => Toast {
                badge: Badge::Timer,
                title: "Время вышло".into(),
                text: format!("Таймер · {}", timer::length_label(self.timer.length)),
                level: None,
                actions: vec!["Ещё раз".into(), "✓".into()],
            },
            Alarm::Reminder(text) => Toast {
                badge: Badge::Bell,
                title: "Напоминание".into(),
                text: text.clone(),
                level: None,
                actions: vec!["+10 мин".into(), "✓".into()],
            },
            Alarm::Agent(id) => {
                let Some(card) = self.agent_cards.iter().find(|c| c.id == *id) else { return };
                let (title, text) = (card.msg.title.clone(), card.msg.text.clone());
                let mut actions = card.msg.options.clone();
                let widths = self.renderer.as_ref().and_then(|r| r.pill_text_widths(&title, &text));
                let shown_in_full = !agent::has_hidden(&title) && !agent::has_hidden(&text) && widths.is_some_and(|(a, b)| {
                    let room = self.agent_card_max_w() - self.agent_card_chrome(&actions);
                    a <= room && b <= room
                });
                if !shown_in_full {
                    // Allowing is only offered for what the card shows whole.
                    actions.retain(|a| a != "✓");
                }
                self.agent_text_w = widths.map_or(0.0, |(a, b)| a.max(b));
                Toast { badge: Badge::Text("AI".into()), title, text, level: None, actions }
            }
        };
        self.alarm = Some(alarm);
        self.alarm_since = Instant::now();
        if self.fullscreen {
            self.slide_in();
        }
        self.compact.toast = Some(toast);
        self.retarget_collapsed();
        self.wobble.velocity += 120.0;
        self.dirty = true;
        self.last_frame = None;
        self.alarm_pulses = 0;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_TOAST);
            SetTimer(Some(self.hwnd), TIMER_ALARM_PULSE, ALARM_PULSE_MS, None);
            let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(MB_ICONASTERISK);
        }
    }

    fn alarm_pulse(&mut self) {
        self.alarm_pulses += 1;
        if self.alarm.is_none() || self.alarm_pulses > ALARM_PULSES {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_ALARM_PULSE);
            }
            return;
        }
        if !self.hovered {
            // The island bristles until someone notices.
            self.wobble.velocity += 110.0;
            self.last_frame = None;
        }
        if self.alarm_pulses % 4 == 0 && self.alarm_pulses <= 8 {
            unsafe {
                let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(MB_ICONASTERISK);
            }
        }
    }

    fn dismiss_alarm(&mut self) {
        self.alarm = None;
        self.compact.toast = None;
        self.retarget_collapsed();
        self.dirty = true;
        self.last_frame = None;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ALARM_PULSE);
        }
    }

    /// Button `i` of the alarm in the pill.
    fn alarm_action(&mut self, i: usize) {
        let Some(alarm) = self.alarm.clone() else { return };
        if let Alarm::Agent(id) = alarm {
            if self.alarm_since.elapsed() < AGENT_CLICK_GUARD {
                return; // The card just changed under the cursor.
            }
            self.dismiss_alarm();
            if let Some(pos) = self.agent_cards.iter().position(|c| c.id == id) {
                let card = self.agent_cards.remove(pos);
                let _ = card.reply.send(Some(i));
            }
            self.next_agent_card();
            return;
        }
        let last = self.compact.toast.as_ref().map_or(0, |t| t.actions.len().saturating_sub(1));
        self.dismiss_alarm();
        if i == last {
            if alarm == Alarm::Timer {
                self.timer.reset();
                self.timer_changed();
            }
            self.next_agent_card();
            return;
        }
        match alarm {
            Alarm::Timer => {
                self.timer.start(self.timer.length, Instant::now());
                self.timer_changed();
            }
            Alarm::Reminder(text) => {
                self.store.add(Reminder { at: unix_now() + 600, text });
                self.store.save();
                self.arm_reminders();
                self.refresh_notes_view();
            }
            Alarm::Agent(_) => {}
        }
        self.next_agent_card();
    }

    /// Notices and cards from coding agents (island-hook.exe).
    fn agent_arrived(&mut self) {
        let Some(inbox) = self.agent_inbox.clone() else { return };
        for event in inbox.take() {
            match event {
                agent::Event::Notice(msg) => {
                    let toast = Toast { badge: Badge::Text("AI".into()), title: msg.title, text: msg.text, level: None, actions: Vec::new() };
                    self.show_toast(toast, AGENT_TOAST_MS);
                }
                agent::Event::Ask { id, msg, reply } => {
                    self.agent_cards.push(AgentCard { id, msg, reply });
                    self.next_agent_card();
                }
                agent::Event::Gone(id) => {
                    self.agent_cards.retain(|c| c.id != id);
                    if self.alarm == Some(Alarm::Agent(id)) {
                        self.dismiss_alarm();
                        self.next_agent_card();
                    }
                }
            }
        }
    }

    /// Shows the oldest waiting agent card unless another alarm is up.
    fn next_agent_card(&mut self) {
        if self.alarm.is_none() {
            if let Some(id) = self.agent_cards.first().map(|c| c.id) {
                self.raise_alarm(Alarm::Agent(id));
            }
        }
    }

    /// Alarm button under (x, y) client pixels while collapsed.
    fn toast_action_at(&self, x: i32, y: i32) -> Option<usize> {
        if self.hovered || self.alarm.is_none() {
            return None;
        }
        let t = self.compact.toast.as_ref()?;
        let shape = self.shape();
        let (lx, ly) = ((x as f32 - shape.left) / self.scale, (y as f32 - shape.top) / self.scale);
        layout::toast_actions(shape.width / self.scale, shape.height / self.scale, &t.actions)
            .iter()
            .position(|r| r.contains(lx, ly))
    }

    fn refresh_notes_view(&mut self) {
        let now = unix_now();
        let since = Self::since_midnight();
        let hint = if self.remind.is_empty() {
            "Например: «в 18:30 позвонить» и Enter".to_string()
        } else {
            match notes::parse(&self.remind.text, now, since) {
                Some((at, text)) => format!("Enter — «{text}» {}", notes::when_label(at, now, since)),
                None => "Не понял время — начните с «через» или «в»".to_string(),
            }
        };
        let preset = layout::TIMER_PRESETS.iter().position(|&p| Duration::from_secs(p) == self.timer.length);
        let view = NotesView {
            note: self.note.clone(),
            note_focus: self.focus == Focus::Note,
            remind: self.remind.clone(),
            remind_focus: self.focus == Focus::Remind,
            remind_hint: hint,
            reminders: self.store.reminders.iter().take(3).map(|r| (notes::when_label(r.at, now, since), r.text.clone())).collect(),
            timer_text: timer::clock(self.timer.left(Instant::now())),
            timer: match self.timer.state {
                timer::State::Idle => TimerUi::Idle,
                timer::State::Running { .. } => TimerUi::Running,
                timer::State::Paused { .. } => TimerUi::Paused,
                timer::State::Done => TimerUi::Done,
            },
            timer_preset: preset,
        };
        if Some(&view) != self.notes_view.as_ref() {
            self.notes_view = Some(view);
            self.dirty = true;
        }
    }

    /// Right-click menu for clipboard rows and dock apps.
    fn context_menu(&mut self, x: i32, y: i32) {
        let hit = self.hit_at(x, y);
        let items: &[(usize, PCWSTR)] = match hit {
            Some(Hit::Clip(_)) => &[(MENU_REMOVE, w!("Удалить из истории")), (MENU_CLEAR, w!("Очистить историю"))],
            Some(Hit::Dock(_)) => &[(MENU_REMOVE, w!("Убрать из дока"))],
            Some(Hit::ShelfRow(_)) => &[(MENU_REMOVE, w!("Убрать с полки")), (MENU_CLEAR, w!("Очистить полку"))],
            _ => return,
        };
        let cmd = unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            for (id, text) in items {
                let _ = AppendMenuW(menu, MF_STRING, *id, *text);
            }
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY, pt.x, pt.y, None, self.hwnd, None);
            let _ = DestroyMenu(menu);
            cmd.0 as usize
        };
        match (hit, cmd) {
            (Some(Hit::Clip(i)), MENU_REMOVE) => {
                if i < self.history.items.len() {
                    let item = self.history.items.remove(i);
                    if let Entry::Image { file, .. } = item.entry {
                        let _ = std::fs::remove_file(clip_history::dir().join(file));
                    }
                }
                self.history.save();
                self.refresh_clip_rows();
            }
            (Some(Hit::Clip(_)), MENU_CLEAR) => {
                for file in self.history.clear() {
                    let _ = std::fs::remove_file(clip_history::dir().join(file));
                }
                self.history.save();
                self.refresh_clip_rows();
            }
            (Some(Hit::ShelfRow(i)), MENU_REMOVE) => {
                if i < self.shelf.len() {
                    self.shelf.remove(i);
                }
                shelf::save(&self.shelf);
                self.refresh_shelf_rows();
            }
            (Some(Hit::ShelfRow(_)), MENU_CLEAR) => {
                self.shelf.clear();
                shelf::save(&self.shelf);
                self.refresh_shelf_rows();
            }
            (Some(Hit::Dock(i)), MENU_REMOVE) => {
                if let (Some(app), Some(list)) = (self.dock_apps.get(i), self.cfg.dock_apps.as_mut()) {
                    let entry = app.entry.clone();
                    list.retain(|e| *e != entry);
                }
                self.cfg.save();
                self.load_dock();
            }
            _ => {}
        }
    }

    fn load_dock(&mut self) {
        self.dock_apps = dock::load(self.cfg.dock_apps.as_deref().unwrap_or_default());
        self.dock_icons = self.dock_apps.iter().map(|a| DockIcon { id: a.id, icon: a.icon.clone() }).collect();
        self.hot = None;
        self.dirty = true;
    }

    fn refresh_clip_rows(&mut self) {
        let now = unix_now();
        let rows: Vec<ClipRow> = self
            .history
            .items
            .iter()
            .take(8)
            .map(|i| {
                let (glyph, text) = clip_history::label(&i.entry);
                ClipRow { glyph, text, ago: clip_history::ago(i.copied_at, now) }
            })
            .collect();
        let home: Vec<ClipRow> = rows.iter().take(3).cloned().collect();
        if home != self.clip_rows {
            self.clip_rows = home;
            self.dirty = true;
        }
        if rows != self.clip_page {
            self.clip_page = rows;
            self.dirty = true;
        }
    }

    /// Something new was copied anywhere in Windows.
    fn clipboard_changed(&mut self) {
        if std::mem::take(&mut self.ignore_next_clip) {
            return;
        }
        let Some(entry) = clipboard::read(self.hwnd) else { return };
        let (glyph, text) = clip_history::label(&entry);
        let quiet = std::mem::take(&mut self.quiet_next_clip);
        for file in self.history.push(entry, unix_now()) {
            let _ = std::fs::remove_file(clip_history::dir().join(file));
        }
        self.history.save();
        self.refresh_clip_rows();
        if !self.hovered && !quiet {
            self.show_toast(Toast { badge: Badge::Glyph(glyph), title: "Скопировано".into(), text, level: None, actions: Vec::new() }, TOAST_MS);
        }
    }

    /// Puts history row `i` back on the clipboard and moves it to the top.
    fn recopy(&mut self, i: usize) {
        let Some(item) = self.history.items.get(i).cloned() else { return };
        self.ignore_next_clip = clipboard::write(self.hwnd, &item.entry);
        self.history.push(item.entry, unix_now());
        self.history.save();
        self.refresh_clip_rows();
    }

    fn show_toast(&mut self, toast: Toast, ms: u32) {
        if self.fullscreen || self.hovered || self.alarm.is_some() {
            return; // Not over a full-screen app, the open panel, or an alarm.
        }
        // A new notice bounces the island; updates of the same one (volume
        // being dragged) just redraw.
        if self.compact.toast.is_none() {
            self.wobble.velocity += 50.0;
        }
        self.compact.toast = Some(toast);
        self.retarget_collapsed();
        self.dirty = true;
        self.last_frame = None;
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_TOAST, ms, None);
        }
    }

    /// Icon id for an exe, extracting it once.
    fn app_icon(&mut self, exe: &str) -> Option<u64> {
        if let Some((_, d)) = self.app_icons.iter().find(|(p, _)| p.eq_ignore_ascii_case(exe)) {
            return d.icon.as_ref().map(|_| d.id);
        }
        let d = DockIcon { id: dock::path_id(exe), icon: dock::icon(exe).ok() };
        let id = d.icon.as_ref().map(|_| d.id);
        if self.app_icons.len() >= 24 {
            self.app_icons.remove(0);
        }
        self.app_icons.push((exe.to_string(), d));
        self.extra_icons = self.app_icons.iter().map(|(_, d)| d.clone()).collect();
        id
    }

    fn set_active(&mut self, app: ActiveApp, announce: bool) {
        let switched = self.active.as_ref().is_none_or(|a| a.hwnd != app.hwnd);
        let icon = self.app_icon(&app.exe);
        let other_notice = self.compact.toast.as_ref().is_some_and(|t| !matches!(t.badge, Badge::App(_)));
        if announce && switched && self.cfg.show_active_app && !self.handle_mode() && !other_notice {
            let badge = icon.map_or(Badge::Glyph(clip_history::Glyph::Files), Badge::App);
            let text = if app.title != app.name { app.title.clone() } else { String::new() };
            self.show_toast(Toast { badge, title: app.name.clone(), text, level: None, actions: Vec::new() }, APP_TOAST_MS);
        }
        self.active = Some(app);
        self.refresh_window_view();
    }

    fn foreground_settled(&mut self) {
        match active_app::current() {
            Some(app) => self.set_active(app, true),
            None => {
                // Desktop/taskbar: keep the last app if its window still exists.
                if self.active.as_ref().is_some_and(|a| !active_app::alive(a)) {
                    self.active = None;
                    self.refresh_window_view();
                }
            }
        }
    }

    fn refresh_window_view(&mut self) {
        let view = self.active.as_ref().map(|a| WindowView {
            icon_id: Some(dock::path_id(&a.exe)).filter(|id| self.extra_icons.iter().any(|d| d.id == *id && d.icon.is_some())),
            name: a.name.clone(),
            title: a.title.clone(),
            topmost: a.topmost,
            opacity: (a.alpha as u32 * 100 + 127) / 255,
        });
        if view != self.window_view {
            self.window_view = view;
            self.dirty = true;
        }
    }

    fn volume_changed(&mut self, level: f32, muted: bool) {
        if !self.cfg.osd {
            return;
        }
        let (badge, title) = if muted || level <= 0.0 { (Badge::Muted, "Звук выключен") } else { (Badge::Volume, "Громкость") };
        self.show_toast(Toast { badge, title: title.into(), text: String::new(), level: Some(if muted { 0.0 } else { level }), actions: Vec::new() }, LEVEL_TOAST_MS);
    }

    fn brightness_changed(&mut self, percent: u32) {
        if !self.cfg.osd {
            return;
        }
        self.show_toast(Toast { badge: Badge::Brightness, title: "Яркость".into(), text: String::new(), level: Some(percent as f32 / 100.0), actions: Vec::new() }, LEVEL_TOAST_MS);
    }

    /// Announces a layout switch inside the same window (switching windows
    /// with different layouts is not news).
    fn check_layout(&mut self) {
        let now = signals::foreground_layout();
        let before = std::mem::replace(&mut self.layout, now);
        if now.0 == before.0 && now.1 != before.1 && now.1 != 0 {
            self.layout_changed(now.1);
        }
    }

    fn layout_changed(&mut self, hkl: isize) {
        if !self.cfg.osd {
            return;
        }
        let (code, name) = signals::layout_name(hkl);
        self.show_toast(Toast { badge: Badge::Text(code), title: name, text: "Раскладка клавиатуры".into(), level: None, actions: Vec::new() }, KEY_TOAST_MS);
    }

    fn caps_changed(&mut self, on: bool) {
        if !self.cfg.osd {
            return;
        }
        let title = if on { "Caps Lock включён" } else { "Caps Lock выключен" };
        let badge = Badge::Text(if on { "A" } else { "a" }.into());
        self.show_toast(Toast { badge, title: title.into(), text: String::new(), level: None, actions: Vec::new() }, KEY_TOAST_MS);
    }

    fn privacy_changed(&mut self) {
        let Some(now) = self.privacy.as_ref().map(|p| p.get()) else { return };
        let started_cam: Vec<String> = now.camera.iter().filter(|a| !self.in_use.camera.contains(a)).cloned().collect();
        let started_mic: Vec<String> = now.microphone.iter().filter(|a| !self.in_use.microphone.contains(a)).cloned().collect();
        self.in_use = now;
        self.dirty = true;
        self.last_frame = None;
        if let Some(app) = started_cam.first() {
            self.show_toast(Toast { badge: Badge::Camera, title: "Камера включена".into(), text: app.clone(), level: None, actions: Vec::new() }, PRIVACY_TOAST_MS);
        } else if let Some(app) = started_mic.first() {
            self.show_toast(Toast { badge: Badge::Mic, title: "Микрофон включён".into(), text: app.clone(), level: None, actions: Vec::new() }, PRIVACY_TOAST_MS);
        }
    }

    fn hide_toast(&mut self) {
        if self.alarm.is_some() {
            return; // Alarms stay until dismissed.
        }
        if self.compact.toast.take().is_some() {
            self.retarget_collapsed();
            self.dirty = true;
            self.last_frame = None;
        }
    }

    fn drag_volume(&mut self, x: i32, force_send: bool) {
        let Some(tile) = self.tiles.iter().find(|t| t.kind == TileKind::Media) else { return };
        let r = layout::media_regions(tile);
        let shape = self.shape();
        let (ox, _) = self.panel_origin(&shape);
        let v = layout::volume_at(&r.volume, (x as f32 - ox) / self.scale);
        if let Some(m) = self.media.as_mut() {
            m.volume = Some(v);
        }
        self.refresh_media_view();
        let due = self.last_volume_sent.is_none_or(|t| t.elapsed() >= VOLUME_THROTTLE);
        if force_send || due {
            if let Some(svc) = &self.media_service {
                svc.send(Command::SetVolume(v));
            }
            self.last_volume_sent = Some(Instant::now());
        }
    }

    /// Whether (x, y) client pixels are inside the island's current shape.
    fn inside(&self, x: i32, y: i32) -> bool {
        self.region.is_some_and(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
    }

    fn shape(&self) -> Shape {
        let s = self.scale;
        let width = self.width.value * s;
        let height = self.height.value * s;
        Shape {
            left: (self.win_size.0 as f32 - width) / 2.0,
            // Slides up out of view when hidden for a full-screen app.
            // The thin handle hugs the screen edge; the pill floats below it.
            top: (self.top_gap.value
                - self.hide.value / 100.0 * (self.cfg.collapsed_height + self.cfg.top_offset + 4.0))
                * s,
            width,
            height,
            radius: (self.radius.value * s).min(height / 2.0),
        }
    }

    /// Top-left of the Home panel in pixels. Anchored to the island's
    /// top center so tiles ride along while it grows.
    fn panel_origin(&self, shape: &Shape) -> (f32, f32) {
        let panel_w = self.cfg.expanded_width * self.scale;
        (shape.left + (shape.width - panel_w) / 2.0, shape.top)
    }

    fn tick_clock(&mut self) {
        self.refresh_clip_rows();
        let time = clock_text();
        if time != self.compact.time {
            self.compact.time = time;
            self.dirty = true;
        }
    }

    fn weather_arrived(&mut self) {
        let w = self.weather.as_ref().and_then(|s| s.get());
        let compact = w.map(|w| (w.icon, w.is_day, w.temperature_label()));
        if compact != self.compact.weather {
            self.compact.weather = compact;
            self.dirty = true;
        }
    }

    /// 1 while collapsed, fading to 0 early in the expansion.
    fn compact_alpha(&self) -> f32 {
        let span = self.panel_height - self.cfg.collapsed_height;
        if span <= 0.0 {
            return if self.hovered { 0.0 } else { 1.0 };
        }
        let p = (self.height.value - self.cfg.collapsed_height) / span;
        (1.0 - p * 2.5).clamp(0.0, 1.0) * self.pill_amount().powi(2)
    }

    /// 0 while mostly collapsed, 1 when (nearly) fully expanded.
    fn content_alpha(&self) -> f32 {
        let span = self.panel_height - self.cfg.collapsed_height;
        if span <= 0.0 {
            return if self.hovered { 1.0 } else { 0.0 };
        }
        let p = (self.height.value - self.cfg.collapsed_height) / span;
        (p * 1.6 - 0.6).clamp(0.0, 1.0)
    }

    fn hover_at(&mut self, x: i32, y: i32) {
        let shape = self.shape();
        let (ox, oy) = self.panel_origin(&shape);
        let (dx, dy) = ((x as f32 - ox) / self.scale, (y as f32 - oy) / self.scale);
        let tile = if self.content_alpha() > 0.5 { self.tiles.iter().position(|t| t.contains(dx, dy)) } else { None };
        if tile != self.hovered_tile {
            self.hovered_tile = tile;
            self.dirty = true;
        }
        let hot = self.hit_at(x, y);
        if hot != self.hot {
            self.hot = hot;
            self.dirty = true;
        }
    }

    fn update_region(&mut self, shape: &Shape) {
        let wobble_px = (self.wobble.value.abs() * self.scale).ceil() as i32;
        let rect = RECT {
            left: shape.left.floor() as i32,
            top: shape.top.floor() as i32,
            right: (shape.left + shape.width).ceil() as i32,
            bottom: (shape.top + shape.height).ceil() as i32 + wobble_px + 1,
        };
        if self.region == Some(rect) {
            return;
        }
        self.region = Some(rect);
        unsafe {
            let rgn = CreateRectRgn(rect.left, rect.top, rect.right, rect.bottom);
            // The system owns the region after this call.
            SetWindowRgn(self.hwnd, Some(rgn), false);
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = self.last_frame.map_or(1.0 / 60.0, |t| (now - t).as_secs_f32()).min(1.0 / 30.0);
        self.last_frame = Some(now);
        self.width.step(dt);
        self.height.step(dt);
        self.radius.step(dt);
        self.wobble.step(dt);
        self.hide.step(dt);
        self.top_gap.step(dt);
        if !self.wobble.is_settled() {
            self.phase = (self.phase + dt * WAVE_SPEED) % std::f32::consts::TAU;
        }
        self.dirty = false;

        let shape = self.shape();
        self.update_region(&shape);
        let scene = Scene {
            shape,
            wobble: self.wobble.value * self.scale,
            phase: self.phase,
            theme: &self.theme,
            content_alpha: self.content_alpha(),
            origin: self.panel_origin(&shape),
            scale: self.scale,
            tiles: &self.tiles,
            hovered: self.hovered_tile,
            panel_size: (self.cfg.expanded_width, self.panel_height),
            compact: &self.compact,
            compact_alpha: self.compact_alpha(),
            compact_size: (self.collapsed_width(), self.cfg.collapsed_height),
            media: self.media_view.as_ref(),
            cover: self.media.as_ref().and_then(|m| m.cover.as_ref()),
            relaxed: self.width.is_settled() && self.height.is_settled() && self.radius.is_settled(),
            clip_rows: &self.clip_rows,
            dock: &self.dock_icons,
            hot: self.hot,
            // The handle shows the accent while music plays.
            tint: (self.docked && self.media.as_ref().is_some_and(|m| m.playing))
                .then(|| (self.theme.accent, 1.0 - self.pill_amount())),
            extra_icons: &self.extra_icons,
            privacy: (!self.in_use.camera.is_empty(), !self.in_use.microphone.is_empty()),
            window: self.window_view.as_ref(),
            tab: self.tab,
            notes: self.notes_view.as_ref(),
            clip_page: &self.clip_page,
            search: self.search_view.as_ref(),
            shelf: &self.shelf_rows,
            text: self.text_view.as_ref(),
            control: self.ctl_view.as_ref(),
        };
        let started = Instant::now();
        let (failed, rebuilt) = match self.renderer.as_mut().map(|r| r.draw(&scene)) {
            Some(Ok(rebuilt)) => (false, rebuilt),
            _ => (true, false),
        };
        self.stats.frame(started.elapsed(), rebuilt);
        if failed {
            // Device lost (driver update, GPU reset): rebuild on the next frame.
            self.renderer = Renderer::new(self.hwnd, self.win_size.0 as u32, self.win_size.1 as u32, self.cfg.gpu).ok();
            self.dirty = true;
        }
        if self.hide.target == 100.0 && self.hide.is_settled() && !self.window_hidden {
            // Fully out of view: take the window off the screen entirely so
            // nothing sits over the full-screen app.
            self.window_hidden = true;
            crate::stats::log_line("window hidden");
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
        if !self.is_animating() {
            self.last_frame = None;
            self.stats.settled(if self.hovered { "expand" } else { "collapse" });
            if !self.hovered {
                // Re-arming resets the countdown, so it fires only once the
                // island has stayed still for the whole delay.
                unsafe {
                    SetTimer(Some(self.hwnd), TIMER_TRIM, TRIM_AFTER_IDLE_MS, None);
                }
            }
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Local time as "HH:MM".
fn clock_text() -> String {
    let t = unsafe { GetLocalTime() };
    format!("{:02}:{:02}", t.wHour, t.wMinute)
}

/// Milliseconds until just past the next minute boundary.
fn ms_to_next_minute() -> u32 {
    let t = unsafe { GetLocalTime() };
    let into_minute = t.wSecond as u32 * 1000 + t.wMilliseconds as u32;
    60_000 - into_minute.min(59_999) + 20
}

/// One wake-up per minute, aligned to the minute, instead of polling.
fn arm_clock(hwnd: HWND) {
    unsafe {
        SetTimer(Some(hwnd), TIMER_CLOCK, ms_to_next_minute(), None);
    }
}

fn mouse_pos(lparam: LPARAM) -> (i32, i32) {
    ((lparam.0 & 0xFFFF) as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => {
            // Only text fields take the keyboard; everything else leaves the
            // user's window active.
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = ScreenToClient(hwnd, &mut pt);
            let field = with_island(|i| matches!(i.hit_at(pt.x, pt.y), Some(Hit::NoteField | Hit::RemindField | Hit::SearchField | Hit::TextField | Hit::PasswordField)));
            return LRESULT(if field == Some(true) { MA_ACTIVATE } else { MA_NOACTIVATE } as isize);
        }
        WM_ACTIVATE => {
            if wparam.0 & 0xFFFF == 0 {
                // Lost activation (WA_INACTIVE): stop typing.
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                let outside = with_island(|i| {
                    if i.tab == Tab::Search {
                        i.close_search();
                    } else {
                        i.set_focus(Focus::None);
                    }
                    !i.inside(pt.x, pt.y)
                });
                if outside == Some(true) {
                    SetTimer(Some(hwnd), TIMER_COLLAPSE, COLLAPSE_DELAY_MS, None);
                }
            }
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            if with_island(|i| i.key_down(wparam.0 as u16)) == Some(true) {
                return LRESULT(0);
            }
        }
        WM_CHAR => {
            with_island(|i| i.char_input(wparam.0 as u16));
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_NOTE_SAVE => {
            let _ = KillTimer(Some(hwnd), TIMER_NOTE_SAVE);
            with_island(|i| i.save_note());
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_COUNTDOWN => {
            with_island(|i| i.countdown_tick());
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_TIMER_END => {
            let _ = KillTimer(Some(hwnd), TIMER_TIMER_END);
            with_island(|i| i.timer_ended());
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_REMINDER => {
            let _ = KillTimer(Some(hwnd), TIMER_REMINDER);
            with_island(|i| i.reminders_due());
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_ALARM_PULSE => {
            with_island(|i| i.alarm_pulse());
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let _ = KillTimer(Some(hwnd), TIMER_COLLAPSE);
            if !TRACKING_LEAVE.with(|t| t.replace(true)) {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
            }
            let (x, y) = mouse_pos(lparam);
            let drag = with_island(|i| match i.shelf_press {
                Some((row, px, py)) if (x - px).abs() > 6 || (y - py).abs() > 6 => {
                    i.shelf_press = None;
                    i.shelf.get(row).cloned()
                }
                _ => None,
            })
            .flatten();
            if let Some(path) = drag {
                let _ = ReleaseCapture();
                shelf::drag_out(&path);
                return LRESULT(0);
            }
            with_island(|i| {
                if i.dragging_volume {
                    i.drag_volume(x, false);
                    return;
                }
                if i.hovered {
                    i.hover_at(x, y);
                } else if i.alarm.is_some() {
                    // Buttons in the pill: a click opens the panel instead.
                } else if i.cfg.hover_delay_ms == 0 {
                    i.set_hover(true);
                    i.hover_at(x, y);
                } else if !DWELLING.with(|d| d.replace(true)) {
                    // Expand only if the cursor stays: passing by does nothing.
                    SetTimer(Some(hwnd), TIMER_DWELL, i.cfg.hover_delay_ms, None);
                }
            });
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_DWELL => {
            let _ = KillTimer(Some(hwnd), TIMER_DWELL);
            DWELLING.with(|d| d.set(false));
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = ScreenToClient(hwnd, &mut pt);
            with_island(|i| {
                crate::stats::log_line(&format!("dwell fired at {},{} inside={} region={:?}", pt.x, pt.y, i.inside(pt.x, pt.y), i.region));
                if i.inside(pt.x, pt.y) {
                    i.set_hover(true);
                    i.hover_at(pt.x, pt.y);
                }
            });
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_DOCK => {
            with_island(|i| {
                i.check_docked();
                i.check_layout();
            });
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            TRACKING_LEAVE.with(|t| t.set(false));
            let _ = KillTimer(Some(hwnd), TIMER_DWELL);
            DWELLING.with(|d| d.set(false));
            // Dragging the volume knob past the edge must not collapse.
            if with_island(|i| i.dragging_volume) != Some(true) {
                SetTimer(Some(hwnd), TIMER_COLLAPSE, COLLAPSE_DELAY_MS, None);
            }
            return LRESULT(0);
        }
        WM_RBUTTONUP => {
            let (x, y) = mouse_pos(lparam);
            with_island(|i| i.context_menu(x, y));
            return LRESULT(0);
        }
        WM_CLIPBOARDUPDATE => {
            with_island(|i| i.clipboard_changed());
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_TOAST => {
            let _ = KillTimer(Some(hwnd), TIMER_TOAST);
            with_island(|i| i.hide_toast());
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            let (x, y) = mouse_pos(lparam);
            // Alarm buttons work right in the collapsed pill.
            if let Some(i) = with_island(|i| i.toast_action_at(x, y)).flatten() {
                with_island(|is| is.alarm_action(i));
                return LRESULT(0);
            }
            // A click on the collapsed island opens it without waiting.
            if with_island(|i| {
                let opening = !i.hovered;
                if opening {
                    i.set_hover(true);
                }
                opening
            }) == Some(true)
            {
                let _ = KillTimer(Some(hwnd), TIMER_DWELL);
                DWELLING.with(|d| d.set(false));
                return LRESULT(0);
            }
            if with_island(|i| i.click(x, y)) == Some(true) {
                SetCapture(hwnd);
            }
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            let (x, y) = mouse_pos(lparam);
            if let Some(path) = with_island(|i| i.shelf_press.take().and_then(|(row, _, _)| i.shelf.get(row).cloned())).flatten() {
                let _ = ReleaseCapture();
                dock::launch(&path);
                return LRESULT(0);
            }
            let was_dragging = with_island(|i| {
                let was = i.dragging_volume;
                if was {
                    i.drag_volume(x, true);
                    i.dragging_volume = false;
                }
                (was, i.inside(x, y))
            });
            if let Some((true, inside)) = was_dragging {
                let _ = ReleaseCapture();
                if !inside {
                    SetTimer(Some(hwnd), TIMER_COLLAPSE, COLLAPSE_DELAY_MS, None);
                }
            }
            return LRESULT(0);
        }
        WM_SETCURSOR => {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = ScreenToClient(hwnd, &mut pt);
            if with_island(|i| i.is_clickable(pt.x, pt.y)) == Some(true) {
                if let Ok(hand) = LoadCursorW(None, IDC_HAND) {
                    SetCursor(Some(hand));
                    return LRESULT(1);
                }
            }
        }
        WM_TIMER if wparam.0 == TIMER_MEDIA => {
            with_island(|i| i.refresh_media_view());
            return LRESULT(0);
        }
        WM_MEDIA => {
            with_island(|i| i.media_arrived());
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_TRIM => {
            let _ = KillTimer(Some(hwnd), TIMER_TRIM);
            with_island(|i| {
                if !i.hovered && !i.is_animating() {
                    if let Some(r) = i.renderer.as_mut() {
                        r.trim();
                    }
                }
            });
            return LRESULT(0);
        }
        WM_BEAT => {
            let strength = wparam.0 as f32 / 1000.0;
            with_island(|i| i.on_beat(strength));
            return LRESULT(0);
        }
        WM_LYRICS => {
            with_island(|i| i.lyrics_arrived());
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_LYRIC => {
            let _ = KillTimer(Some(hwnd), TIMER_LYRIC);
            with_island(|i| i.lyric_tick());
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_COLLAPSE => {
            let _ = KillTimer(Some(hwnd), TIMER_COLLAPSE);
            // set_hover keeps the island open while a field has the keyboard.
            with_island(|i| i.set_hover(false));
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_EDGE => {
            with_island(|i| i.edge_tick());
            return LRESULT(0);
        }
        WM_APPBAR if wparam.0 == ABN_FULLSCREENAPP => {
            with_island(|i| i.check_fullscreen());
            return LRESULT(0);
        }
        WM_FOREGROUND => {
            with_island(|i| {
                i.check_fullscreen();
                i.check_docked();
            });
            SetTimer(Some(hwnd), TIMER_APP, APP_SETTLE_MS, None);
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_APP => {
            let _ = KillTimer(Some(hwnd), TIMER_APP);
            with_island(|i| i.foreground_settled());
            return LRESULT(0);
        }
        WM_VOLUME => {
            let level = wparam.0 as f32 / 1000.0;
            let muted = lparam.0 != 0;
            with_island(|i| i.volume_changed(level, muted));
            return LRESULT(0);
        }
        WM_POWERBROADCAST if wparam.0 as u32 == PBT_POWERSETTINGCHANGE => {
            if let Some(pct) = signals::brightness_from(lparam) {
                with_island(|i| {
                    // The first report after registering is just the current value.
                    let primed = i.signals.as_ref().is_some_and(|s| s.brightness_primed);
                    if let Some(s) = i.signals.as_mut() {
                        s.brightness_primed = true;
                    }
                    if primed {
                        i.brightness_changed(pct);
                    }
                });
            }
            return LRESULT(1);
        }
        WM_INPUT => {
            match with_island(|i| i.signals.as_mut().and_then(|s| s.raw_input(lparam))).flatten() {
                Some(signals::KeyEvent::Caps(on)) => {
                    with_island(|i| i.caps_changed(on));
                }
                Some(signals::KeyEvent::LayoutKeyUp) => {
                    SetTimer(Some(hwnd), TIMER_LAYOUT, LAYOUT_CHECK_MS, None);
                }
                None => {}
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_TIMER if wparam.0 == TIMER_LAYOUT => {
            let _ = KillTimer(Some(hwnd), TIMER_LAYOUT);
            with_island(|i| i.check_layout());
            return LRESULT(0);
        }
        WM_PRIVACY => {
            with_island(|i| i.privacy_changed());
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_CLOCK => {
            with_island(|i| i.tick_clock());
            arm_clock(hwnd);
            return LRESULT(0);
        }
        WM_TIMECHANGE => {
            with_island(|i| i.tick_clock());
            arm_clock(hwnd);
            return LRESULT(0);
        }
        WM_POWERBROADCAST if wparam.0 as u32 == PBT_APMRESUMEAUTOMATIC => {
            // Back from sleep: the clock and the weather are both stale.
            with_island(|i| {
                i.tick_clock();
                if let Some(w) = &i.weather {
                    w.refresh();
                }
            });
            arm_clock(hwnd);
            return LRESULT(1);
        }
        WM_WEATHER => {
            with_island(|i| i.weather_arrived());
            return LRESULT(0);
        }
        WM_HOTKEY if wparam.0 as i32 == HOTKEY_LAUNCHER => {
            with_island(|i| if i.tab == Tab::Search { i.close_search() } else { i.open_search() });
            return LRESULT(0);
        }
        WM_CONTROL => {
            with_island(|i| i.control_arrived());
            return LRESULT(0);
        }
        WM_AGENT => {
            with_island(|i| i.agent_arrived());
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_CONFIRM => {
            let _ = KillTimer(Some(hwnd), TIMER_CONFIRM);
            with_island(|i| {
                i.power_confirm = None;
                i.refresh_control_view();
            });
            return LRESULT(0);
        }
        WM_OVERLAY_DONE => {
            with_island(|i| i.overlay_done());
            return LRESULT(0);
        }
        WM_OCR => {
            with_island(|i| i.ocr_done());
            return LRESULT(0);
        }
        WM_TRANSLATED => {
            with_island(|i| i.translated());
            return LRESULT(0);
        }
        WM_SHELF_DRAG => {
            let over = wparam.0 == 1;
            with_island(|i| i.shelf_drag(over));
            return LRESULT(0);
        }
        WM_SHELF_DROP => {
            with_island(|i| i.shelf_dropped());
            return LRESULT(0);
        }
        WM_INDEX => {
            with_island(|i| {
                if i.tab == Tab::Search {
                    i.run_search();
                }
            });
            return LRESULT(0);
        }
        WM_HOTKEY if wparam.0 as i32 == HOTKEY_THEME => {
            with_island(|i| i.toggle_theme());
            return LRESULT(0);
        }
        WM_SETTINGCHANGE => {
            // Windows switched between light and dark app mode.
            let area = PCWSTR(lparam.0 as *const u16);
            if !area.is_null() && area.to_string().is_ok_and(|s| s == "ImmersiveColorSet") {
                with_island(|i| {
                    i.theme = Theme::resolve(&i.cfg);
                    i.dirty = true;
                });
            }
        }
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            with_island(|i| {
                i.layout();
                i.check_fullscreen();
            });
            return LRESULT(0);
        }
        tray::WM_TRAY => {
            match tray::on_message(hwnd, (lparam.0 & 0xFFFF) as u32) {
                TrayAction::Exit => {
                    let _ = DestroyWindow(hwnd);
                }
                TrayAction::Reload => {
                    with_island(|i| i.reload(Config::load()));
                }
                TrayAction::None => {}
            }
            return LRESULT(0);
        }
        WM_DESTROY => {
            clipboard::stop_listening(hwnd);
            with_island(|i| i.shelf_target = None);
            with_island(|i| i.save_note());
            with_island(|i| {
                if let Some(mut s) = i.signals.take() {
                    s.stop();
                }
            });
            with_island(|i| i.fs_watcher = None);
            let _ = UnregisterHotKey(Some(hwnd), HOTKEY_THEME);
            let _ = UnregisterHotKey(Some(hwnd), HOTKEY_LAUNCHER);
            tray::remove(hwnd);
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {}
    }
    if msg != 0 && msg == TASKBAR_CREATED.with(|c| c.get()) {
        // Explorer restarted: the tray icon has to be added again.
        tray::add(hwnd);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
