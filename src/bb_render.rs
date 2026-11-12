//! bb is painted by the island's Direct2D renderer, not a separate form.
use super::*;
use crate::bb_island::{self, View};
impl Renderer {
    pub(super) fn paint_bb(&self, scene: &Scene, v: &View) {
        let fonts = self.fonts.as_ref().unwrap();
        let (k, th) = (scene.scale, scene.theme);
        let r = bb_island::regions(scene.panel_size.0, scene.panel_size.1, v);
        let px = |r: Rect| D2D_RECT_F {
            left: r.x * k,
            top: r.y * k,
            right: (r.x + r.w) * k,
            bottom: (r.y + r.h) * k,
        };
        let text = |s: &str, r: Rect, c: Rgba, fmt: &IDWriteTextFormat| unsafe {
            self.brush.SetColor(&color(c, 1.0));
            let wide: Vec<u16> = s.encode_utf16().collect();
            self.dc.DrawText(
                &wide,
                fmt,
                &px(r),
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        };
        let button = |label: &str, r: Rect, hit: Hit, primary: bool| {
            self.fill_rect(
                if primary {
                    th.accent
                } else if scene.hot == Some(hit) {
                    th.tile_hover
                } else {
                    th.tile
                },
                px(r),
                12.0 * k,
            );
            text(
                label,
                Rect {
                    x: r.x + 10.0,
                    y: r.y + 7.0,
                    w: r.w - 20.0,
                    h: r.h - 10.0,
                },
                if primary {
                    [0.035, 0.05, 0.075, 1.0]
                } else {
                    th.text
                },
                &fonts.lyric,
            );
        };
        self.fill_rect(
            if scene.hot == Some(Hit::BbThread) {
                th.tile_hover
            } else {
                th.tile
            },
            px(r.thread),
            12.0 * k,
        );
        text(
            &v.thread,
            Rect {
                x: r.thread.x + 12.0,
                y: r.thread.y + 8.0,
                w: r.thread.w - 38.0,
                h: 22.0,
            },
            th.text,
            &fonts.track_title,
        );
        text(
            "⌄",
            Rect {
                x: r.thread.x + r.thread.w - 25.0,
                y: r.thread.y + 5.0,
                w: 20.0,
                h: 24.0,
            },
            th.subtext,
            &fonts.lyric,
        );
        button("↗", r.window, Hit::BbWindow, false);
        button("···", r.settings, Hit::BbSettings, false);
        if v.picker {
            self.paint_field(
                &v.field,
                r.field,
                v.focus,
                "Найти разговор",
                &fonts.note,
                true,
                k,
                th,
            );
            if v.threads.is_empty() {
                text(
                    "Ничего не найдено",
                    Rect {
                        x: 28.0,
                        y: 178.0,
                        w: 400.0,
                        h: 28.0,
                    },
                    th.subtext,
                    &fonts.note,
                );
            }
            for (i, ((title, status), rc)) in v.threads.iter().zip(&r.rows).enumerate() {
                self.fill_rect(
                    if scene.hot == Some(Hit::BbThreadRow(i)) {
                        th.tile_hover
                    } else {
                        th.tile
                    },
                    px(*rc),
                    10.0 * k,
                );
                text(
                    title,
                    Rect {
                        x: rc.x + 12.0,
                        y: rc.y + 5.0,
                        w: rc.w - 28.0,
                        h: 23.0,
                    },
                    th.text,
                    &fonts.track_title,
                );
                text(
                    status,
                    Rect {
                        x: rc.x + 12.0,
                        y: rc.y + 27.0,
                        w: rc.w - 28.0,
                        h: 16.0,
                    },
                    th.subtext,
                    &fonts.small,
                );
            }
            button("Готово", r.send, Hit::BbSend, true);
            return;
        }
        text(
            &v.meta,
            Rect {
                x: 24.0,
                y: 108.0,
                w: 260.0,
                h: 20.0,
            },
            th.subtext,
            &fonts.small,
        );
        if v.pending > 1 {
            button("›", r.next, Hit::BbNext, false);
        }
        button(
            if v.meta == "Новое сообщение" && v.pending > 0 {
                "К вопросам"
            } else {
                "Написать"
            },
            r.mode,
            Hit::BbMode,
            false,
        );
        self.paint_bb_wrapped(&v.prompt, r.prompt, &fonts.note, th.text, v.scroll, k);
        for (i, (o, rc)) in v.choices.iter().zip(&r.choices).enumerate() {
            let fill = if o.selected {
                [0.08, 0.15, 0.25, 1.0]
            } else if scene.hot == Some(Hit::BbOption(i)) {
                th.tile_hover
            } else {
                th.tile
            };
            self.fill_rect(fill, px(*rc), 12.0 * k);
            if o.selected {
                unsafe {
                    self.brush.SetColor(&color(th.accent, 0.7));
                    self.dc.DrawRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: px(*rc),
                            radiusX: 12.0 * k,
                            radiusY: 12.0 * k,
                        },
                        &self.brush,
                        1.0 * k,
                        None,
                    );
                }
            }
            let (cx, cy) = ((rc.x + 18.0) * k, (rc.y + 21.0) * k);
            if v.multi {
                self.fill_rect(
                    if o.selected { th.accent } else { th.tile_hover },
                    D2D_RECT_F {
                        left: cx - 6.0 * k,
                        top: cy - 6.0 * k,
                        right: cx + 6.0 * k,
                        bottom: cy + 6.0 * k,
                    },
                    3.0 * k,
                );
                if o.selected {
                    text(
                        "✓",
                        Rect {
                            x: rc.x + 12.0,
                            y: rc.y + 11.0,
                            w: 16.0,
                            h: 20.0,
                        },
                        [0.03, 0.05, 0.08, 1.0],
                        &fonts.small,
                    );
                }
            } else {
                self.fill_circle(
                    if o.selected { th.accent } else { th.subtext },
                    cx,
                    cy,
                    6.0 * k,
                );
                self.fill_circle(fill, cx, cy, 4.5 * k);
                if o.selected {
                    self.fill_circle(th.accent, cx, cy, 2.8 * k);
                }
            }
            text(
                &o.label,
                Rect {
                    x: rc.x + 36.0,
                    y: rc.y + if o.description.is_empty() { 11.0 } else { 4.0 },
                    w: rc.w - 52.0,
                    h: 22.0,
                },
                th.text,
                &fonts.track_title,
            );
            if !o.description.is_empty() {
                text(
                    &o.description,
                    Rect {
                        x: rc.x + 36.0,
                        y: rc.y + 24.0,
                        w: rc.w - 52.0,
                        h: 16.0,
                    },
                    th.subtext,
                    &fonts.small,
                );
            }
        }
        if v.allow_text {
            self.paint_field(
                &v.field,
                r.field,
                v.focus,
                if v.choices.is_empty() {
                    "Напишите сообщение…"
                } else {
                    "Свой ответ или уточнение…"
                },
                &fonts.note,
                true,
                k,
                th,
            );
        } else if v.choices.is_empty() && !v.approval {
            text(
                "Продолжите в окне bb",
                Rect {
                    x: 24.0,
                    y: 240.0,
                    w: 420.0,
                    h: 24.0,
                },
                th.subtext,
                &fonts.note,
            );
        }
        if v.can_back || v.approval {
            button(
                if v.approval {
                    "Отказ"
                } else {
                    "Назад"
                },
                r.back,
                Hit::BbBack,
                false,
            );
        }
        text(
            if v.status.is_empty() {
                "Ctrl + Enter — отправить"
            } else {
                &v.status
            },
            r.status,
            if v.status.is_empty() {
                th.subtext
            } else {
                th.accent
            },
            &fonts.small,
        );
        button(&v.send, r.send, Hit::BbSend, !v.busy);
    }
    fn paint_bb_wrapped(
        &self,
        s: &str,
        r: Rect,
        fmt: &IDWriteTextFormat,
        ink: Rgba,
        scroll: f32,
        k: f32,
    ) {
        unsafe {
            let clip = D2D_RECT_F {
                left: r.x * k,
                top: r.y * k,
                right: (r.x + r.w) * k,
                bottom: (r.y + r.h) * k,
            };
            let wide: Vec<u16> = s.encode_utf16().collect();
            if let Ok(layout) = self.dwrite.CreateTextLayout(&wide, fmt, r.w * k, 100000.0) {
                let mut metrics = DWRITE_TEXT_METRICS::default();
                let _ = layout.GetMetrics(&mut metrics);
                let offset = (scroll * k).min((metrics.height - r.h * k).max(0.0));
                self.dc
                    .PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
                self.brush.SetColor(&color(ink, 1.0));
                self.dc.DrawTextLayout(
                    pt(r.x * k, r.y * k - offset),
                    &layout,
                    &self.brush,
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
                self.dc.PopAxisAlignedClip();
            }
        }
    }
}
