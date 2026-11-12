//! Home tab: live MPRIS card plus honest summaries of the remaining tiles.

use std::cell::RefCell;
use std::collections::VecDeque;

use gtk::glib;
use gtk::prelude::*;
use tidemark_mpris::TrackSnapshot;

use crate::model;
use crate::services::music::{MusicCommand, MusicEvent};
use crate::ui::{card, muted, page_header, section_title, Shared};

pub struct HomeView {
    pub root: gtk::Box,
    shared: Shared,
    snapshot: RefCell<Option<TrackSnapshot>>,
    title: gtk::Label,
    artist: gtk::Label,
    time: gtk::Label,
    progress: gtk::Scale,
    volume: gtk::Scale,
    play: gtk::Button,
    cover_stack: gtk::Stack,
    picture: gtk::Picture,
    textures: RefCell<VecDeque<(u64, gtk::gdk::MemoryTexture)>>,
    status: gtk::Label,
    sleep_status: gtk::Label,
    tiles: gtk::Box,
}

impl HomeView {
    pub fn build(shared: Shared) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
        root.add_css_class("tidemark-page");
        root.append(&page_header(
            "Главная",
            Some("Состояние рабочего стола и быстрые действия"),
        ));

        let music = card();
        music.append(&section_title("Сейчас играет"));

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let cover_stack = gtk::Stack::new();
        cover_stack.set_size_request(64, 64);
        let placeholder = gtk::Image::from_icon_name("audio-x-generic-symbolic");
        placeholder.set_pixel_size(44);
        cover_stack.add_named(&placeholder, Some("placeholder"));
        let picture = gtk::Picture::new();
        picture.set_size_request(64, 64);
        picture.set_can_shrink(true);
        picture.add_css_class("cover");
        cover_stack.add_named(&picture, Some("cover"));
        cover_stack.set_visible_child_name("placeholder");
        top.append(&cover_stack);

        let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
        texts.set_hexpand(true);
        let title = gtk::Label::new(Some("Ничего не играет"));
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.add_css_class("tile-title");
        let artist = gtk::Label::new(Some("Запустите музыку в MPRIS-плеере"));
        artist.set_xalign(0.0);
        artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
        artist.add_css_class("muted");
        let time = gtk::Label::new(Some("0:00"));
        time.set_xalign(0.0);
        time.add_css_class("tiny");
        texts.append(&title);
        texts.append(&artist);
        texts.append(&time);
        top.append(&texts);
        music.append(&top);

        let progress = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.001);
        progress.set_draw_value(false);
        progress.set_hexpand(true);
        progress.set_sensitive(false);
        progress.add_css_class("tidemark");
        music.append(&progress);

        let transport = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        transport.set_halign(gtk::Align::Center);
        let previous = gtk::Button::from_icon_name("media-skip-backward-symbolic");
        previous.set_tooltip_text(Some("Предыдущий трек"));
        let play = gtk::Button::from_icon_name("media-playback-start-symbolic");
        play.set_tooltip_text(Some("Воспроизведение или пауза"));
        let next = gtk::Button::from_icon_name("media-skip-forward-symbolic");
        next.set_tooltip_text(Some("Следующий трек"));
        transport.append(&previous);
        transport.append(&play);
        transport.append(&next);
        music.append(&transport);

        let volume_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let volume_icon = gtk::Image::from_icon_name("audio-volume-medium-symbolic");
        volume_row.append(&volume_icon);
        let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        volume.set_draw_value(false);
        volume.set_hexpand(true);
        volume.set_sensitive(false);
        volume_row.append(&volume);
        music.append(&volume_row);

        let sleep_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        sleep_row.append(&muted("Сон:"));
        for minutes in [15u32, 30, 60] {
            let button = gtk::Button::with_label(&format!("{minutes} мин"));
            let music_service = shared.music.clone();
            button.connect_clicked(move |_| music_service.send(MusicCommand::Sleep(minutes)));
            sleep_row.append(&button);
        }
        let cancel_sleep = gtk::Button::with_label("Отмена");
        let music_service = shared.music.clone();
        cancel_sleep.connect_clicked(move |_| music_service.send(MusicCommand::CancelSleep));
        sleep_row.append(&cancel_sleep);
        music.append(&sleep_row);

        let sleep_status = gtk::Label::new(Some("Таймер сна не задан"));
        sleep_status.set_xalign(0.0);
        sleep_status.add_css_class("tiny");
        music.append(&sleep_status);

        let status = gtk::Label::new(Some(if shared.smoke {
            "Режим проверки: MPRIS не запускается."
        } else {
            "MPRIS: ожидание плеера."
        }));
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.add_css_class("tiny");
        music.append(&status);

        let refresh = gtk::Button::with_label("Обновить");
        {
            let service = shared.music.clone();
            refresh.connect_clicked(move |_| service.send(MusicCommand::Refresh));
        }
        music.append(&refresh);

        shake_handles(&shared, &previous, &play, &next, &progress, &volume, &title);
        root.append(&music);

        let tiles = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.append(&tiles);

        if shared.config.borrow().beat_edge {
            root.append(&muted(
                "Пульсация края недоступна: Linux-версия не анализирует аудиопоток.",
            ));
        }

        let view = Self {
            root,
            shared,
            snapshot: RefCell::new(None),
            title,
            artist,
            time,
            progress,
            volume,
            play,
            cover_stack,
            picture,
            textures: RefCell::new(VecDeque::new()),
            status,
            sleep_status,
            tiles,
        };
        view.refresh_tiles();
        view
    }

    pub fn handle_event(&self, event: &MusicEvent) {
        match event {
            MusicEvent::Snapshot(Some(now)) => {
                let snapshot = &now.snapshot;
                self.title.set_text(if snapshot.title.is_empty() {
                    "Без названия"
                } else {
                    &snapshot.title
                });
                let artists = snapshot.artists.join(", ");
                let subtitle = if artists.is_empty() {
                    snapshot.album.clone().unwrap_or_default()
                } else {
                    artists
                };
                self.artist.set_text(&subtitle);
                self.play.set_icon_name(if snapshot.playing {
                    "media-playback-pause-symbolic"
                } else {
                    "media-playback-start-symbolic"
                });
                self.progress
                    .set_sensitive(snapshot.can_seek && snapshot.duration.is_some());
                self.volume.set_sensitive(snapshot.volume.is_some());
                if let Some(volume) = snapshot.volume {
                    self.set_scale(&self.volume, volume as f64);
                }
                match &now.cover {
                    Some(cover) => self.show_cover(cover),
                    None => self.cover_stack.set_visible_child_name("placeholder"),
                }
                self.status.remove_css_class("danger");
                self.status
                    .set_text(&format!("MPRIS: {}", snapshot.player.bus_name));
                self.artist
                    .set_tooltip_text(Some(&snapshot.player.to_string()));
                self.snapshot.replace(Some(snapshot.clone()));
                self.tick();
            }
            MusicEvent::Snapshot(None) => {
                self.title.set_text("Ничего не играет");
                self.artist.set_text("Запустите музыку в MPRIS-плеере");
                self.time.set_text("0:00");
                self.play.set_icon_name("media-playback-start-symbolic");
                self.progress.set_sensitive(false);
                self.progress.set_value(0.0);
                self.volume.set_sensitive(false);
                self.cover_stack.set_visible_child_name("placeholder");
                self.status.remove_css_class("danger");
                self.status.set_text("MPRIS: плееров не найдено.");
                self.snapshot.replace(None);
            }
            MusicEvent::Error(message) => {
                self.status.add_css_class("danger");
                self.status.set_text(message);
            }
            MusicEvent::SleepArmed(minutes) => {
                self.sleep_status
                    .set_text(&format!("Сон через {minutes} мин; плеер закреплён."));
            }
            MusicEvent::SleepCancelled => {
                self.sleep_status.set_text("Таймер сна отменён.");
            }
            MusicEvent::SleepFired => {
                self.sleep_status
                    .set_text("Таймер сна сработал: воспроизведение приостановлено.");
            }
        }
    }

    pub fn tick(&self) {
        let snapshot = self.snapshot.borrow();
        let Some(snapshot) = snapshot.as_ref() else {
            return;
        };
        let position = snapshot.position_now();
        if let Some(duration) = snapshot.duration {
            let total = duration.as_secs_f64();
            let fraction = if total > 0.0 {
                (position.as_secs_f64() / total).clamp(0.0, 1.0)
            } else {
                0.0
            };
            self.set_scale(&self.progress, fraction);
        }
        self.time
            .set_text(&model::progress_label(position, snapshot.duration));
    }

    pub fn now_playing(&self) -> Option<String> {
        self.snapshot.borrow().as_ref().map(|snapshot| {
            if snapshot.title.is_empty() {
                "Музыка".to_string()
            } else if snapshot.artists.is_empty() {
                snapshot.title.clone()
            } else {
                format!("{} — {}", snapshot.title, snapshot.artists.join(", "))
            }
        })
    }

    fn show_cover(&self, cover: &tidemark_artwork::Cover) {
        let mut textures = self.textures.borrow_mut();
        let texture = if let Some(index) = textures.iter().position(|(id, _)| *id == cover.id) {
            let (_, texture) = textures.remove(index).expect("existing cache index");
            texture
        } else {
            let size = tidemark_artwork::COVER_PX as usize;
            if cover.pixels.len() != size * size * 4 {
                self.cover_stack.set_visible_child_name("placeholder");
                return;
            }
            let bytes = glib::Bytes::from_owned(cover.pixels.clone());
            gtk::gdk::MemoryTexture::new(
                size as i32,
                size as i32,
                gtk::gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                size * 4,
            )
        };
        self.picture.set_paintable(Some(&texture));
        self.cover_stack.set_visible_child_name("cover");
        textures.push_back((cover.id, texture));
        while textures.len() > 8 {
            textures.pop_front();
        }
    }

    fn set_scale(&self, scale: &gtk::Scale, value: f64) {
        scale.set_value(value);
    }

    pub fn refresh_tiles(&self) {
        while let Some(child) = self.tiles.first_child() {
            self.tiles.remove(&child);
        }
        let ids = model::visible_home_tiles(&self.shared.config.borrow());
        let grid = gtk::Grid::new();
        grid.set_column_spacing(8);
        grid.set_row_spacing(8);
        grid.set_column_homogeneous(true);
        for (index, id) in ids.iter().enumerate() {
            let tile = self.tile(id);
            grid.attach(&tile, (index % 2) as i32, (index / 2) as i32, 1, 1);
        }
        self.tiles.append(&grid);
        self.tiles.append(&muted(
            "Музыка и буфер доступны сейчас. Окно, инструменты, погода и док требуют \
             адаптера GNOME Shell; кнопки для них не имитируют успех.",
        ));
    }

    fn tile(&self, id: &str) -> gtk::Box {
        let tile = card();
        let (name, description) = model::HOME_TILES
            .iter()
            .find(|(tile_id, _, _)| *tile_id == id)
            .map(|(_, name, description)| (*name, *description))
            .unwrap_or(("Плитка", "Описание отсутствует"));
        let title = gtk::Label::new(Some(name));
        title.set_xalign(0.0);
        title.add_css_class("tile-title");
        tile.append(&title);

        let value = match id {
            "media" => self
                .snapshot
                .borrow()
                .as_ref()
                .map(|snapshot| short(&snapshot.title, 30))
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| "Нет трека".into()),
            "clipboard" => format!("{} в истории", self.shared.clip.borrow().items.len()),
            "window" => "Нет доступа".into(),
            "tools" => "Недоступно".into(),
            "weather" => {
                if self.shared.config.borrow().weather_city.trim().is_empty() {
                    "Выключено".into()
                } else {
                    "Не подключено".into()
                }
            }
            "apps" => "Недоступно".into(),
            _ => String::new(),
        };
        let value_label = gtk::Label::new(Some(&value));
        value_label.set_xalign(0.0);
        value_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        value_label.add_css_class("tile-value");
        tile.append(&value_label);

        let description = match id {
            "window" => "Нужен адаптер GNOME Shell: активное окно и действия с ним.",
            "tools" => "Захват экрана, OCR и QR отключены до адаптера.",
            "weather" => "Сетевой прогноз пока не реализован в Linux-версии.",
            "apps" => "Док приложений требует интеграции с рабочим столом.",
            _ => description,
        };
        tile.append(&muted(description));
        tile
    }
}

fn shake_handles(
    shared: &Shared,
    previous: &gtk::Button,
    play: &gtk::Button,
    next: &gtk::Button,
    progress: &gtk::Scale,
    volume: &gtk::Scale,
    _title: &gtk::Label,
) {
    for (button, command) in [
        (previous, MusicCommand::Previous),
        (play, MusicCommand::PlayPause),
        (next, MusicCommand::Next),
    ] {
        let service = shared.music.clone();
        button.set_sensitive(true);
        button.connect_clicked(move |_| service.send(command.clone()));
    }
    let service = shared.music.clone();
    progress.connect_change_value(move |_, _, value| {
        service.send(MusicCommand::Seek(value));
        glib::Propagation::Stop
    });
    let service = shared.music.clone();
    volume.connect_change_value(move |_, _, value| {
        service.send(MusicCommand::SetVolume(value));
        glib::Propagation::Stop
    });
}

fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!(
            "{}…",
            text.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}
