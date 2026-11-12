//! Tile layout for the "Home" view: a 4-column skyline packer. Each tile
//! has a fixed span; removing tiles lets the rest move up and the panel
//! shrinks to fit what is left.

pub const COLS: usize = 4;
pub const PAD: f32 = 14.0;
pub const GAP: f32 = 10.0;
pub const ROW_H: f32 = 64.0;
/// Panel height when the Home view has no tiles at all.
pub const EMPTY_HEIGHT: f32 = 96.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileKind {
    Media,
    Clipboard,
    Tools,
    Weather,
    Apps,
    /// The active window with quick actions.
    Window,
}

impl TileKind {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id.trim().to_ascii_lowercase().as_str() {
            "media" => Self::Media,
            "clipboard" => Self::Clipboard,
            "tools" => Self::Tools,
            "weather" => Self::Weather,
            "apps" => Self::Apps,
            "window" => Self::Window,
            _ => return None,
        })
    }

    /// (columns, rows) the tile occupies.
    pub fn span(self) -> (usize, usize) {
        match self {
            Self::Media => (2, 2),
            Self::Clipboard => (2, 2),
            Self::Tools => (4, 1),
            Self::Weather => (2, 1),
            Self::Apps => (4, 1),
            Self::Window => (4, 1),
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Media => "Музыка",
            Self::Clipboard => "Недавно скопировано",
            Self::Tools => "Инструменты",
            Self::Weather => "Погода",
            Self::Apps => "Приложения",
            Self::Window => "Окно",
        }
    }
}

/// A placed tile, in DIPs relative to the panel's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub kind: TileKind,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Placed {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Parses tile ids from the config, skipping unknown ids and duplicates.
pub fn tiles_from_ids(ids: &[String]) -> Vec<TileKind> {
    let mut out = Vec::new();
    for kind in ids.iter().filter_map(|id| TileKind::from_id(id)) {
        if !out.contains(&kind) {
            out.push(kind);
        }
    }
    out
}

/// Packs `tiles` into a panel `width` DIPs wide. Returns the placed tiles
/// and the panel height.
pub fn pack(tiles: &[TileKind], width: f32) -> (Vec<Placed>, f32) {
    if tiles.is_empty() {
        return (Vec::new(), EMPTY_HEIGHT);
    }
    let col_w = (width - PAD * 2.0 - GAP * (COLS as f32 - 1.0)) / COLS as f32;
    let mut heights = [0usize; COLS]; // in rows
    let mut placed = Vec::with_capacity(tiles.len());

    for &kind in tiles {
        let (cols, rows) = kind.span();
        let cols = cols.min(COLS);
        // Leftmost position whose highest covered column is lowest.
        let (col, row) = (0..=COLS - cols)
            .map(|c| (c, *heights[c..c + cols].iter().max().unwrap()))
            .min_by_key(|&(_, top)| top)
            .unwrap();
        for h in &mut heights[col..col + cols] {
            *h = row + rows;
        }
        placed.push(Placed {
            kind,
            x: PAD + col as f32 * (col_w + GAP),
            y: PAD + row as f32 * (ROW_H + GAP),
            w: cols as f32 * col_w + (cols as f32 - 1.0) * GAP,
            h: rows as f32 * ROW_H + (rows as f32 - 1.0) * GAP,
        });
    }

    let total_rows = *heights.iter().max().unwrap();
    let height = PAD * 2.0 + total_rows as f32 * ROW_H + (total_rows as f32 - 1.0) * GAP;
    (placed, height)
}

/// Axis-aligned rectangle in DIPs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Parts of the player tile, shared by drawing and hit-testing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaRegions {
    pub cover: Rect,
    pub title: Rect,
    pub artist: Rect,
    pub prev: Rect,
    pub play: Rect,
    pub next: Rect,
    pub progress: Rect,
    pub times: Rect,
    pub speaker: Rect,
    /// The visible volume track; `volume_hit` is taller for easy grabbing.
    pub volume: Rect,
    pub volume_hit: Rect,
    pub like: Rect,
    pub progress_hit: Rect,
}

pub fn media_regions(tile: &Placed) -> MediaRegions {
    let (x, y, w) = (tile.x, tile.y, tile.w);
    let text_x = x + 88.0;
    let text_w = (x + w - 12.0) - text_x;
    let cx = text_x + text_w / 2.0;
    let button = |dx: f32| Rect {
        x: cx + dx - 14.0,
        y: y + 54.0,
        w: 28.0,
        h: 28.0,
    };
    let volume = Rect {
        x: x + 32.0,
        y: y + 122.0,
        w: w - 44.0,
        h: 4.0,
    };
    MediaRegions {
        cover: Rect {
            x: x + 12.0,
            y: y + 12.0,
            w: 64.0,
            h: 64.0,
        },
        title: Rect {
            x: text_x,
            y: y + 12.0,
            w: text_w,
            h: 20.0,
        },
        artist: Rect {
            x: text_x,
            y: y + 32.0,
            w: text_w,
            h: 18.0,
        },
        prev: button(-38.0),
        play: button(0.0),
        next: button(38.0),
        progress: Rect {
            x: x + 12.0,
            y: y + 90.0,
            w: w - 24.0,
            h: 4.0,
        },
        times: Rect {
            x: x + 12.0,
            y: y + 97.0,
            w: w - 24.0,
            h: 14.0,
        },
        speaker: Rect {
            x: x + 12.0,
            y: y + 117.0,
            w: 14.0,
            h: 14.0,
        },
        volume,
        volume_hit: Rect {
            x: volume.x - 6.0,
            y: volume.y - 8.0,
            w: volume.w + 12.0,
            h: volume.h + 16.0,
        },
        like: Rect {
            x: x + w - 36.0,
            y: y + 54.0,
            w: 24.0,
            h: 28.0,
        },
        progress_hit: Rect {
            x: x + 12.0,
            y: y + 84.0,
            w: w - 24.0,
            h: 16.0,
        },
    }
}

/// Something clickable on the Home view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    MediaPrev,
    MediaPlay,
    MediaNext,
    MediaVolume,
    MediaLike,
    MediaSeek,
    MediaCopy,
    /// Row `n` of the clipboard history tile.
    Clip(usize),
    /// App `n` of the dock.
    Dock(usize),
    DockAdd,
    WinPin,
    WinOpacity,
    WinMinimize,
    WinActivate,
    WinMenu,
    Tab(Tab),
    NoteField,
    RemindField,
    /// Timer preset `n` of TIMER_PRESETS.
    TimerPreset(usize),
    TimerToggle,
    TimerReset,
    /// Remove reminder row `n`.
    RemindRemove(usize),
    SearchField,
    /// File `n` on the shelf.
    ShelfRow(usize),
    /// Button `n` of the Tools tile (see TOOLS).
    Tool(usize),
    TextField,
    /// Transform `n` of text_tools::OPS.
    TextOp(usize),
    TextTranslate,
    TextCopy,
    /// Control tab: segment (Wi-Fi, Bluetooth, VPN).
    CtlSegment(usize),
    /// The action button of list row `n`.
    CtlAction(usize),
    CtlRefresh,
    CtlSettings,
    PasswordField,
    PasswordGo,
    /// Power button `n` of control::POWER.
    Power(usize),
    /// Search result `n`.
    SearchRow(usize),
}

/// Views of the open island, shown as a row of tabs on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Home,
    Clipboard,
    Notes,
    Shelf,
    Text,
    Control,
    /// Alt+Space search; not in the tab row.
    Search,
}

pub const TABS: [Tab; 6] = [
    Tab::Home,
    Tab::Clipboard,
    Tab::Notes,
    Tab::Shelf,
    Tab::Text,
    Tab::Control,
];

impl Tab {
    pub fn title(self) -> &'static str {
        match self {
            Tab::Home => "Главная",
            Tab::Clipboard => "Буфер",
            Tab::Notes => "Заметки",
            Tab::Shelf => "Полка",
            Tab::Text => "Текст",
            Tab::Control => "Управление",
            Tab::Search => "Поиск",
        }
    }
}

/// Height of the tab row; tab content starts below it.
pub const TAB_BAR_H: f32 = 44.0;
/// Content height of the Clipboard and Notes tabs.
pub const PAGE_H: f32 = 286.0;

/// Tab buttons: the active one is wide enough for its label.
pub fn tab_buttons(active: Tab) -> Vec<(Tab, Rect)> {
    let mut x = PAD;
    TABS.iter()
        .map(|&t| {
            let w = if t == active { 100.0 } else { 34.0 };
            let r = Rect {
                x,
                y: 10.0,
                w,
                h: 28.0,
            };
            x += w + 6.0;
            (t, r)
        })
        .collect()
}

pub const TIMER_PRESETS: [u64; 4] = [60, 5 * 60, 10 * 60, 25 * 60];

/// Parts of the Notes tab, for a panel `width` DIPs wide.
#[derive(Debug, Clone, PartialEq)]
pub struct NotesRegions {
    pub note_card: Rect,
    pub note: Rect,
    pub timer_card: Rect,
    pub timer_text: Rect,
    pub toggle: Rect,
    pub reset: Rect,
    pub presets: Vec<Rect>,
    pub remind_card: Rect,
    pub remind: Rect,
    pub remind_hint: Rect,
    pub rows: Vec<Rect>,
    pub removes: Vec<Rect>,
}

pub fn notes_regions(width: f32) -> NotesRegions {
    let top = TAB_BAR_H;
    let inner = width - PAD * 2.0;
    let left_w = (inner - GAP) * 0.52;
    let note_card = Rect {
        x: PAD,
        y: top,
        w: left_w,
        h: PAGE_H - PAD,
    };
    let rx = PAD + left_w + GAP;
    let rw = inner - left_w - GAP;
    let timer_card = Rect {
        x: rx,
        y: top,
        w: rw,
        h: 116.0,
    };
    let b = 30.0;
    let reset = Rect {
        x: rx + rw - 12.0 - b,
        y: top + 34.0,
        w: b,
        h: b,
    };
    let toggle = Rect {
        x: reset.x - 8.0 - b,
        ..reset
    };
    let pw = (rw - 24.0 - 3.0 * 6.0) / 4.0;
    let presets = (0..4)
        .map(|i| Rect {
            x: rx + 12.0 + i as f32 * (pw + 6.0),
            y: top + 78.0,
            w: pw,
            h: 26.0,
        })
        .collect();
    let remind_card = Rect {
        x: rx,
        y: top + 116.0 + GAP,
        w: rw,
        h: PAGE_H - PAD - 116.0 - GAP,
    };
    let ry = remind_card.y;
    let rows: Vec<Rect> = (0..3)
        .map(|i| Rect {
            x: rx + 6.0,
            y: ry + 74.0 + i as f32 * 22.0,
            w: rw - 12.0,
            h: 22.0,
        })
        .collect();
    let removes = rows
        .iter()
        .map(|r| Rect {
            x: r.x + r.w - 24.0,
            y: r.y,
            w: 24.0,
            h: r.h,
        })
        .collect();
    NotesRegions {
        note: Rect {
            x: PAD + 12.0,
            y: top + 32.0,
            w: left_w - 24.0,
            h: PAGE_H - PAD - 44.0,
        },
        note_card,
        timer_text: Rect {
            x: rx + 12.0,
            y: top + 28.0,
            w: rw - 24.0 - 2.0 * b - 16.0,
            h: 44.0,
        },
        toggle,
        reset,
        presets,
        timer_card,
        remind: Rect {
            x: rx + 10.0,
            y: ry + 30.0,
            w: rw - 20.0,
            h: 26.0,
        },
        remind_hint: Rect {
            x: rx + 12.0,
            y: ry + 57.0,
            w: rw - 24.0,
            h: 16.0,
        },
        remind_card,
        rows,
        removes,
    }
}

pub const SEARCH_ROWS: usize = 6;
pub const SEARCH_H: f32 = 64.0 + SEARCH_ROWS as f32 * 38.0 + 8.0;

pub fn search_field(width: f32) -> Rect {
    Rect {
        x: PAD,
        y: 12.0,
        w: width - 2.0 * PAD,
        h: 40.0,
    }
}

pub fn search_rows(width: f32) -> Vec<Rect> {
    (0..SEARCH_ROWS)
        .map(|i| Rect {
            x: PAD,
            y: 62.0 + i as f32 * 38.0,
            w: width - 2.0 * PAD,
            h: 36.0,
        })
        .collect()
}

pub const TOOLS: [&str; 8] = [
    "Область",
    "Весь экран",
    "Текст",
    "QR-код",
    "Пипетка",
    "Без формата",
    "Команда",
    "Запись",
];
/// Index of the screen recording button in TOOLS.
pub const TOOL_RECORD: usize = 7;

/// Buttons of the Tools tile, left to right.
pub fn tool_buttons(tile: &Placed) -> Vec<Rect> {
    let n = TOOLS.len() as f32;
    let gap = 6.0;
    let w = (tile.w - 16.0 - gap * (n - 1.0)) / n;
    (0..TOOLS.len())
        .map(|i| Rect {
            x: tile.x + 8.0 + i as f32 * (w + gap),
            y: tile.y + 6.0,
            w,
            h: tile.h - 12.0,
        })
        .collect()
}

/// Parts of the Text tab.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRegions {
    pub card: Rect,
    pub field: Rect,
    pub stats: Rect,
    pub copy: Rect,
    pub status: Rect,
    pub ops: Vec<Rect>,
    pub translate: Rect,
}

pub fn text_regions(width: f32) -> TextRegions {
    let top = TAB_BAR_H;
    let inner = width - 2.0 * PAD;
    let lw = (inner - GAP) * 0.56;
    let h = PAGE_H - PAD;
    let card = Rect {
        x: PAD,
        y: top,
        w: lw,
        h,
    };
    let rx = PAD + lw + GAP;
    let rw = inner - lw - GAP;
    let bw = (rw - 6.0) / 2.0;
    let ops = (0..12)
        .map(|i| Rect {
            x: rx + (i % 2) as f32 * (bw + 6.0),
            y: top + (i / 2) as f32 * 34.0,
            w: bw,
            h: 30.0,
        })
        .collect();
    TextRegions {
        field: Rect {
            x: PAD + 12.0,
            y: top + 12.0,
            w: lw - 24.0,
            h: h - 56.0,
        },
        stats: Rect {
            x: PAD + 12.0,
            y: top + h - 36.0,
            w: lw - 120.0,
            h: 16.0,
        },
        status: Rect {
            x: PAD + 12.0,
            y: top + h - 20.0,
            w: lw - 120.0,
            h: 16.0,
        },
        copy: Rect {
            x: PAD + lw - 100.0,
            y: top + h - 38.0,
            w: 88.0,
            h: 28.0,
        },
        card,
        ops,
        translate: Rect {
            x: rx,
            y: top + 6.0 * 34.0 + 4.0,
            w: rw,
            h: 34.0,
        },
    }
}

pub const CTL_ROWS: usize = 5;

/// Parts of the Control tab.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlRegions {
    pub segments: Vec<Rect>,
    pub refresh: Rect,
    pub settings: Rect,
    pub rows: Vec<Rect>,
    pub actions: Vec<Rect>,
    pub message: Rect,
    pub power: Vec<Rect>,
}

pub fn control_regions(width: f32) -> ControlRegions {
    let top = TAB_BAR_H;
    let inner = width - 2.0 * PAD;
    let segments = (0..3)
        .map(|i| Rect {
            x: PAD + i as f32 * 96.0,
            y: top,
            w: 92.0,
            h: 28.0,
        })
        .collect();
    let settings = Rect {
        x: width - PAD - 96.0,
        y: top,
        w: 96.0,
        h: 28.0,
    };
    let refresh = Rect {
        x: settings.x - 34.0,
        y: top,
        w: 28.0,
        h: 28.0,
    };
    let rows: Vec<Rect> = (0..CTL_ROWS)
        .map(|i| Rect {
            x: PAD,
            y: top + 36.0 + i as f32 * 30.0,
            w: inner,
            h: 28.0,
        })
        .collect();
    let actions = rows
        .iter()
        .map(|r| Rect {
            x: r.x + r.w - 104.0,
            y: r.y + 2.0,
            w: 100.0,
            h: 24.0,
        })
        .collect();
    let py = top + 36.0 + CTL_ROWS as f32 * 30.0 + 22.0;
    let pw = (inner - 3.0 * GAP) / 4.0;
    let power = (0..4)
        .map(|i| Rect {
            x: PAD + i as f32 * (pw + GAP),
            y: py,
            w: pw,
            h: top + PAGE_H - PAD - py,
        })
        .collect();
    ControlRegions {
        segments,
        refresh,
        settings,
        rows,
        actions,
        message: Rect {
            x: PAD + 4.0,
            y: top + 36.0 + CTL_ROWS as f32 * 30.0 + 2.0,
            w: inner - 8.0,
            h: 16.0,
        },
        power,
    }
}

/// Password field and its button inside list row `r`.
pub fn password_parts(r: &Rect) -> (Rect, Rect) {
    let go = Rect {
        x: r.x + r.w - 76.0,
        y: r.y + 2.0,
        w: 72.0,
        h: 24.0,
    };
    (
        Rect {
            x: r.x + r.w * 0.45,
            y: r.y + 2.0,
            w: go.x - 6.0 - (r.x + r.w * 0.45),
            h: 24.0,
        },
        go,
    )
}

/// Rows of the Clipboard tab.
pub fn clip_page_rows() -> Vec<Rect> {
    (0..8)
        .map(|i| Rect {
            x: PAD,
            y: TAB_BAR_H + i as f32 * 34.0,
            w: 0.0,
            h: 32.0,
        })
        .collect()
}

/// Buttons of a notice in the collapsed pill, relative to the pill's
/// top-left; one-character ones ("✓", "✕") are round.
pub fn toast_actions(pill_w: f32, pill_h: f32, labels: &[String]) -> Vec<Rect> {
    let mut x = pill_w - 6.0;
    let mut out: Vec<Rect> = labels
        .iter()
        .rev()
        .map(|label| {
            // One-character buttons ("✓", "✕") are round.
            let w = if label.chars().count() == 1 {
                pill_h - 10.0
            } else {
                14.0 + label.chars().count() as f32 * 7.0
            };
            x -= w;
            let r = Rect {
                x,
                y: 5.0,
                w,
                h: pill_h - 10.0,
            };
            x -= 6.0;
            r
        })
        .collect();
    out.reverse();
    out
}

/// Parts of the active-window tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowRegions {
    pub icon: Rect,
    pub name: Rect,
    pub title: Rect,
    pub pin: Rect,
    pub opacity: Rect,
    pub minimize: Rect,
    pub menu: Rect,
}

pub fn window_regions(tile: &Placed) -> WindowRegions {
    let (x, y, w, h) = (tile.x, tile.y, tile.w, tile.h);
    let b = 34.0;
    let by = y + (h - b) / 2.0;
    let button = |i: f32| Rect {
        x: x + w - 12.0 - b - i * (b + 8.0),
        y: by,
        w: b,
        h: b,
    };
    let (menu, minimize, opacity, pin) = (button(0.0), button(1.0), button(2.0), button(3.0));
    let text_x = x + 64.0;
    let text_w = pin.x - 12.0 - text_x;
    WindowRegions {
        icon: Rect {
            x: x + 12.0,
            y: y + (h - 40.0) / 2.0,
            w: 40.0,
            h: 40.0,
        },
        name: Rect {
            x: text_x,
            y: y + 12.0,
            w: text_w,
            h: 20.0,
        },
        title: Rect {
            x: text_x,
            y: y + 32.0,
            w: text_w,
            h: 18.0,
        },
        pin,
        opacity,
        minimize,
        menu,
    }
}

pub fn window_hit(tile: &Placed, x: f32, y: f32) -> Option<Hit> {
    if !tile.contains(x, y) {
        return None;
    }
    let r = window_regions(tile);
    [
        (r.pin, Hit::WinPin),
        (r.opacity, Hit::WinOpacity),
        (r.minimize, Hit::WinMinimize),
        (r.menu, Hit::WinMenu),
    ]
    .into_iter()
    .find(|(r, _)| r.contains(x, y))
    .map(|(_, h)| h)
    .or(Some(Hit::WinActivate))
}

pub const CLIP_ROW_H: f32 = 30.0;
const CLIP_TOP: f32 = 34.0;

/// Rows of the clipboard tile that fit below its title.
pub fn clip_rows(tile: &Placed) -> Vec<Rect> {
    let fit = ((tile.h - CLIP_TOP - 6.0) / CLIP_ROW_H).floor().max(0.0) as usize;
    (0..fit)
        .map(|i| Rect {
            x: tile.x + 6.0,
            y: tile.y + CLIP_TOP + i as f32 * CLIP_ROW_H,
            w: tile.w - 12.0,
            h: CLIP_ROW_H,
        })
        .collect()
}

pub const DOCK_ICON: f32 = 40.0;
const DOCK_GAP: f32 = 12.0;

/// Slots for `n` dock icons (as many as fit) plus the "+" button after them.
pub fn dock_slots(tile: &Placed, n: usize) -> (Vec<Rect>, Rect) {
    let step = DOCK_ICON + DOCK_GAP;
    let fit = (((tile.w - 28.0 - DOCK_ICON) / step).floor().max(0.0) as usize).min(n);
    let y = tile.y + (tile.h - DOCK_ICON) / 2.0;
    let slot = |i: usize| Rect {
        x: tile.x + 14.0 + i as f32 * step,
        y,
        w: DOCK_ICON,
        h: DOCK_ICON,
    };
    ((0..fit).map(slot).collect(), slot(fit))
}

/// Volume (0..1) for a click at `x` on the volume track.
pub fn volume_at(track: &Rect, x: f32) -> f32 {
    ((x - track.x) / track.w).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use TileKind::*;

    #[test]
    fn default_home_packs_into_five_rows() {
        let (tiles, h) = pack(&[Media, Clipboard, Window, Tools, Apps], 560.0);
        assert_eq!(tiles.len(), 5);
        // Media and clipboard side by side, then three full-width rows.
        assert_eq!(tiles[0].y, tiles[1].y);
        assert!(tiles[1].x > tiles[0].x);
        assert!(tiles[2].y > tiles[0].y && tiles[3].y > tiles[2].y && tiles[4].y > tiles[3].y);
        assert_eq!(h, PAD * 2.0 + 5.0 * ROW_H + 4.0 * GAP);
    }

    #[test]
    fn removing_tiles_shrinks_panel() {
        let (_, full) = pack(&[Media, Clipboard, Tools, Weather, Apps], 560.0);
        let (tiles, less) = pack(&[Media, Apps], 560.0);
        assert!(less < full);
        // Apps moves up next to nothing but below media.
        assert_eq!(tiles[1].y, PAD + 2.0 * (ROW_H + GAP));
    }

    #[test]
    fn tiles_fill_gaps_left_by_shorter_ones() {
        // A 1-row tile next to Media (2 rows): the next one fits under it.
        let (tiles, _) = pack(&[Media, Weather, Weather], 560.0);
        assert_eq!(tiles[2].x, tiles[1].x);
        assert_eq!(tiles[2].y, PAD + ROW_H + GAP);
    }

    #[test]
    fn tiles_stay_inside_panel() {
        let (tiles, h) = pack(&[Media, Clipboard, Tools, Weather, Apps], 560.0);
        for t in tiles {
            assert!(t.x >= PAD && t.x + t.w <= 560.0 - PAD + 0.01);
            assert!(t.y + t.h <= h - PAD + 0.01);
        }
    }

    #[test]
    fn media_regions_fit_inside_the_tile() {
        let (tiles, _) = pack(&[Media, Clipboard], 560.0);
        let t = tiles[0];
        let r = media_regions(&t);
        for part in [
            r.cover,
            r.title,
            r.artist,
            r.prev,
            r.play,
            r.next,
            r.like,
            r.progress,
            r.progress_hit,
            r.times,
            r.speaker,
            r.volume,
        ] {
            assert!(
                part.x >= t.x && part.x + part.w <= t.x + t.w + 0.01,
                "{part:?} outside {t:?}"
            );
            assert!(
                part.y >= t.y && part.y + part.h <= t.y + t.h + 0.01,
                "{part:?} outside {t:?}"
            );
        }
        // Buttons do not overlap each other.
        assert!(r.prev.x + r.prev.w <= r.play.x && r.play.x + r.play.w <= r.next.x);
        assert!(r.next.x + r.next.w <= r.like.x);
    }

    #[test]
    fn clipboard_rows_fit() {
        let (tiles, _) = pack(&[Media, Clipboard], 560.0);
        let rows = clip_rows(&tiles[1]);
        assert_eq!(rows.len(), 3);
        let last = rows.last().unwrap();
        assert!(last.y + last.h <= tiles[1].y + tiles[1].h);
        assert_eq!(
            clip_rows(&Placed {
                kind: Clipboard,
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 30.0
            })
            .len(),
            0
        );
    }

    #[test]
    fn dock_slots_and_add_button() {
        let (tiles, _) = pack(&[Apps], 560.0);
        let t = tiles[0];
        let (slots, add) = dock_slots(&t, 3);
        assert_eq!(slots.len(), 3);
        assert!(add.x > slots[2].x);
        // Too many apps: only what fits, and "+" still inside the tile.
        let (slots, add) = dock_slots(&t, 50);
        assert!(slots.len() < 50);
        assert!(add.x + add.w <= t.x + t.w);
    }

    #[test]
    fn window_tile_parts_do_not_overlap() {
        let (tiles, _) = pack(&[Window], 560.0);
        let r = window_regions(&tiles[0]);
        assert!(r.name.x + r.name.w <= r.pin.x);
        assert!(r.pin.x + r.pin.w <= r.opacity.x);
        assert!(r.opacity.x + r.opacity.w <= r.minimize.x);
        assert!(r.minimize.x + r.minimize.w <= r.menu.x);
        assert!(r.menu.x + r.menu.w <= tiles[0].x + tiles[0].w);
    }

    #[test]
    fn window_card_buttons_and_body_have_distinct_actions() {
        let (tiles, _) = pack(&[Window], 560.0);
        let tile = &tiles[0];
        let r = window_regions(tile);
        for (rect, hit) in [
            (r.pin, Hit::WinPin),
            (r.opacity, Hit::WinOpacity),
            (r.minimize, Hit::WinMinimize),
            (r.menu, Hit::WinMenu),
        ] {
            assert_eq!(
                window_hit(tile, rect.x + rect.w / 2.0, rect.y + rect.h / 2.0),
                Some(hit)
            );
        }
        assert_eq!(
            window_hit(tile, r.name.x + 2.0, r.name.y + 2.0),
            Some(Hit::WinActivate)
        );
        assert_eq!(window_hit(tile, tile.x - 1.0, tile.y), None);
    }

    #[test]
    fn tabs_and_pages_fit() {
        let tabs = tab_buttons(Tab::Notes);
        assert_eq!(tabs.len(), 6);
        assert_eq!(tabs[2].1.w, 100.0);
        let r = notes_regions(560.0);
        assert!(r.note_card.x + r.note_card.w < r.timer_card.x);
        assert!(r.timer_card.y + r.timer_card.h <= r.remind_card.y);
        for row in r.rows.iter().chain(r.presets.iter()) {
            assert!(row.x + row.w <= 560.0 - PAD + 0.01);
        }
        let last = r.rows.last().unwrap();
        assert!(last.y + last.h <= r.remind_card.y + r.remind_card.h + 0.01);
        assert!(r.remind_card.y + r.remind_card.h <= TAB_BAR_H + PAGE_H);
        let rows = clip_page_rows();
        assert!(rows.last().unwrap().y + 32.0 <= TAB_BAR_H + PAGE_H);
    }

    #[test]
    fn tools_and_text_page_fit() {
        let (tiles, _) = pack(&[Tools], 560.0);
        let b = tool_buttons(&tiles[0]);
        assert_eq!(b.len(), TOOLS.len());
        assert!(b[TOOLS.len() - 1].x + b[TOOLS.len() - 1].w <= tiles[0].x + tiles[0].w + 0.01);
        // Room for labels like «Без формата».
        assert!(b.iter().all(|r| r.w >= 56.0), "{:?}", b[0].w);
        let r = text_regions(560.0);
        assert!(r.card.x + r.card.w < r.ops[0].x);
        assert!(r.translate.y + r.translate.h <= TAB_BAR_H + PAGE_H);
        assert!(r.ops[11].x + r.ops[11].w <= 560.0 - PAD + 0.01);
    }

    #[test]
    fn control_page_fits() {
        let r = control_regions(560.0);
        assert!(r.refresh.x > r.segments[2].x + r.segments[2].w);
        let last = r.rows.last().unwrap();
        assert!(last.y + last.h < r.message.y + 0.01);
        assert!(r.message.y + r.message.h <= r.power[0].y);
        assert!(r.power[0].h >= 40.0);
        assert!(r.power[3].x + r.power[3].w <= 560.0 - PAD + 0.01);
        assert!(r.power[0].y + r.power[0].h <= TAB_BAR_H + PAGE_H);
        let (field, go) = password_parts(&r.rows[0]);
        assert!(field.x + field.w < go.x);
    }

    #[test]
    fn toast_buttons_from_the_right() {
        let b = toast_actions(320.0, 36.0, &["Ещё раз".into(), "✓".into()]);
        assert_eq!(b.len(), 2);
        assert!(b[0].x + b[0].w < b[1].x);
        assert!((b[1].x + b[1].w - 314.0).abs() < 0.01);
    }

    #[test]
    fn volume_track_maps_to_0_1() {
        let track = Rect {
            x: 10.0,
            y: 0.0,
            w: 100.0,
            h: 4.0,
        };
        assert_eq!(volume_at(&track, 10.0), 0.0);
        assert_eq!(volume_at(&track, 60.0), 0.5);
        assert_eq!(volume_at(&track, 500.0), 1.0);
        assert_eq!(volume_at(&track, -5.0), 0.0);
    }

    #[test]
    fn empty_and_unknown_ids() {
        assert_eq!(pack(&[], 560.0).1, EMPTY_HEIGHT);
        let ids = ["media", "nope", "Media", "apps"].map(String::from);
        assert_eq!(tiles_from_ids(&ids), vec![Media, Apps]);
    }
}
