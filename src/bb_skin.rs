//! Calm native bb surfaces, using Tidemark's existing settings palette.
use crate::{
    bb_layout::{layout, Rect},
    settings::skin::{self, Canvas},
};
use windows::Win32::Graphics::Gdi::*;
pub const WHITE: u32 = 0xFFFFFF;
pub unsafe fn wrapped(c: &Canvas, s: &str, r: Rect, font: usize, color: u32) {
    let old = SelectObject(c.dc, c.fonts[font].into());
    SetTextColor(c.dc, skin::color(color));
    SetBkMode(c.dc, TRANSPARENT);
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    let mut rc = c.rect(r.x, r.y, r.w, r.h);
    DrawTextW(
        c.dc,
        &mut buf,
        &mut rc,
        DT_LEFT | DT_WORDBREAK | DT_NOPREFIX | DT_END_ELLIPSIS,
    );
    SelectObject(c.dc, old);
}
pub unsafe fn surface(c: &Canvas, r: Rect, focused: bool) {
    c.round(
        r.x,
        r.y,
        r.w,
        r.h,
        12,
        if focused { skin::ACCENT } else { skin::LINE },
    );
    c.round(r.x + 1, r.y + 1, r.w - 2, r.h - 2, 11, WHITE);
}
pub unsafe fn paint(
    c: &Canvas,
    width: i32,
    height: i32,
    connection: bool,
    meta: &str,
    choices: usize,
    focus_reply: bool,
) {
    c.fill(0, 0, width, height, skin::BG);
    c.mark(28, 24);
    c.text(
        if connection {
            "Подключение к bb"
        } else {
            "Агенты bb"
        },
        80,
        21,
        width - 360,
        34,
        2,
        skin::INK,
    );
    c.text(
        if connection {
            "Один раз настройте связь с вашим рабочим пространством"
        } else {
            "Вопросы, ответы и продолжение работы"
        },
        80,
        56,
        width - 112,
        24,
        1,
        skin::MUTED,
    );
    c.fill(28, 96, width - 56, 1, skin::LINE);
    if connection {
        c.text("Адрес сервера", 40, 154, width - 100, 26, 0, skin::INK);
        surface(
            c,
            Rect {
                x: 28,
                y: 192,
                w: width - 56,
                h: 52,
            },
            false,
        );
        c.text("Токен подключения", 40, 276, width - 100, 26, 0, skin::INK);
        surface(
            c,
            Rect {
                x: 28,
                y: 314,
                w: width - 56,
                h: 52,
            },
            false,
        );
        c.text(
            "Получите токен в bb командой:  bb plugin token tidemark",
            40,
            384,
            width - 90,
            26,
            1,
            skin::MUTED,
        );
        c.text(
            "Токен хранится зашифрованным на этом компьютере.",
            40,
            414,
            width - 90,
            26,
            1,
            skin::MUTED,
        );
        c.fill(28, height - 80, width - 56, 1, skin::LINE);
    } else {
        c.text("Разговор", 28, 106, width / 2 - 30, 20, 1, skin::MUTED);
        c.text(
            "Что сделать",
            width / 2 + 12,
            106,
            width / 2 - 40,
            20,
            1,
            skin::MUTED,
        );
        let l = layout(width, height, choices);
        surface(c, l.question, false);
        c.text(meta, 48, l.question.y + 12, width - 96, 22, 1, skin::MUTED);
        c.text(
            if choices > 0 {
                "Свой ответ, если хотите уточнить"
            } else {
                "Ваше сообщение"
            },
            28,
            l.reply.y - 28,
            width - 56,
            24,
            0,
            skin::INK,
        );
        surface(c, l.reply, focus_reply);
        c.fill(28, l.footer, width - 56, 1, skin::LINE);
    }
}
pub unsafe fn button(
    c: &Canvas,
    width: i32,
    height: i32,
    label: &str,
    primary: bool,
    pressed: bool,
    focused: bool,
    enabled: bool,
    hovered: bool,
) {
    c.fill(0, 0, width, height, skin::BG);
    let fill = if !enabled {
        skin::SIDE
    } else if primary {
        if pressed || hovered {
            0x853A28
        } else {
            skin::ACCENT
        }
    } else if pressed || hovered {
        skin::SELECT
    } else {
        skin::SIDE
    };
    c.round(0, 0, width, height, 9, fill);
    let old = SelectObject(c.dc, c.fonts[0].into());
    SetTextColor(
        c.dc,
        skin::color(if !enabled {
            skin::MUTED
        } else if primary {
            WHITE
        } else {
            skin::INK
        }),
    );
    SetBkMode(c.dc, TRANSPARENT);
    let mut buf: Vec<u16> = label.encode_utf16().collect();
    let mut r = c.rect(10, 0, width - 20, height);
    DrawTextW(
        c.dc,
        &mut buf,
        &mut r,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    SelectObject(c.dc, old);
    if focused {
        let r = c.rect(4, 4, width - 8, height - 8);
        let _ = DrawFocusRect(c.dc, &r);
    }
}
pub unsafe fn choice(
    c: &Canvas,
    width: i32,
    height: i32,
    label: &str,
    description: &str,
    selected: bool,
    multi: bool,
    focused: bool,
) {
    c.fill(0, 0, width, height, skin::BG);
    c.round(
        0,
        2,
        width - 2,
        height - 6,
        10,
        if selected { skin::ACCENT } else { skin::LINE },
    );
    c.round(
        1,
        3,
        width - 4,
        height - 8,
        9,
        if selected { 0xF4EAE4 } else { WHITE },
    );
    let y = height / 2 - 9;
    if multi {
        c.round(
            16,
            y,
            18,
            18,
            4,
            if selected { skin::ACCENT } else { skin::LINE },
        );
        if selected {
            c.text("✓", 18, y - 2, 16, 21, 0, WHITE);
        } else {
            c.round(18, y + 2, 14, 14, 2, WHITE);
        }
    } else {
        c.round(
            16,
            y,
            18,
            18,
            9,
            if selected { skin::ACCENT } else { skin::LINE },
        );
        c.round(18, y + 2, 14, 14, 7, WHITE);
        if selected {
            c.round(21, y + 5, 8, 8, 4, skin::ACCENT);
        }
    }
    c.text(
        label,
        48,
        if description.is_empty() { 0 } else { 8 },
        width - 64,
        if description.is_empty() {
            height - 4
        } else {
            23
        },
        0,
        skin::INK,
    );
    if !description.is_empty() {
        wrapped(
            c,
            description,
            Rect {
                x: 48,
                y: 32,
                w: width - 70,
                h: height - 38,
            },
            1,
            skin::MUTED,
        );
    }
    if focused {
        let r = c.rect(5, 6, width - 12, height - 14);
        let _ = DrawFocusRect(c.dc, &r);
    }
}

#[cfg(test)]
mod preview {
    use super::*;
    #[test]
    #[ignore = "writes a design preview using only an offscreen GDI bitmap"]
    fn render_design_preview() {
        unsafe {
            let (width, height) = (760, 740);
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap =
                CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            let old = SelectObject(dc, bitmap.into());
            let fonts = [
                skin::font(15, 400, 96),
                skin::font(12, 400, 96),
                skin::font(25, 600, 96),
                skin::font(18, 400, 96),
            ];
            let c = Canvas { dc, dpi: 96, fonts };
            paint(&c, width, height, false, "Вопрос 1 из 2", 3, false);
            c.text("Редизайн личного кабинета", 40, 128, 310, 26, 0, skin::INK);
            c.text("Вопрос от агента", 404, 128, 300, 26, 0, skin::INK);
            let l = layout(width, height, 3);
            wrapped(
                &c,
                "Какой вариант навигации оставляем в новой версии?",
                Rect {
                    x: 48,
                    y: l.question.y + 44,
                    w: 650,
                    h: 70,
                },
                3,
                skin::INK,
            );
            let choices = [
                (
                    "Боковая панель",
                    "Разделы всегда перед глазами. Удобно на большом экране.",
                ),
                (
                    "Компактное меню",
                    "Больше места для содержимого, навигация по кнопке.",
                ),
                (
                    "Оставить агенту",
                    "Подобрать вариант с учётом структуры проекта.",
                ),
            ];
            for (i, (label, desc)) in choices.iter().enumerate() {
                let saved = SaveDC(dc);
                let _ = SetViewportOrgEx(dc, 28, l.options.y + i as i32 * 64, None);
                choice(&c, 704, 64, label, desc, i == 0, false, false);
                let _ = RestoreDC(dc, saved);
            }
            wrapped(
                &c,
                "Оставь боковую панель, а на узких экранах сворачивай её в меню.",
                Rect {
                    x: l.reply.x + 16,
                    y: l.reply.y + 14,
                    w: l.reply.w - 32,
                    h: l.reply.h - 28,
                },
                0,
                skin::INK,
            );
            let saved = SaveDC(dc);
            let _ = SetViewportOrgEx(dc, width - 248, l.footer + 16, None);
            button(
                &c,
                220,
                42,
                "Следующий вопрос",
                true,
                false,
                false,
                true,
                false,
            );
            let _ = RestoreDC(dc, saved);
            let saved = SaveDC(dc);
            let _ = SetViewportOrgEx(dc, width - 188, 26, None);
            button(&c, 160, 36, "Подключение", false, false, false, true, false);
            let _ = RestoreDC(dc, saved);
            let saved = SaveDC(dc);
            let _ = SetViewportOrgEx(dc, width - 298, 26, None);
            button(&c, 98, 36, "Обновить", false, false, false, true, false);
            let _ = RestoreDC(dc, saved);
            c.text(
                "Подключено к bb",
                28,
                l.footer + 22,
                240,
                28,
                1,
                skin::MUTED,
            );
            let pixels =
                std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize);
            let mut out = Vec::with_capacity(pixels.len() + 54);
            out.extend_from_slice(b"BM");
            out.extend_from_slice(&((pixels.len() + 54) as u32).to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&54u32.to_le_bytes());
            out.extend_from_slice(&40u32.to_le_bytes());
            out.extend_from_slice(&width.to_le_bytes());
            out.extend_from_slice(&(-height).to_le_bytes());
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&32u16.to_le_bytes());
            out.extend_from_slice(&[0; 24]);
            out.extend_from_slice(pixels);
            let path = std::env::var("BB_DESIGN_PREVIEW")
                .unwrap_or_else(|_| "target/bb-design-preview.bmp".into());
            std::fs::write(path, out).unwrap();
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            for font in fonts {
                let _ = DeleteObject(font.into());
            }
        }
    }
}
