//! Settings window. Every control writes the shared `Config` immediately and
//! applies visual changes without restarting the island.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::prelude::*;
use tidemark_core::config::Config;

use crate::model;
use crate::services::autostart;
use crate::ui::{muted, Shared};

pub fn build(app: &adw::Application, shared: Shared) -> adw::PreferencesWindow {
    let window = adw::PreferencesWindow::builder()
        .application(app)
        .title("Tidemark — настройки")
        .default_width(660)
        .default_height(760)
        .build();

    window.add(&appearance_page(&shared));
    window.add(&behaviour_page(&shared));
    window.add(&system_page(&shared));
    crate::ui::install_hand_cursors(&window);
    window
}

fn appearance_page(shared: &Shared) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title("Внешний вид");
    page.set_icon_name(Some("applications-graphics-symbolic"));

    let island = adw::PreferencesGroup::new();
    island.set_title("Остров");

    let theme = adw::ComboRow::new();
    theme.set_title("Тема");
    theme.set_subtitle("Как в системе, принудительно тёмная или светлая");
    let list = gtk::StringList::new(&model::THEME_LABELS);
    theme.set_model(Some(&list));
    theme.set_selected(theme_index(&shared.config.borrow()));
    {
        let shared = shared.clone();
        theme.connect_selected_notify(move |row| {
            let index = row.selected() as usize;
            let value = model::THEME_VALUES.get(index).copied().unwrap_or("system");
            shared.config.borrow_mut().theme = value.to_string();
            shared.save_config();
            shared.reload_css();
        });
    }
    island.add(&theme);

    let accent = adw::EntryRow::new();
    accent.set_title("Акцент");
    accent.set_text(&shared.config.borrow().accent);
    {
        let shared = shared.clone();
        accent.connect_changed(move |row| {
            if let Some(value) = model::normalize_hex(&row.text()) {
                shared.config.borrow_mut().accent = value;
                shared.save_config();
                shared.reload_css();
            }
        });
    }
    island.add(&accent);

    let background = adw::EntryRow::new();
    background.set_title("Фон острова");
    background.set_text(&shared.config.borrow().background);
    background.set_tooltip_text(Some("Пусто — цвет темы. Иначе #RRGGBB."));
    {
        let shared = shared.clone();
        background.connect_changed(move |row| {
            let text = row.text().to_string();
            if text.trim().is_empty() {
                shared.config.borrow_mut().background.clear();
                shared.save_config();
                shared.reload_css();
            } else if let Some(value) = model::normalize_hex(&text) {
                shared.config.borrow_mut().background = value;
                shared.save_config();
                shared.reload_css();
            }
        });
    }
    island.add(&background);

    let opacity_row = adw::ActionRow::new();
    opacity_row.set_title("Непрозрачность");
    opacity_row.set_subtitle("0,2 — почти прозрачный, 1,0 — непрозрачный");
    let opacity = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.2, 1.0, 0.01);
    opacity.set_value(shared.config.borrow().opacity.clamp(0.2, 1.0) as f64);
    opacity.set_size_request(180, -1);
    opacity.set_draw_value(false);
    {
        let shared = shared.clone();
        opacity.connect_value_changed(move |scale| {
            shared.config.borrow_mut().opacity = scale.value() as f32;
            shared.save_config();
            shared.reload_css();
        });
    }
    opacity_row.add_suffix(&opacity);
    island.add(&opacity_row);

    spin_row(
        &island,
        "Ширина свёрнутого острова",
        "DIP",
        100.0,
        1000.0,
        shared.config.borrow().collapsed_width as f64,
        {
            let shared = shared.clone();
            move |value| {
                shared.config.borrow_mut().collapsed_width = value as f32;
                shared.save_config();
            }
        },
    );
    spin_row(
        &island,
        "Высота свёрнутого острова",
        "DIP",
        20.0,
        200.0,
        shared.config.borrow().collapsed_height as f64,
        {
            let shared = shared.clone();
            move |value| {
                shared.config.borrow_mut().collapsed_height = value as f32;
                shared.save_config();
            }
        },
    );
    spin_row(
        &island,
        "Ширина раскрытого острова",
        "DIP",
        300.0,
        1400.0,
        shared.config.borrow().expanded_width as f64,
        {
            let shared = shared.clone();
            move |value| {
                shared.config.borrow_mut().expanded_width = value as f32;
                shared.save_config();
            }
        },
    );
    spin_row(
        &island,
        "Радиус раскрытого острова",
        "DIP",
        0.0,
        64.0,
        shared.config.borrow().expanded_radius as f64,
        {
            let shared = shared.clone();
            move |value| {
                shared.config.borrow_mut().expanded_radius = value as f32;
                shared.save_config();
            }
        },
    );
    spin_row(
        &island,
        "Отступ сверху",
        "DIP",
        0.0,
        200.0,
        shared.config.borrow().top_offset as f64,
        {
            let shared = shared.clone();
            move |value| {
                shared.config.borrow_mut().top_offset = value as f32;
                shared.save_config();
            }
        },
    );
    spin_row(
        &island,
        "Задержка раскрытия",
        "мс",
        0.0,
        5000.0,
        shared.config.borrow().hover_delay_ms as f64,
        {
            let shared = shared.clone();
            move |value| {
                shared.config.borrow_mut().hover_delay_ms = value as u32;
                shared.save_config();
            }
        },
    );

    page.add(&island);

    let tiles = adw::PreferencesGroup::new();
    tiles.set_title("Плитки главной");
    tiles.set_description(Some(
        "Выберите плитки главной. Для погоды укажите город, для управления окнами подключите расширение GNOME.",
    ));
    for (id, name, description) in model::HOME_TILES {
        let row = adw::SwitchRow::new();
        row.set_title(name);
        row.set_subtitle(description);
        row.set_active(
            shared
                .config
                .borrow()
                .home_tiles
                .iter()
                .any(|tile| tile == id),
        );
        let shared = shared.clone();
        row.connect_active_notify(move |row| set_tile(&shared, id, row.is_active()));
        tiles.add(&row);
    }
    page.add(&tiles);

    page
}

fn behaviour_page(shared: &Shared) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title("Поведение");
    page.set_icon_name(Some("preferences-system-symbolic"));

    let switches = adw::PreferencesGroup::new();
    switches.set_title("Возможности");

    for (title, subtitle, get, set) in behaviour_switches() {
        let row = adw::SwitchRow::new();
        row.set_title(title);
        row.set_subtitle(subtitle);
        row.set_active(get(&shared.config.borrow()));
        let shared = shared.clone();
        let clipboard_setting = title == "История буфера обмена";
        row.connect_active_notify(move |row| {
            set(&mut shared.config.borrow_mut(), row.is_active());
            if clipboard_setting {
                shared.shell.borrow().set_clipboard_capture(row.is_active());
            }
            shared.save_config();
        });
        switches.add(&row);
    }
    page.add(&switches);

    let network = adw::PreferencesGroup::new();
    network.set_title("Погода и поиск");
    network.set_description(Some(
        "Погода не реализована в Linux-версии; строка поиска пока только сохраняется.",
    ));
    let city = adw::EntryRow::new();
    city.set_title("Город для погоды");
    city.set_text(&shared.config.borrow().weather_city);
    {
        let shared = shared.clone();
        city.connect_changed(move |row| {
            shared.config.borrow_mut().weather_city = row.text().to_string();
            shared.save_config();
        });
    }
    network.add(&city);
    let search = adw::EntryRow::new();
    search.set_title("Шаблон поиска");
    search.set_text(&shared.config.borrow().search_url);
    {
        let shared = shared.clone();
        search.connect_changed(move |row| {
            shared.config.borrow_mut().search_url = row.text().to_string();
            shared.save_config();
        });
    }
    network.add(&search);
    page.add(&network);

    page
}

fn system_page(shared: &Shared) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title("Система");
    page.set_icon_name(Some("computer-symbolic"));

    let startup = adw::PreferencesGroup::new();
    startup.set_title("Запуск");
    let autostart_row = adw::SwitchRow::new();
    autostart_row.set_title("Запускать при входе");
    autostart_row.set_subtitle("Файл ~/.config/autostart/tidemark.desktop");
    autostart_row.set_active(autostart::is_enabled());
    let guard = Rc::new(Cell::new(false));
    {
        let guard = guard.clone();
        autostart_row.connect_active_notify(move |row| {
            if guard.get() {
                return;
            }
            if let Err(error) = autostart::set_enabled(row.is_active()) {
                eprintln!("Tidemark autostart: {error}");
                guard.set(true);
                row.set_active(!row.is_active());
                guard.set(false);
            }
        });
    }
    startup.add(&autostart_row);
    page.add(&startup);

    let adapter = adw::PreferencesGroup::new();
    adapter.set_title("GNOME Shell");
    let state = adw::ActionRow::new();
    state.set_title("Адаптер org.tidemark.Shell1");
    state.set_subtitle(&shared.shell_status.borrow());
    let weak = state.downgrade();
    let status = shared.shell_status.clone();
    gtk::glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
        let Some(row) = weak.upgrade() else {
            return gtk::glib::ControlFlow::Break;
        };
        row.set_subtitle(&status.borrow());
        gtk::glib::ControlFlow::Continue
    });
    adapter.add(&state);
    let contract = adw::ActionRow::new();
    contract.set_title("Контракт");
    contract.set_subtitle("Версия 1 · org.tidemark.Shell · /org/tidemark/Shell");
    adapter.add(&contract);
    let missing = muted(&format!(
        "Недоступно без моста: {}.",
        model::missing_capabilities(&[])
            .iter()
            .map(|capability| model::capability_text(*capability).0)
            .collect::<Vec<_>>()
            .join(", ")
    ));
    adapter.add(&missing);
    page.add(&adapter);

    let about = adw::PreferencesGroup::new();
    about.set_title("О приложении");
    let version = adw::ActionRow::new();
    version.set_title("Tidemark");
    version.set_subtitle(&format!("Версия {}", env!("CARGO_PKG_VERSION")));
    about.add(&version);
    let app_id = adw::ActionRow::new();
    app_id.set_title("Идентификатор приложения");
    app_id.set_subtitle("io.github.treyzar.Tidemark");
    about.add(&app_id);
    page.add(&about);

    page
}

fn theme_index(config: &Config) -> u32 {
    model::THEME_VALUES
        .iter()
        .position(|value| *value == config.theme)
        .unwrap_or(0) as u32
}

fn set_tile(shared: &Shared, id: &str, enabled: bool) {
    let mut config = shared.config.borrow_mut();
    let present = config.home_tiles.iter().any(|tile| tile == id);
    if enabled && !present {
        config.home_tiles.push(id.to_string());
    } else if !enabled && present {
        config.home_tiles.retain(|tile| tile != id);
    }
    drop(config);
    shared.save_config();
}

fn spin_row<F>(
    group: &adw::PreferencesGroup,
    title: &str,
    suffix: &str,
    min: f64,
    max: f64,
    value: f64,
    changed: F,
) where
    F: Fn(f64) + 'static,
{
    let row = adw::ActionRow::new();
    row.set_title(title);
    let spin = gtk::SpinButton::with_range(min, max, 1.0);
    spin.set_value(value);
    spin.set_numeric(true);
    spin.set_tooltip_text(Some(suffix));
    row.add_suffix(&spin);
    let unit = gtk::Label::new(Some(suffix));
    unit.add_css_class("muted");
    row.add_suffix(&unit);
    spin.connect_value_changed(move |spin| changed(spin.value()));
    group.add(&row);
}

type BoolGetter = fn(&Config) -> bool;
type BoolSetter = fn(&mut Config, bool);
type SwitchSpec = (&'static str, &'static str, BoolGetter, BoolSetter);

fn behaviour_switches() -> [SwitchSpec; 9] {
    [
        (
            "История буфера обмена",
            "Фоновая запись появится с адаптером GNOME Shell; ручное добавление работает.",
            |config| config.clipboard_history,
            |config, value| config.clipboard_history = value,
        ),
        (
            "Прятать в полноэкранном режиме",
            "Требует отслеживания окон через адаптер.",
            |config| config.hide_in_fullscreen,
            |config, value| config.hide_in_fullscreen = value,
        ),
        (
            "Сжимать над развёрнутым окном",
            "Требует отслеживания окон через адаптер.",
            |config| config.shrink_under_windows,
            |config, value| config.shrink_under_windows = value,
        ),
        (
            "Показывать активное приложение",
            "Требует адаптер GNOME Shell.",
            |config| config.show_active_app,
            |config, value| config.show_active_app = value,
        ),
        (
            "Уведомления системы",
            "Громкость, яркость, раскладка и Caps Lock приходят от адаптера.",
            |config| config.osd,
            |config, value| config.osd = value,
        ),
        (
            "Индикатор приватности",
            "Камера и микрофон: требуется адаптер.",
            |config| config.privacy_indicator,
            |config, value| config.privacy_indicator = value,
        ),
        (
            "Поиск по Super+Alt+Space",
            "Приложения, файлы, калькулятор и поиск в браузере; сочетание регистрирует GNOME.",
            |config| config.launcher,
            |config, value| config.launcher = value,
        ),
        (
            "Пульсация края",
            "На Linux аудиопоток пока не анализируется.",
            |config| config.beat_edge,
            |config, value| config.beat_edge = value,
        ),
        (
            "Аппаратный GPU",
            "Настройка Windows-рендерера; GTK использует свой рендерер.",
            |config| config.gpu,
            |config, value| config.gpu = value,
        ),
    ]
}
