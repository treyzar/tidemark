//! Home tab: MPRIS, configured weather and explicitly requested online lyrics.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
#[path = "../services/home_online.rs"]
mod online;
use online::{Latest, Request, TrackKey, WeatherSchedule};

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
    online: Rc<RefCell<OnlineHome>>,
    lyrics_button: gtk::Button,
    lyrics_status: gtk::Label,
    lyrics_buffer: gtk::TextBuffer,
    lyrics_scroll: gtk::ScrolledWindow,
    title: gtk::Label,
    artist: gtk::Label,
    time: gtk::Label,
    progress: gtk::Scale,
    volume: gtk::Scale,
    play: gtk::Button,
    open_player: gtk::Button,
    open_target: Rc<RefCell<Option<tidemark_mpris::MprisPlayerId>>>,
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
        let open_player = gtk::Button::with_label("Плеер");
        open_player.set_tooltip_text(Some("Открыть текущий плеер"));
        open_player.set_sensitive(false);
        let open_target = Rc::new(RefCell::new(None::<tidemark_mpris::MprisPlayerId>));
        {
            let target = open_target.clone();
            let service = shared.music.clone();
            open_player.connect_clicked(move |_| {
                if let Some(player) = target.borrow().as_ref() {
                    service.send(MusicCommand::Raise(player.clone()));
                }
            });
        }
        transport.append(&open_player);
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

        let online = Rc::new(RefCell::new(OnlineHome::default()));
        let lyrics_button = gtk::Button::with_label("Текст песни онлайн");
        lyrics_button.set_sensitive(false);
        lyrics_button.set_tooltip_text(Some(
            "Отправить название и исполнителя текущего трека в LRCLIB.",
        ));
        let lyrics_status =
            gtk::Label::new(Some("Выберите трек, затем запросите текст по кнопке."));
        lyrics_status.set_xalign(0.0);
        lyrics_status.set_wrap(true);
        lyrics_status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        lyrics_status.set_max_width_chars(1);
        lyrics_status.add_css_class("tiny");
        let lyrics_buffer = gtk::TextBuffer::new(None);
        let lyrics_view = gtk::TextView::with_buffer(&lyrics_buffer);
        lyrics_view.set_editable(false);
        lyrics_view.set_cursor_visible(false);
        lyrics_view.set_wrap_mode(gtk::WrapMode::WordChar);
        let lyrics_scroll = gtk::ScrolledWindow::new();
        lyrics_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        lyrics_scroll.set_size_request(-1, 160);
        lyrics_scroll.set_child(Some(&lyrics_view));
        lyrics_scroll.set_visible(false);
        music.append(&lyrics_button);
        music.append(&lyrics_status);
        music.append(&lyrics_scroll);
        {
            let online = online.clone();
            let smoke = shared.smoke;
            lyrics_button.connect_clicked(move |button| {
                if smoke {
                    return;
                }
                let mut online = online.borrow_mut();
                online.lyrics_requested = online.lyrics.key().cloned();
                button.set_sensitive(false);
            });
        }

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
            online,
            lyrics_button,
            lyrics_status,
            lyrics_buffer,
            lyrics_scroll,
            title,
            artist,
            time,
            progress,
            volume,
            play,
            open_player,
            open_target,
            cover_stack,
            picture,
            textures: RefCell::new(VecDeque::new()),
            status,
            sleep_status,
            tiles,
        };
        view.refresh_tiles();
        view.tick_online();
        view
    }

    pub fn handle_event(&self, event: &MusicEvent) {
        match event {
            MusicEvent::Snapshot(Some(now)) => {
                let snapshot = &now.snapshot;
                self.open_target.replace(Some(snapshot.player.clone()));
                self.open_player.set_sensitive(true);
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
                self.open_target.replace(None);
                self.open_player.set_sensitive(false);
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
        self.tick_online();
    }

    pub fn tick(&self) {
        self.tick_online();
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

    fn tick_online(&self) {
        let city = self.shared.config.borrow().weather_city.clone();
        let track = self.snapshot.borrow().as_ref().map(track_key);
        let mut state = self.online.borrow_mut();
        let mut weather_dirty = false;
        let mut lyrics_dirty = false;
        if state.weather.set_city(&city) {
            state.weather_value = if city.trim().is_empty() {
                "Выключено"
            } else {
                "Ожидание прогноза…"
            }
            .into();
            state.weather_detail = if city.trim().is_empty() {
                "Укажите город в настройках, чтобы включить погоду.".into()
            } else {
                format!("Город: {}", city.trim())
            };
            weather_dirty = true;
        }
        if state.lyrics.set(track) {
            state.lyrics_requested = None;
            state.lyrics_text = None;
            state.lyrics_message = "Трек изменён. Запросите текст по кнопке.".into();
            lyrics_dirty = true;
        }
        if let Some((request, result)) = state
            .weather_job
            .as_ref()
            .and_then(|(r, rx)| poll(rx).map(|result| (r.clone(), result)))
        {
            state.weather_job = None;
            let result = result.unwrap_or(Err(tidemark_net::Error::Transport));
            if let Err(tidemark_net::Error::RateLimited { retry_after }) = &result {
                state.weather.defer(Instant::now(), *retry_after);
            }
            if state
                .weather
                .finish(&request, Instant::now(), result.is_ok())
            {
                match result {
                    Ok(Some(weather)) => {
                        state.weather_value = format!(
                            "{} · {}",
                            weather.temperature_label(),
                            weather_name(weather.icon)
                        );
                        state.weather_detail = format!(
                            "{} · {} · автообновление: 20 мин",
                            request.key,
                            if weather.is_day {
                                "день"
                            } else {
                                "ночь"
                            }
                        );
                    }
                    Ok(None) => {
                        state.weather_value = "Выключено".into();
                    }
                    Err(error) => {
                        state.weather_value = "Нет свежего прогноза".into();
                        state.weather_detail = format!(
                            "{}: {error}. Автоповтор через 5 мин. или после ограничения сервиса.",
                            request.key
                        );
                    }
                }
                weather_dirty = true;
            }
        }
        if let Some((request, result)) = state
            .lyrics_job
            .as_ref()
            .and_then(|(r, rx)| poll(rx).map(|result| (r.clone(), result)))
        {
            state.lyrics_job = None;
            let result = result.unwrap_or(Err(tidemark_net::Error::Transport));
            if let Err(tidemark_net::Error::RateLimited { retry_after }) = &result {
                state.lyrics_retry_at =
                    Some(Instant::now() + (*retry_after).min(Duration::from_secs(365 * 24 * 3600)));
            }
            if state.lyrics.finish(&request) {
                match result {
                    Ok(Some(lyrics)) => {
                        state.lyrics_message = if lyrics.instrumental {
                            "Инструментальный трек."
                        } else if !lyrics.synced.is_empty() {
                            "Текст LRCLIB с таймкодами."
                        } else {
                            "Текст LRCLIB без таймкодов."
                        }
                        .into();
                        state.lyrics_text = Some(lyrics);
                    }
                    Ok(None) => {
                        state.lyrics_message = "В LRCLIB текст этого трека не найден.".into();
                    }
                    Err(error) => {
                        state.lyrics_message =
                            format!("Не удалось загрузить текст: {error}. Можно повторить запрос.");
                    }
                }
            } else {
                state.lyrics_message =
                    "Предыдущий трек изменён — результат не показан. Запросите текст снова.".into();
            }
            lyrics_dirty = true;
        }
        if !self.shared.smoke {
            let manual = std::mem::take(&mut state.weather_manual);
            if let Some(request) = state.weather.begin(Instant::now(), manual, false) {
                let (tx, rx) = mpsc::sync_channel(1);
                let city = request.key.clone();
                let city_label = city.clone();
                if std::thread::Builder::new()
                    .name("home-weather".into())
                    .spawn(move || {
                        let result = tidemark_net::Client::new().weather(&city);
                        let _ = tx.try_send(result);
                    })
                    .is_ok()
                {
                    state.weather_job = Some((request, rx));
                    state.weather_value = "Загружаем погоду…".into();
                    state.weather_detail = format!("{} · запрос прогноза", city_label);
                } else {
                    state.weather.finish(&request, Instant::now(), false);
                    state.weather_value = "Погода недоступна".into();
                    state.weather_detail =
                        "Не удалось запустить запрос. Автоповтор через 5 мин. или после ограничения сервиса.".into();
                }
                weather_dirty = true;
            }
            if let Some(wanted) = state.lyrics_requested.take() {
                if state.lyrics.key() == Some(&wanted)
                    && state
                        .lyrics_retry_at
                        .is_none_or(|until| Instant::now() >= until)
                {
                    if let Some(request) = state.lyrics.begin() {
                        let query = tidemark_net::lyrics::TrackQuery {
                            artist: request.key.artists.join(", "),
                            title: request.key.title.clone(),
                            duration: request.key.duration.unwrap_or_default(),
                        };
                        let (tx, rx) = mpsc::sync_channel(1);
                        if std::thread::Builder::new()
                            .name("home-lyrics".into())
                            .spawn(move || {
                                let result = tidemark_net::Client::new().lyrics(&query);
                                let _ = tx.try_send(result);
                            })
                            .is_ok()
                        {
                            state.lyrics_job = Some((request, rx));
                            state.lyrics_text = None;
                            state.lyrics_message = "Ищем текст в LRCLIB…".into();
                        } else {
                            state.lyrics.finish(&request);
                            state.lyrics_message = "Не удалось запустить запрос текста.".into();
                        }
                        lyrics_dirty = true;
                    }
                }
            }
        } else if !city.trim().is_empty() {
            state.weather_value = "Режим проверки".into();
            state.weather_detail = "Сетевые запросы отключены.".into();
        }
        self.lyrics_button.set_sensitive(
            !self.shared.smoke
                && !state.lyrics.busy()
                && state
                    .lyrics_retry_at
                    .is_none_or(|until| Instant::now() >= until)
                && state.lyrics.key().is_some_and(|track| {
                    !track.title.trim().is_empty() && !track.artists.is_empty()
                }),
        );
        if lyrics_dirty {
            self.lyrics_status.set_text(&state.lyrics_message);
            if let Some(lyrics) = &state.lyrics_text {
                let text = lyrics_display(lyrics);
                self.lyrics_buffer.set_text(&text);
                self.lyrics_scroll.set_visible(!text.is_empty());
            } else {
                self.lyrics_buffer.set_text("");
                self.lyrics_scroll.set_visible(false);
            }
        }
        drop(state);
        if weather_dirty {
            self.refresh_tiles();
        }
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
            "Музыка, буфер и онлайн-погода доступны сейчас. Действия с окнами, захват экрана \
             и док пока недоступны в этой сборке.",
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
            "weather" => self.online.borrow().weather_value.clone(),
            "apps" => "Недоступно".into(),
            _ => String::new(),
        };
        let value_label = gtk::Label::new(Some(&value));
        value_label.set_xalign(0.0);
        value_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        value_label.add_css_class("tile-value");
        tile.append(&value_label);

        let description = match id {
            "window" => "Действия с активным окном пока не подключены.",
            "tools" => "Захват экрана, OCR и QR пока недоступны в этой сборке.",
            "weather" => "Open-Meteo; при недоступности — wttr.in.",
            "apps" => "Док приложений пока недоступен в этой сборке.",
            _ => description,
        };
        let detail = if id == "weather" {
            self.online.borrow().weather_detail.clone()
        } else {
            description.to_owned()
        };
        let description = muted(&detail);
        description.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        description.set_max_width_chars(1);
        tile.append(&description);
        if id == "weather" {
            let refresh = gtk::Button::with_label("Обновить погоду");
            let state = self.online.borrow();
            refresh.set_sensitive(
                !self.shared.smoke && state.weather.city().is_some() && !state.weather.busy(),
            );
            drop(state);
            let online = self.online.clone();
            let smoke = self.shared.smoke;
            refresh.connect_clicked(move |_| {
                if smoke {
                    return;
                }
                online.borrow_mut().weather_manual = true;
            });
            tile.append(&refresh);
        }
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

type WeatherResult = tidemark_net::Result<Option<tidemark_net::weather::Weather>>;
type LyricsResult = tidemark_net::Result<Option<tidemark_net::lyrics::Lyrics>>;
struct OnlineHome {
    weather: WeatherSchedule,
    weather_job: Option<(Request<String>, Receiver<WeatherResult>)>,
    weather_manual: bool,
    weather_value: String,
    weather_detail: String,
    lyrics: Latest<TrackKey>,
    lyrics_job: Option<(Request<TrackKey>, Receiver<LyricsResult>)>,
    lyrics_requested: Option<TrackKey>,
    lyrics_retry_at: Option<Instant>,
    lyrics_text: Option<tidemark_net::lyrics::Lyrics>,
    lyrics_message: String,
}
impl Default for OnlineHome {
    fn default() -> Self {
        Self {
            weather: WeatherSchedule::default(),
            weather_job: None,
            weather_manual: false,
            weather_value: "Выключено".into(),
            weather_detail: "Укажите город в настройках, чтобы включить погоду.".into(),
            lyrics: Latest::default(),
            lyrics_job: None,
            lyrics_requested: None,
            lyrics_retry_at: None,
            lyrics_text: None,
            lyrics_message: "Выберите трек, затем запросите текст по кнопке.".into(),
        }
    }
}
fn track_key(track: &TrackSnapshot) -> TrackKey {
    TrackKey {
        bus_name: track.player.bus_name.clone(),
        owner: track.player.owner.clone(),
        track_id: track.track_id.clone(),
        title: track.title.clone(),
        artists: track.artists.clone(),
        album: track.album.clone(),
        duration: track.duration,
    }
}
fn poll<T>(rx: &Receiver<T>) -> Option<Result<T, ()>> {
    match rx.try_recv() {
        Ok(result) => Some(Ok(result)),
        Err(mpsc::TryRecvError::Empty) => None,
        Err(mpsc::TryRecvError::Disconnected) => Some(Err(())),
    }
}
fn weather_name(icon: tidemark_net::weather::Icon) -> &'static str {
    use tidemark_net::weather::Icon;
    match icon {
        Icon::Clear => "Ясно",
        Icon::PartlyCloudy => "Малооблачно",
        Icon::Cloudy => "Облачно",
        Icon::Fog => "Туман",
        Icon::Drizzle => "Морось",
        Icon::Rain => "Дождь",
        Icon::Snow => "Снег",
        Icon::Thunder => "Гроза",
    }
}
fn lyrics_display(lyrics: &tidemark_net::lyrics::Lyrics) -> String {
    if lyrics.synced.is_empty() {
        return lyrics.plain.clone().unwrap_or_default();
    }
    use std::fmt::Write;
    let mut text = String::new();
    for line in &lyrics.synced {
        let seconds = line.at.as_secs();
        let _ = writeln!(text, "[{}:{:02}] {}", seconds / 60, seconds % 60, line.text);
    }
    text
}
