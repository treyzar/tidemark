//! Opt-in previews through the production Direct2D island renderer.
use super::*;
use crate::bb_island::{Choice, View};
use windows::Win32::{System::Com::*, UI::WindowsAndMessaging::*};

struct HiddenWindow(HWND);
impl Drop for HiddenWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

fn sample() -> View {
    View {
        thread: "План на вечер".into(),
        meta: "Вопрос 1 из 1".into(),
        prompt: "Что приготовить к вечерней работе?".into(),
        choices: vec![
            Choice {
                label: "Кофе".into(),
                description: "Эспрессо или капучино".into(),
                selected: false,
            },
            Choice {
                label: "Чай".into(),
                description: "Зелёный чай с лимоном".into(),
                selected: true,
            },
            Choice {
                label: "Вода".into(),
                description: "Без кофеина".into(),
                selected: false,
            },
        ],
        field: TextField::with_text("Чай, пожалуйста. И продолжай работу над задачей.", true),
        focus: true,
        allow_text: true,
        multi: false,
        busy: false,
        status: "Подключено к bb".into(),
        send: "Отправить".into(),
        picker: false,
        threads: vec![],
        pending: 1,
        can_back: false,
        scroll: 0.0,
        offset: 0,
        approval: false,
    }
}

unsafe fn save_bitmap(renderer: &Renderer, bitmap: &ID2D1Bitmap1, path: &str) {
    let size = bitmap.GetPixelSize();
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        ..Default::default()
    };
    let readback = renderer
        .dc
        .CreateBitmap(size, None, 0, &props)
        .expect("CPU readback bitmap");
    readback
        .CopyFromBitmap(None, bitmap, None)
        .expect("copy rendered pixels");
    let mapped = readback
        .Map(D2D1_MAP_OPTIONS_READ)
        .expect("map rendered pixels");
    let row_bytes = size.width as usize * 4;
    let mut pixels = Vec::with_capacity(row_bytes * size.height as usize);
    for y in 0..size.height as usize {
        pixels.extend_from_slice(std::slice::from_raw_parts(
            mapped.bits.add(y * mapped.pitch as usize),
            row_bytes,
        ));
    }
    readback.Unmap().expect("unmap preview");
    assert!(
        pixels.chunks_exact(4).any(|p| p[0] > 100),
        "preview contains rendered foreground"
    );
    let mut bmp = Vec::with_capacity(54 + pixels.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((54 + pixels.len()) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(size.width as i32).to_le_bytes());
    bmp.extend_from_slice(&(-(size.height as i32)).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&[0; 24]);
    bmp.extend_from_slice(&pixels);
    std::fs::create_dir_all("target").unwrap();
    std::fs::write(path, bmp).expect("write preview bitmap");
    println!("Direct2D preview: {path}");
}

#[test]
#[ignore = "renders the actual island with a hidden HWND and writes preview BMPs"]
fn render_real_bb_island_preview() {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .expect("initialize COM");
        let window = HiddenWindow(
            CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("Tidemark offscreen preview"),
                WS_POPUP,
                0,
                0,
                560,
                500,
                None,
                None,
                None,
                None,
            )
            .expect("hidden preview HWND"),
        );
        let mut renderer =
            Renderer::new(window.0, 560, 500, false).expect("production WARP renderer");
        renderer.ensure_fonts(1.0).unwrap();
        let theme = crate::bb_island::theme();
        let compact = Compact {
            time: "20:30".into(),
            weather: None,
            media: None,
            toast: None,
            timer: None,
        };
        let mut view = sample();
        for (path, compose) in [
            ("target/bb-island-preview.bmp", false),
            ("target/bb-island-compose-preview.bmp", true),
        ] {
            if compose {
                view.meta = "Дополнение к задаче".into();
                view.prompt = "Продолжите разговор с агентом".into();
                view.choices.clear();
                view.pending = 0;
                view.send = "Отправить".into();
                view.field = TextField::with_text(
                    "Добавь тёмную тему и проверь поведение на узком экране.",
                    true,
                );
            }
            let scene = Scene {
                bb: Some(&view),
                shape: Shape {
                    left: 0.0,
                    top: 0.0,
                    width: 560.0,
                    height: 500.0,
                    radius: 24.0,
                },
                wobble: 0.0,
                phase: 0.0,
                theme: &theme,
                content_alpha: 1.0,
                origin: (0.0, 0.0),
                scale: 1.0,
                tiles: &[],
                hovered: None,
                panel_size: (560.0, 500.0),
                compact: &compact,
                compact_alpha: 0.0,
                compact_size: (180.0, 36.0),
                media: None,
                cover: None,
                clip_rows: &[],
                dock: &[],
                hot: None,
                tint: None,
                extra_icons: &[],
                privacy: (false, false),
                window: None,
                tab: Tab::Bb,
                notes: None,
                clip_page: &[],
                search: None,
                shelf: &[],
                text: None,
                control: None,
                recording: false,
            };
            let bitmap = renderer
                .layer_bitmap(
                    None,
                    D2D_SIZE_U {
                        width: 560,
                        height: 500,
                    },
                )
                .unwrap();
            renderer
                .paint_layer(&bitmap, |r| {
                    r.dc.Clear(Some(&color(theme.background, 1.0)));
                    r.paint_home(&scene);
                })
                .expect("paint production home and bb content");
            save_bitmap(&renderer, &bitmap, path);
        }
        drop(renderer);
        drop(window);
        CoUninitialize();
    }
}
