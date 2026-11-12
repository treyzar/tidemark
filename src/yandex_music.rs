//! Yandex Music actions through the player's Windows accessibility interface.
//! No credentials or simulated mouse/keyboard input are involved.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, IsIconic, IsWindow, PostMessageW, SetForegroundWindow, ShowWindowAsync,
    SW_RESTORE, SW_SHOWMINNOACTIVE, WM_APP,
};

pub const WM_YANDEX_LIKE: u32 = WM_APP + 22;
pub const WM_YANDEX_OPEN: u32 = WM_APP + 23;
pub struct Outcome {
    pub title: String,
    pub artist: String,
    pub result: Result<bool, String>,
}
pub struct OpenOutcome {
    pub title: String,
    pub artist: String,
    pub result: Result<(), String>,
}

#[derive(Clone, Copy)]
enum Action {
    Like(bool),
    Open,
}
impl Action {
    fn opens(self) -> bool {
        matches!(self, Self::Open)
    }
}
enum ActionResult {
    Liked(bool),
    Opened,
}

static BUSY: AtomicBool = AtomicBool::new(false);
struct Busy;
impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}
struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

/// Starts one asynchronous operation. False means busy or thread creation failed.
/// The result contains the verified service state and is posted to `hwnd`.
pub fn spawn(
    hwnd: HWND,
    title: String,
    artist: String,
    want: bool,
    slot: Arc<Mutex<Option<Outcome>>>,
) -> bool {
    if BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    let raw = hwnd.0 as isize;
    let started = std::thread::Builder::new()
        .name("yandex-like".into())
        .spawn(move || {
            let _busy = Busy;
            let result = unsafe { change_like(&title, &artist, want) };
            let outcome = Outcome {
                title,
                artist,
                result,
            };
            // A poisoned UI slot must not strand the completion notification.
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(raw as *mut _)),
                    WM_YANDEX_LIKE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        });
    if started.is_err() {
        BUSY.store(false, Ordering::Release);
        return false;
    }
    true
}

/// Opens the existing player for this exact track. No tabs are created and no
/// playback or favorite state is changed. Shares the like worker's busy gate.
pub fn spawn_open(
    hwnd: HWND,
    title: String,
    artist: String,
    slot: Arc<Mutex<Option<OpenOutcome>>>,
) -> bool {
    if BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    let raw = hwnd.0 as isize;
    let started = std::thread::Builder::new()
        .name("yandex-open".into())
        .spawn(move || {
            let _busy = Busy;
            let result = unsafe { run_action(&title, &artist, Action::Open) }.map(|_| ());
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(OpenOutcome {
                title,
                artist,
                result,
            });
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(raw as *mut _)),
                    WM_YANDEX_OPEN,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        });
    if started.is_err() {
        BUSY.store(false, Ordering::Release);
        return false;
    }
    true
}

fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn artist_name(text: &str) -> String {
    normalized(text.trim_end().trim_end_matches(['—', '–', '-']).trim_end())
}
fn metadata_matches(names: &[String], title: &str, artist: &str) -> bool {
    let title = normalized(title);
    let artist = artist_name(artist);
    !title.is_empty()
        && !artist.is_empty()
        && names.iter().any(|s| normalized(s) == title)
        && names.iter().any(|s| artist_name(s) == artist)
}
fn is_like(name: &str) -> bool {
    name == "Нравится"
}
fn service_name(name: &str) -> bool {
    // A site mention in an arbitrary tab title is insufficient: the service's
    // own branded title must be the complete title or its leading component.
    let name = normalized(name);
    ["яндекс музыка", "yandex music", "yandex.music"]
        .iter()
        .any(|brand| {
            name == *brand
                || name
                    .strip_prefix(brand)
                    .is_some_and(|s| s.starts_with(" — ") || s.starts_with(" - "))
        })
}
fn trusted_music_url(url: &str) -> bool {
    // Conservative parsing: do not accept browser-normalized backslashes,
    // whitespace, userinfo, ports, percent-encoded hosts, or suffix matches.
    if url
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        return false;
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    [
        "music.yandex.ru",
        "music.yandex.com",
        "music.yandex.by",
        "music.yandex.kz",
    ]
    .iter()
    .any(|host| authority.eq_ignore_ascii_case(host))
}

unsafe fn element_value(element: &IUIAutomationElement) -> Option<String> {
    if let Ok(pattern) =
        element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
    {
        if let Ok(value) = pattern.CurrentValue() {
            let value = value.to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    let pattern = element
        .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
            UIA_LegacyIAccessiblePatternId,
        )
        .ok()?;
    let value = pattern.CurrentValue().ok()?.to_string();
    (!value.is_empty()).then_some(value)
}

unsafe fn has_document_ancestor(
    uia: &IUIAutomation,
    element: &IUIAutomationElement,
    window: &IUIAutomationElement,
) -> Result<bool, String> {
    let walker = uia
        .RawViewWalker()
        .map_err(|e| fail("Не удалось проверить источник страницы", e))?;
    let mut current = element.clone();
    for _ in 0..64 {
        if uia
            .CompareElements(&current, window)
            .map_err(|e| fail("Не удалось проверить окно браузера", e))?
            .as_bool()
        {
            return Ok(false);
        }
        current = walker
            .GetParentElement(&current)
            .map_err(|e| fail("Не удалось проверить источник страницы", e))?;
        if current
            .CurrentControlType()
            .map_err(|e| fail("Источник страницы недоступен", e))?
            == UIA_DocumentControlTypeId
        {
            return Ok(true);
        }
    }
    Err("Не удалось определить источник страницы Яндекс Музыки".into())
}

unsafe fn web_document(
    uia: &IUIAutomation,
    window: &IUIAutomationElement,
) -> Result<IUIAutomationElement, String> {
    let document_condition = by_type(uia, UIA_DocumentControlTypeId)?;
    let mut documents = Vec::new();
    for document in elements(window, TreeScope_Descendants, &document_condition, 64)? {
        if document
            .CurrentIsOffscreen()
            .map_err(|e| fail("Недоступно состояние страницы", e))?
            .as_bool()
        {
            continue;
        }
        if !has_document_ancestor(uia, &document, window)? {
            documents.push(document);
        }
    }
    if documents.len() != 1 {
        return Err("Не удалось однозначно определить открытую страницу Яндекс Музыки".into());
    }
    let document = documents.remove(0);
    if let Some(url) = element_value(&document) {
        return if trusted_music_url(&url) {
            Ok(document)
        } else {
            Err("Адрес страницы не соответствует Яндекс Музыке; операция отменена".into())
        };
    }
    // Chromium may omit Document.Value. Its native address field is outside
    // every web document; page-authored inputs must never verify the origin.
    let address_condition = uia
        .CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from("view_1012"))
        .map_err(|e| fail("Не удалось проверить адрес Яндекс Музыки", e))?;
    let mut addresses = Vec::new();
    for address in elements(window, TreeScope_Descendants, &address_condition, 8)? {
        if address.CurrentControlType().ok() != Some(UIA_EditControlTypeId)
            || !address
                .CurrentIsOffscreen()
                .is_ok_and(|offscreen| !offscreen.as_bool())
            || has_document_ancestor(uia, &address, window)?
        {
            continue;
        }
        addresses.push(address);
    }
    if addresses.len() == 1
        && element_value(&addresses[0]).is_some_and(|url| trusted_music_url(&url))
    {
        return Ok(document);
    }
    Err("Не удалось подтвердить адрес Яндекс Музыки. Откройте music.yandex.ru и повторите".into())
}

unsafe fn confirmed_player(
    uia: &IUIAutomation,
    window: &IUIAutomationElement,
    web: bool,
    title: &str,
    artist: &str,
) -> Result<IUIAutomationElement, String> {
    // Known native executable windows have no web origin. Browser and PWA
    // windows must supply a verified service URL and a player in that document.
    if web {
        player(uia, &web_document(uia, window)?, title, artist)
    } else {
        player(uia, window, title, artist)
    }
}
fn fail(context: &str, err: windows::core::Error) -> String {
    format!("{context}: {err}")
}

unsafe fn elements(
    root: &IUIAutomationElement,
    scope: TreeScope,
    condition: &IUIAutomationCondition,
    cap: i32,
) -> Result<Vec<IUIAutomationElement>, String> {
    let list = root
        .FindAll(scope, condition)
        .map_err(|e| fail("Не удалось прочитать интерфейс Яндекс Музыки", e))?;
    let count = list
        .Length()
        .map_err(|e| fail("Недоступен список элементов", e))?;
    if count > cap {
        return Err("Слишком много элементов интерфейса; операция отменена".into());
    }
    (0..count)
        .map(|i| {
            list.GetElement(i)
                .map_err(|e| fail("Элемент интерфейса исчез", e))
        })
        .collect()
}
unsafe fn by_type(
    uia: &IUIAutomation,
    kind: UIA_CONTROLTYPE_ID,
) -> Result<IUIAutomationCondition, String> {
    uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(kind.0))
        .map_err(|e| fail("Не удалось настроить поиск интерфейса", e))
}

unsafe fn executable(element: &IUIAutomationElement) -> Option<String> {
    let pid = element.CurrentProcessId().ok()? as u32;
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut buffer = [0u16; 1024];
    let mut size = buffer.len() as u32;
    let result = QueryFullProcessImageNameW(
        process,
        PROCESS_NAME_WIN32,
        PWSTR(buffer.as_mut_ptr()),
        &mut size,
    );
    let _ = CloseHandle(process);
    result.ok()?;
    let path = String::from_utf16_lossy(&buffer[..size as usize]);
    Some(path.rsplit(['\\', '/']).next()?.to_lowercase())
}

struct RestoreTab {
    original: IUIAutomationSelectionItemPattern,
    target: IUIAutomationSelectionItemPattern,
    armed: bool,
}
impl Drop for RestoreTab {
    fn drop(&mut self) {
        unsafe {
            // If the user changed tabs meanwhile, leave their selection alone.
            if self.armed && self.target.CurrentIsSelected().is_ok_and(|v| v.as_bool()) {
                let _ = self.original.Select();
            }
        }
    }
}
unsafe fn selection(
    tab: &IUIAutomationElement,
) -> Result<IUIAutomationSelectionItemPattern, String> {
    tab.GetCurrentPatternAs(UIA_SelectionItemPatternId)
        .map_err(|e| fail("Браузер не разрешает переключить вкладку Яндекс Музыки", e))
}
unsafe fn select_tab(
    tabs: &[IUIAutomationElement],
    target: &IUIAutomationElement,
) -> Result<Option<RestoreTab>, String> {
    let target_pattern = selection(target)?;
    if target_pattern
        .CurrentIsSelected()
        .map_err(|e| fail("Недоступно состояние вкладки", e))?
        .as_bool()
    {
        return Ok(None);
    }
    let mut original = None;
    for tab in tabs {
        if let Ok(pattern) = selection(tab) {
            if pattern.CurrentIsSelected().is_ok_and(|v| v.as_bool()) {
                original = Some(pattern);
                break;
            }
        }
    }
    let original = original.ok_or("Не удалось запомнить текущую вкладку; операция отменена")?;
    let restore = RestoreTab {
        original,
        target: target_pattern,
        armed: true,
    };
    restore
        .target
        .Select()
        .map_err(|e| fail("Не удалось открыть вкладку Яндекс Музыки", e))?;
    Ok(Some(restore))
}

unsafe fn selected(tab: Option<&IUIAutomationElement>) -> Result<(), String> {
    if let Some(tab) = tab {
        if !selection(tab)?
            .CurrentIsSelected()
            .map_err(|e| fail("Недоступно состояние вкладки", e))?
            .as_bool()
        {
            return Err("Вкладка изменилась; операция отменена".into());
        }
    }
    Ok(())
}

unsafe fn player(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    title: &str,
    artist: &str,
) -> Result<IUIAutomationElement, String> {
    // Scope to the exact footer region. Other cards on the page also have likes.
    let condition = uia
        .CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from("Плеер"))
        .map_err(|e| fail("Не удалось найти плеер", e))?;
    let candidates = elements(root, TreeScope_Descendants, &condition, 16)?;
    let text_condition = by_type(uia, UIA_TextControlTypeId)?;
    let mut matching = Vec::new();
    for candidate in candidates {
        if candidate.CurrentControlType().ok() != Some(UIA_GroupControlTypeId) {
            continue;
        }
        let texts = elements(&candidate, TreeScope_Descendants, &text_condition, 256)?;
        let names: Vec<_> = texts
            .iter()
            .filter_map(|t| t.CurrentName().ok().map(|n| n.to_string()))
            .collect();
        if metadata_matches(&names, title, artist) {
            matching.push(candidate);
        }
    }
    if matching.len() != 1 {
        return Err(
            "В плеере Яндекс Музыки не подтверждён этот трек. Откройте его вкладку и повторите"
                .into(),
        );
    }
    Ok(matching.remove(0))
}

unsafe fn like_button(
    uia: &IUIAutomation,
    player: &IUIAutomationElement,
) -> Result<IUIAutomationTogglePattern, String> {
    let condition = by_type(uia, UIA_ButtonControlTypeId)?;
    let buttons = elements(player, TreeScope_Descendants, &condition, 80)?;
    let matching: Vec<_> = buttons
        .into_iter()
        .filter(|b| b.CurrentName().is_ok_and(|n| is_like(&n.to_string())))
        .collect();
    if matching.len() != 1 {
        return Err("В плеере не найдена однозначная кнопка «Нравится»".into());
    }
    if !matching[0].CurrentIsEnabled().is_ok_and(|v| v.as_bool()) {
        return Err("Кнопка «Нравится» сейчас недоступна".into());
    }
    matching[0]
        .GetCurrentPatternAs(UIA_TogglePatternId)
        .map_err(|e| fail("Яндекс Музыка не предоставляет состояние лайка", e))
}
unsafe fn state(pattern: &IUIAutomationTogglePattern) -> Result<bool, String> {
    let value = pattern
        .CurrentToggleState()
        .map_err(|e| fail("Не удалось проверить лайк в Яндекс Музыке", e))?;
    if value == ToggleState_On {
        Ok(true)
    } else if value == ToggleState_Off {
        Ok(false)
    } else {
        Err("Яндекс Музыка вернула неопределённое состояние лайка".into())
    }
}

unsafe fn apply(
    uia: &IUIAutomation,
    window: &IUIAutomationElement,
    tab: Option<&IUIAutomationElement>,
    web: bool,
    title: &str,
    artist: &str,
    want: bool,
) -> Result<bool, String> {
    selected(tab)?;
    let current_player = confirmed_player(uia, window, web, title, artist)?;
    let button = like_button(uia, &current_player)?;
    let current = state(&button)?;
    // Re-read track and selection immediately before the only mutating call.
    selected(tab)?;
    let current_player = confirmed_player(uia, window, web, title, artist)?;
    let button = like_button(uia, &current_player)?;
    let current_again = state(&button)?;
    if current != current_again {
        return Err("Состояние лайка изменилось; повторите попытку".into());
    }
    if current != want {
        button
            .Toggle()
            .map_err(|e| fail("Не удалось изменить лайк в Яндекс Музыке", e))?;
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        selected(tab)?;
        let current_player = confirmed_player(uia, window, web, title, artist)?;
        let actual = state(&like_button(uia, &current_player)?)?;
        if actual == want {
            selected(tab)?;
            confirmed_player(uia, window, web, title, artist)?;
            return Ok(actual);
        }
        if Instant::now() >= deadline {
            return Err("Яндекс Музыка не подтвердила изменение лайка".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct RestoreMinimized {
    hwnd: HWND,
    armed: bool,
}
impl Drop for RestoreMinimized {
    fn drop(&mut self) {
        if self.armed {
            unsafe {
                let _ = ShowWindowAsync(self.hwnd, SW_SHOWMINNOACTIVE);
            }
        }
    }
}

struct Candidate {
    window: IUIAutomationElement,
    tab: Option<IUIAutomationElement>,
    web: bool,
    // Drop selection before re-minimizing a restored browser window.
    tab_restore: Option<RestoreTab>,
    minimized_restore: Option<RestoreMinimized>,
}
impl Candidate {
    fn keep_open(&mut self) {
        if let Some(restore) = self.tab_restore.as_mut() {
            restore.armed = false;
        }
        if let Some(restore) = self.minimized_restore.as_mut() {
            restore.armed = false;
        }
    }
}

unsafe fn native_handle(window: &IUIAutomationElement) -> Result<HWND, String> {
    let hwnd = window
        .CurrentNativeWindowHandle()
        .map_err(|e| fail("Не удалось определить окно Яндекс Музыки", e))?;
    if hwnd.is_invalid() || !IsWindow(Some(hwnd)).as_bool() {
        return Err("Окно Яндекс Музыки уже закрыто".into());
    }
    Ok(hwnd)
}

unsafe fn prepare_open(
    window: &IUIAutomationElement,
    action: Action,
) -> Result<Option<RestoreMinimized>, String> {
    if !action.opens() {
        return Ok(None);
    }
    let hwnd = native_handle(window)?;
    if !IsIconic(hwnd).as_bool() {
        return Ok(None);
    }
    let restore = RestoreMinimized { hwnd, armed: true };
    if !ShowWindowAsync(hwnd, SW_RESTORE).as_bool() {
        return Err("Не удалось восстановить окно Яндекс Музыки".into());
    }
    let deadline = Instant::now() + Duration::from_millis(900);
    while IsIconic(hwnd).as_bool() {
        if Instant::now() >= deadline {
            return Err("Окно Яндекс Музыки не восстановилось".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(Some(restore))
}

unsafe fn open_player(
    uia: &IUIAutomation,
    candidate: &Candidate,
    title: &str,
    artist: &str,
) -> Result<(), String> {
    selected(candidate.tab.as_ref())?;
    confirmed_player(uia, &candidate.window, candidate.web, title, artist)?;
    let hwnd = native_handle(&candidate.window)?;
    if !SetForegroundWindow(hwnd).as_bool() {
        return Err("Windows не разрешила открыть окно Яндекс Музыки".into());
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    while GetForegroundWindow() != hwnd {
        if Instant::now() >= deadline {
            return Err("Windows не подтвердила открытие Яндекс Музыки".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    selected(candidate.tab.as_ref())?;
    confirmed_player(uia, &candidate.window, candidate.web, title, artist)?;
    Ok(())
}

unsafe fn change_like(title: &str, artist: &str, want: bool) -> Result<bool, String> {
    match run_action(title, artist, Action::Like(want))? {
        ActionResult::Liked(value) => Ok(value),
        ActionResult::Opened => Err("Неожиданный результат действия Яндекс Музыки".into()),
    }
}

unsafe fn run_action(title: &str, artist: &str, action: Action) -> Result<ActionResult, String> {
    if normalized(title).is_empty() || artist_name(artist).is_empty() {
        return Err("Неизвестны название или исполнитель трека".into());
    }
    CoInitializeEx(None, COINIT_MULTITHREADED)
        .ok()
        .map_err(|e| fail("Не удалось запустить Windows UI Automation", e))?;
    let _apartment = Apartment;
    // CUIAutomation8 is the class that exposes IUIAutomation2 and provider
    // timeouts. Do not rely on the legacy class supporting QueryInterface.
    let uia: IUIAutomation2 = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
        .map_err(|e| fail("Недоступен Windows UI Automation", e))?;
    uia.SetAutoSetFocus(false)
        .map_err(|e| fail("Не удалось отключить автоматическую смену фокуса", e))?;
    uia.SetConnectionTimeout(700)
        .map_err(|e| fail("Не удалось ограничить время подключения", e))?;
    uia.SetTransactionTimeout(1000)
        .map_err(|e| fail("Не удалось ограничить время операции", e))?;
    let mut candidate = find_player(&uia, title, artist, action)?;
    match action {
        Action::Like(want) => apply(
            &uia,
            &candidate.window,
            candidate.tab.as_ref(),
            candidate.web,
            title,
            artist,
            want,
        )
        .map(ActionResult::Liked),
        Action::Open => {
            open_player(&uia, &candidate, title, artist)?;
            // Keep all COM references normally owned/dropped. Only disarm the
            // restoration policy once the exact requested player is foreground.
            candidate.keep_open();
            Ok(ActionResult::Opened)
        }
    }
}

unsafe fn find_player(
    uia: &IUIAutomation,
    title: &str,
    artist: &str,
    action: Action,
) -> Result<Candidate, String> {
    let root = uia
        .GetRootElement()
        .map_err(|e| fail("Недоступен рабочий стол Windows", e))?;
    let all = uia
        .CreateTrueCondition()
        .map_err(|e| fail("Недоступен поиск окон", e))?;
    let windows = elements(&root, TreeScope_Children, &all, 192)?;
    let tabs_condition = by_type(&uia, UIA_TabItemControlTypeId)?;
    let mut last_error = "Откройте Яндекс Музыку в браузере или приложении и повторите".to_string();
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut attempted = 0;
    for window in windows {
        if Instant::now() >= deadline || attempted >= 6 {
            break;
        }
        let Some(exe) = executable(&window) else {
            continue;
        };
        let browser = matches!(
            exe.as_str(),
            "chrome.exe"
                | "msedge.exe"
                | "browser.exe"
                | "firefox.exe"
                | "brave.exe"
                | "opera.exe"
                | "vivaldi.exe"
        );
        let standalone = matches!(
            exe.as_str(),
            "yandexmusic.exe" | "yandex music.exe" | "яндекс музыка.exe"
        );
        if !browser && !standalone {
            continue;
        }
        if standalone {
            attempted += 1;
            // Do not attempt another target once the player has been verified:
            // a timeout after Toggle must never cause a second mutation.
            if player(&uia, &window, title, artist).is_ok() {
                let minimized_restore = prepare_open(&window, action)?;
                return Ok(Candidate {
                    window,
                    tab: None,
                    web: false,
                    tab_restore: None,
                    minimized_restore,
                });
            }
            continue;
        }
        let tabs = match elements(&window, TreeScope_Descendants, &tabs_condition, 128) {
            Ok(t) => t,
            Err(e) => {
                last_error = e;
                continue;
            }
        };
        // An installed browser app/PWA has no browser tab strip. Require its
        // branded window title as well as matching player metadata.
        if tabs.is_empty()
            && window
                .CurrentName()
                .is_ok_and(|n| service_name(&n.to_string()))
        {
            attempted += 1;
            let minimized_restore = prepare_open(&window, action)?;
            if confirmed_player(&uia, &window, true, title, artist).is_ok() {
                return Ok(Candidate {
                    window,
                    tab: None,
                    web: true,
                    tab_restore: None,
                    minimized_restore,
                });
            }
        }
        for tab in &tabs {
            if Instant::now() >= deadline || attempted >= 6 {
                break;
            }
            if !tab
                .CurrentName()
                .is_ok_and(|n| service_name(&n.to_string()))
            {
                continue;
            }
            attempted += 1;
            let minimized_restore = match prepare_open(&window, action) {
                Ok(restore) => restore,
                Err(e) => {
                    last_error = e;
                    continue;
                }
            };
            let tab_restore = match select_tab(&tabs, tab) {
                Ok(r) => r,
                Err(e) => {
                    last_error = e;
                    continue;
                }
            };
            // Chrome may publish its newly selected document asynchronously.
            let ready = Instant::now() + Duration::from_millis(900);
            loop {
                if let Err(e) = selected(Some(tab)) {
                    return Err(e);
                }
                match confirmed_player(&uia, &window, true, title, artist) {
                    Ok(_) => {
                        return Ok(Candidate {
                            window: window.clone(),
                            tab: Some(tab.clone()),
                            web: true,
                            tab_restore,
                            minimized_restore,
                        })
                    }
                    Err(e) => last_error = e,
                }
                if Instant::now() >= ready {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_like_never_selects_dislike_or_cards() {
        assert!(is_like("Нравится"));
        for label in [
            "Не нравится",
            "Больше не нравится",
            "Нравится Sample Song",
            " нравится",
        ] {
            assert!(!is_like(label));
        }
    }
    #[test]
    fn metadata_requires_both_exact_fields() {
        let names = vec!["Sample Artist —".into(), "Sample Song".into()];
        assert!(metadata_matches(&names, "  SAMPLE SONG ", "sample  artist"));
        assert!(!metadata_matches(
            &names,
            "Sample Song remix",
            "Sample Artist"
        ));
        assert!(!metadata_matches(&names, "Sample Song", "Other artist"));
        assert!(!metadata_matches(&names, "Sample Song", ""));
    }
    #[test]
    fn metadata_normalizes_unicode_spaces_and_artist_separator() {
        assert!(metadata_matches(
            &vec!["Трек".into(), "ИМЯ\u{a0}АРТИСТА – ".into()],
            "трек",
            "Имя артиста"
        ));
    }
    #[test]
    fn candidates_require_branded_service_title() {
        assert!(service_name(
            "Яндекс Музыка — собираем музыку и подкасты для вас"
        ));
        assert!(service_name("Yandex Music"));
        assert!(!service_name("Как открыть Яндекс Музыка — поиск"));
        assert!(!service_name("Яндекс Музыка отзывы"));
    }
    #[test]
    fn origin_requires_exact_https_music_authority() {
        for url in [
            "https://music.yandex.ru",
            "HTTPS://MUSIC.YANDEX.RU/album/123?track=456#player",
            "https://music.yandex.com/",
            "https://music.yandex.by/",
            "https://music.yandex.kz/",
        ] {
            assert!(trusted_music_url(url), "{url}");
        }
        for url in [
            "http://music.yandex.ru",
            "https://music.yandex.ru.evil.example/",
            "https://evil.example/?url=https://music.yandex.ru",
            "https://evil.example/#https://music.yandex.ru",
            "https://music.yandex.ru@evil.example/",
            "https://user@music.yandex.ru/",
            "https://music.yandex.ru:443/",
            "https://music.yandex.ru./",
            "https://music.yandex.ru\\@evil.example/",
            "https://music.yandex.ru%2eevil.example/",
            "https://musіc.yandex.ru/",
            " https://music.yandex.ru",
            "https://music.yandex.ru\n",
            "https://music.yandex.ru\t.evil.example/",
            "music.yandex.ru",
            "javascript:https://music.yandex.ru",
            "https:///music.yandex.ru",
        ] {
            assert!(!trusted_music_url(url), "{url}");
        }
    }
}
