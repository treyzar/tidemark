//! GDI settings skin: no web runtime, no animation loop, DPI-aware native controls.
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const BG: u32 = 0xFAF9F6;
pub const SIDE: u32 = 0xF0EFEB;
pub const INK: u32 = 0x292D2C;
pub const MUTED: u32 = 0x646B67;
pub const LINE: u32 = 0xE2E3DE;
pub const ACCENT: u32 = 0xA54831;
pub const SELECT: u32 = 0xE2E5DF;
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |shift: u32| {
        let x = ((a >> shift) & 255) as f32;
        let y = ((b >> shift) & 255) as f32;
        (x + (y - x) * t).round() as u32
    };
    channel(16) << 16 | channel(8) << 8 | channel(0)
}
pub fn color(rgb: u32) -> COLORREF {
    COLORREF(((rgb >> 16) & 255) | (rgb & 0xFF00) | ((rgb & 255) << 16))
}
pub fn scale(n: i32, dpi: u32) -> i32 {
    (n as i64 * dpi as i64 / 96) as i32
}

pub unsafe fn font(size: i32, weight: i32, dpi: u32) -> HFONT {
    let mut f = LOGFONTW {
        lfHeight: -scale(size, dpi),
        lfWeight: weight,
        lfQuality: CLEARTYPE_QUALITY,
        ..Default::default()
    };
    for (a, b) in f.lfFaceName.iter_mut().zip("Segoe UI".encode_utf16()) {
        *a = b;
    }
    CreateFontIndirectW(&f)
}

pub struct Canvas {
    pub dc: HDC,
    pub dpi: u32,
    pub fonts: [HFONT; 4],
}
impl Canvas {
    pub fn rect(&self, x: i32, y: i32, w: i32, h: i32) -> RECT {
        RECT {
            left: scale(x, self.dpi),
            top: scale(y, self.dpi),
            right: scale(x + w, self.dpi),
            bottom: scale(y + h, self.dpi),
        }
    }
    pub unsafe fn fill(&self, x: i32, y: i32, w: i32, h: i32, c: u32) {
        let b = CreateSolidBrush(color(c));
        FillRect(self.dc, &self.rect(x, y, w, h), b);
        let _ = DeleteObject(b.into());
    }
    pub unsafe fn round(&self, x: i32, y: i32, w: i32, h: i32, r: i32, c: u32) {
        let b = CreateSolidBrush(color(c));
        let old = SelectObject(self.dc, b.into());
        let pen = SelectObject(self.dc, GetStockObject(NULL_PEN));
        let a = self.rect(x, y, w, h);
        let _ = RoundRect(
            self.dc,
            a.left,
            a.top,
            a.right,
            a.bottom,
            scale(r * 2, self.dpi),
            scale(r * 2, self.dpi),
        );
        SelectObject(self.dc, old);
        SelectObject(self.dc, pen);
        let _ = DeleteObject(b.into());
    }
    pub unsafe fn translucent_round(
        &self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        r: i32,
        c: u32,
        opacity: i32,
    ) {
        let (sw, sh) = (scale(w, self.dpi), scale(h, self.dpi));
        let mem = CreateCompatibleDC(Some(self.dc));
        let bmp = CreateCompatibleBitmap(self.dc, sw, sh);
        let old = SelectObject(mem, bmp.into());
        let _ = BitBlt(
            mem,
            0,
            0,
            sw,
            sh,
            Some(self.dc),
            scale(x, self.dpi),
            scale(y, self.dpi),
            SRCCOPY,
        );
        let layer = Canvas {
            dc: mem,
            dpi: self.dpi,
            fonts: self.fonts,
        };
        layer.round(0, 0, w, h, r, c);
        let _ = GdiAlphaBlend(
            self.dc,
            scale(x, self.dpi),
            scale(y, self.dpi),
            sw,
            sh,
            mem,
            0,
            0,
            sw,
            sh,
            BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: (opacity.clamp(30, 100) * 255 / 100) as u8,
                AlphaFormat: 0,
            },
        );
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
    }
    pub unsafe fn text(&self, s: &str, x: i32, y: i32, w: i32, h: i32, font: usize, c: u32) {
        let old = SelectObject(self.dc, self.fonts[font].into());
        SetTextColor(self.dc, color(c));
        SetBkMode(self.dc, TRANSPARENT);
        let mut buf: Vec<u16> = s.encode_utf16().collect();
        let mut rc = self.rect(x, y, w, h);
        DrawTextW(
            self.dc,
            &mut buf,
            &mut rc,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        SelectObject(self.dc, old);
    }
    pub unsafe fn image(&self, bmp: HBITMAP, x: i32, y: i32, w: i32, h: i32) {
        if bmp.0.is_null() {
            return;
        }
        let mem = CreateCompatibleDC(Some(self.dc));
        let old = SelectObject(mem, bmp.into());
        let mut info = BITMAP::default();
        GetObjectW(
            bmp.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut info as *mut _ as *mut _),
        );
        SetStretchBltMode(self.dc, HALFTONE);
        let _ = StretchBlt(
            self.dc,
            scale(x, self.dpi),
            scale(y, self.dpi),
            scale(w, self.dpi),
            scale(h, self.dpi),
            Some(mem),
            0,
            0,
            info.bmWidth,
            info.bmHeight,
            SRCCOPY,
        );
        SelectObject(mem, old);
        let _ = DeleteDC(mem);
    }
    pub unsafe fn mark(&self, x: i32, y: i32) {
        self.round(x, y, 36, 36, 10, INK);
        self.round(x + 6, y + 14, 18, 8, 4, 0xF7F5F0);
        self.round(x + 26, y + 15, 6, 6, 3, 0xCF775C);
    }
}

pub unsafe fn caption(hwnd: HWND) -> String {
    let mut b = vec![0u16; GetWindowTextLengthW(hwnd).max(0) as usize + 1];
    let n = GetWindowTextW(hwnd, &mut b) as usize;
    String::from_utf16_lossy(&b[..n])
}

pub unsafe fn load_bitmap(id: usize) -> HBITMAP {
    use windows::core::PCWSTR;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    LoadImageW(
        GetModuleHandleW(None).ok().map(Into::into),
        PCWSTR(id as *const u16),
        IMAGE_BITMAP,
        0,
        0,
        LR_CREATEDIBSECTION,
    )
    .map(|h| HBITMAP(h.0))
    .unwrap_or_default()
}

pub const TITLES: [&str; 4] = [
    "Внешний вид",
    "Плитки острова",
    "Поведение",
    "Система и данные",
];
pub const DESCRIPTIONS: [&str; 4] = [
    "Настройте остров под свой рабочий стол",
    "Оставьте под рукой то, чем пользуетесь",
    "Решите, когда остров появляется и что показывает",
    "Автозапуск, погода и поиск в интернете",
];

pub unsafe fn paint(
    c: &Canvas,
    page: usize,
    bmp: HBITMAP,
    light: bool,
    accent: u32,
    bg: u32,
    opacity: i32,
    status: &str,
) {
    c.fill(0, 0, 1000, 688, BG);
    c.fill(0, 0, 232, 688, SIDE);
    c.fill(231, 0, 1, 688, LINE);
    c.mark(24, 32);
    c.text("Tidemark", 72, 30, 144, 28, 2, INK);
    c.text("Остров для Windows", 24, 84, 184, 24, 1, MUTED);
    c.text("Ваш рабочий стол", 24, 124, 184, 24, 1, MUTED);
    c.text(
        concat!("Tidemark  /  ", env!("CARGO_PKG_VERSION")),
        24,
        600,
        184,
        24,
        1,
        MUTED,
    );
    c.text("Всё рядом. Ничего лишнего", 24, 626, 192, 24, 1, MUTED);
    c.text(TITLES[page], 264, 28, 688, 44, 3, INK);
    c.text(DESCRIPTIONS[page], 264, 76, 688, 28, 0, MUTED);
    match page {
        0 => {
            c.image(bmp, 264, 120, 704, 184);
            c.translucent_round(504, 136, 224, 36, 18, bg, opacity);
            c.round(520, 149, 10, 10, 5, accent);
            let ink = if light { INK } else { 0xF7F5F0 };
            c.text("Tidemark", 540, 140, 104, 28, 1, ink);
            c.text("12:48", 664, 140, 56, 28, 1, ink);
            c.text("Превью острова", 264, 312, 180, 24, 1, MUTED);
            c.text(
                &format!("Непрозрачность {opacity}%"),
                764,
                312,
                204,
                24,
                1,
                MUTED,
            );
            for (x, y, w) in [(696, 408, 184), (696, 460, 184), (860, 564, 112)] {
                c.round(x, y, w, 40, 8, 0xFFFFFF);
            }
            for (y, title, desc) in [
                (356, "Тема", "Следовать Windows или выбрать свою"),
                (408, "Акцент", "Для элементов управления островом"),
                (
                    460,
                    "Фон острова",
                    "Оставьте пустым, чтобы использовать тему",
                ),
                (512, "Непрозрачность", "От лёгкой дымки до плотного фона"),
                (564, "Раскрытие при наведении", "Задержка в миллисекундах"),
            ] {
                c.text(title, 264, y, 404, 24, 0, INK);
                c.text(desc, 264, y + 24, 420, 20, 1, MUTED);
                if y < 564 {
                    c.fill(264, y + 49, 704, 1, LINE);
                }
            }
        }
        1 => {
            let titles = [
                "Музыка",
                "Буфер обмена",
                "Окно",
                "Инструменты",
                "Погода",
                "Приложения",
            ];
            let desc = [
                "Трек, обложка и управление воспроизведением",
                "Недавно скопированные тексты, изображения и файлы",
                "Быстрые действия с активным окном",
                "Скриншоты, запись экрана, OCR и QR",
                "Погода для выбранного города",
                "Закреплённые приложения в доке",
            ];
            for i in 0..6 {
                let y = 124 + i as i32 * 76;
                c.text(titles[i], 264, y, 580, 28, 0, INK);
                c.text(desc[i], 264, y + 28, 580, 24, 1, MUTED);
                c.fill(264, y + 64, 704, 1, LINE);
            }
        }
        2 => {
            let titles = [
                "Полноэкранные приложения",
                "Развёрнутые окна",
                "Активное приложение",
                "Системные индикаторы",
                "Камера и микрофон",
                "Поиск по Alt+Space",
                "Музыкальный край",
                "История буфера обмена",
                "Аппаратный рендер",
            ];
            let desc = [
                "Прятать остров во время игр и просмотра видео",
                "Сжимать остров, чтобы не перекрывать вкладки",
                "Показывать приложение при переключении",
                "Громкость, яркость, раскладка и Caps Lock",
                "Показывать индикатор использования",
                "Приложения, файлы, калькулятор и интернет",
                "Пульсация края в такт воспроизведению",
                "Сохранять недавно скопированное локально",
                "Использовать видеокарту; требует больше памяти",
            ];
            for i in 0..9 {
                let y = 116 + i as i32 * 54;
                c.text(titles[i], 264, y, 588, 25, 0, INK);
                c.text(desc[i], 264, y + 25, 588, 21, 1, MUTED);
                if i < 8 {
                    c.fill(264, y + 51, 704, 1, LINE);
                }
            }
        }
        _ => {
            c.text("Запускать вместе с Windows", 264, 128, 580, 28, 0, INK);
            c.text(
                "Остров будет готов к работе после входа в систему",
                264,
                160,
                596,
                24,
                1,
                MUTED,
            );
            c.fill(264, 204, 704, 1, LINE);
            c.text("Город для погоды", 264, 228, 640, 28, 0, INK);
            c.text("Пустое поле отключает погоду", 264, 260, 640, 24, 1, MUTED);
            c.round(260, 292, 712, 40, 8, 0xFFFFFF);
            c.text("Поиск в интернете", 264, 348, 640, 28, 0, INK);
            c.text(
                "Используйте {q} в адресе вместо поискового запроса",
                264,
                380,
                640,
                24,
                1,
                MUTED,
            );
            c.round(260, 412, 712, 40, 8, 0xFFFFFF);
            c.fill(264, 468, 704, 1, LINE);
            c.text("Расширенные настройки", 264, 492, 640, 28, 0, INK);
            c.text(
                "Размеры острова и приложения в доке — в config.toml",
                264,
                524,
                640,
                24,
                1,
                MUTED,
            );
        }
    }
    c.fill(232, 624, 768, 64, BG);
    c.fill(232, 624, 768, 1, LINE);
    c.text(status, 264, 640, 380, 32, 1, MUTED);
}
