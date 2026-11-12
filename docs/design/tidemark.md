# Tidemark

Tidemark («линия прилива») — новое отображаемое имя Win Island. Технические имена exe, конфигурации, singleton, реестра и интеграций сохранены для совместимости.

## Направление

- Продукт: остров у края рабочего стола Windows с музыкой и быстрыми действиями.
- Характер: спокойный, тактильный, собранный.
- Мир: берег, мокрый камень, ровная вода, тёплый утренний свет.
- Основной визуальный элемент: превью острова на прибрежной фотографии.
- Отличительная деталь: горизонтальная капсула с отдельной точкой, повторяющая форму острова.

Нативный Win32/GDI-интерфейс, Segoe UI, сетка 4 px. Светлый фон #FAF9F6, боковая панель #F0EFEB, основной текст #292D2C, вторичный #646B67, акцент #A54831. В настройках нет постоянного цикла анимации. Рендер острова и его частота кадров не менялись.

## Аудит и изменения

Раньше все настройки были в одном плотном окне, с рамками групп, мелкими стандартными контролами, слабой иерархией и системной иконкой. Теперь доступны четыре раздела с общей боковой навигацией, пояснениями к каждому параметру и закреплённой кнопкой применения. Сохраняются все прежние параметры, в том числе скрытые от интерфейса. Переключатели и ползунок используют нативную обработку клавиатуры и доступность. Цвета и тема отражаются в демонстрационном превью; фактический остров обновляется после применения.

## Панель bb (1.4.1)

Использует те же цвета, Segoe UI и GDI-примитивы, что и настройки. В основном экране — компактный выбор разговора и действия, отдельная поверхность вопроса, карточки с описаниями вариантов и поле ответа. Адрес и токен вынесены на экран подключения. Главное действие закреплено внизу. Компоновка освобождает место под текст, когда вариантов нет, и учитывает доступную высоту экрана и DPI.

Для проверки оформления общий рендерер умеет создавать изображение в памяти (`render_design_preview`); оно не содержит данных пользователя. Нативная отрисовка читает отдельный снимок состояния, сохраняя корректный вид во время модальных диалогов и повторных вызовов Win32.

## Ресурсы

- `assets/brand/tidemark.svg` — исходный знак, построен кодом.
- `assets/brand/tidemark.png` и `tidemark.ico` — растровая версия и Windows-иконка с размерами 16–256 px. Пересоздание: `python scripts/make-brand.py` (Pillow).
- `assets/wallpapers/coast.png` и `night.png` — изображения, созданные встроенным image_gen. Оригиналы включены в portable и установщик, приложение не меняет обои Windows.
- `*-preview.bmp` — уменьшенные кадрированные копии для встроенных Win32 bitmap-ресурсов.

### Prompts (built-in image_gen)

**Coast:** Create a photographic desktop wallpaper for Tidemark, a quiet Windows desktop island utility. Wide landscape 1536x1024. Real aerial coastal photography: a small low rocky island with pale dry grass on the lower right, vast calm muted blue grey sea, hazy warm ivory sky at dawn, subtle terracotta sunrise light on rock. Restrained editorial travel photograph, filmic natural light, beautiful real water details, no dramatic fantasy. The upper central half is calm low detail sky/water suitable for a black desktop status pill overlay. No text, no logos, no UI, no borders. This is an actual wallpaper asset, not a product mockup.

**Night:** Generate a second Tidemark desktop wallpaper, landscape 1536x1024. Photorealistic quiet coastal blue hour: one small low rocky island in lower right of wide tranquil sea, nightfall, desaturated slate ocean, very soft silver moonlight reflected on water, distant misty horizon, muted ink blue sky. Natural professional landscape photography, subdued and peaceful. Upper middle has open calm sky for overlaying a small desktop status pill. No objects other than the real coast, no stars fantasy, no nebula, no glow effects, no text, logos, borders or user interface. Companion to a warm dawn coastal wallpaper, same quiet editorial sensibility.

## Методика

Редизайн выполнен с использованием [unslop](https://github.com/heydamir/unslop) by [Damir Sokolovsky](https://x.com/heydamir), CC BY 4.0. Методика адаптирована к Rust/Win32. Переименование прямо запрошено пользователем. Просмотр исходного release-exe заблокировал Defender; обновлённая debug-сборка запустилась и проверена через Computer Use.
