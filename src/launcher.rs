//! Alt+Space search: apps from the Start menu, recent files and files in
//! Desktop/Documents/Downloads, plus a calculator and a web search. The
//! index is built on a background thread the first time the search opens
//! and refreshed at most every 10 minutes after that.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_INDEX: u32 = WM_APP + 11;
const REFRESH: Duration = Duration::from_secs(600);
const MAX_FILES: usize = 30_000;
const MAX_DEPTH: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    App,
    Recent,
    File,
    Folder,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    pub path: String,
    pub kind: Kind,
    lower: String,
}

impl Item {
    pub fn new(name: &str, path: &str, kind: Kind) -> Self {
        Self { name: name.to_string(), path: path.to_string(), kind, lower: name.to_lowercase() }
    }
}

const EN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.`";
const RU: &str = "йцукенгшщзхъфывапролджэячсмитьбюё";

/// Retypes text as if the other layout (RU <-> EN) had been on.
pub fn swap_layout(s: &str) -> String {
    let en: Vec<char> = EN.chars().collect();
    let ru: Vec<char> = RU.chars().collect();
    s.chars()
        .map(|c| {
            let lc = c.to_lowercase().next().unwrap_or(c);
            if let Some(i) = en.iter().position(|&e| e == lc) {
                ru[i]
            } else if let Some(i) = ru.iter().position(|&r| r == lc) {
                en[i]
            } else {
                lc
            }
        })
        .collect()
}

/// How well `name` (lowercase) matches `q` (lowercase); 0 = no match.
pub fn score(name: &str, q: &str) -> u32 {
    if q.is_empty() {
        return 0;
    }
    if name == q {
        return 1000;
    }
    if name.starts_with(q) {
        return 800 - name.chars().count().min(200) as u32;
    }
    // A word inside the name starts with the query: "chrome" in "google chrome".
    let word_start = name.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(q));
    if word_start {
        return 600 - name.chars().count().min(200) as u32;
    }
    if name.contains(q) {
        return 400 - name.chars().count().min(200) as u32;
    }
    // Initials: "vsc" -> "Visual Studio Code".
    let initials: String = name.split(|c: char| !c.is_alphanumeric()).filter_map(|w| w.chars().next()).collect();
    if q.chars().count() >= 2 && initials.starts_with(q) {
        return 300;
    }
    0
}

/// Best matches for `query` (also tried in the other keyboard layout).
pub fn search(items: &[Item], query: &str, limit: usize) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let alt = swap_layout(&q);
    let mut scored: Vec<(u32, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| {
            // The other-layout reading only counts for strong matches (name or
            // word starts with it): "jnx" inside a random file name is noise.
            let alt_score = score(&it.lower, &alt);
            let alt_score = if alt_score >= 500 { alt_score - 50 } else { 0 };
            let s = score(&it.lower, &q).max(alt_score);
            // Apps first, then recent files, then everything else.
            let bonus = match it.kind {
                Kind::App => 250,
                Kind::Recent => 80,
                Kind::Folder => 20,
                Kind::File => 0,
            };
            (s > 0).then_some((s + bonus, i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(items[a.1].name.len().cmp(&items[b.1].name.len())));
    let mut seen = std::collections::HashSet::new();
    scored
        .into_iter()
        .filter(|(_, i)| seen.insert(items[*i].name.to_lowercase() + "|" + &format!("{:?}", items[*i].kind == Kind::App)))
        .take(limit)
        .map(|(_, i)| i)
        .collect()
}

/// Files that run code when opened. Found in user folders (Downloads!),
/// they are revealed in Explorer instead of being started from the search.
pub fn is_executable(name: &str) -> bool {
    const EXT: [&str; 20] = [
        "exe", "com", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "hta", "msi", "msp", "scr", "pif", "lnk", "url", "cpl",
    ];
    name.rsplit_once('.').is_some_and(|(_, ext)| EXT.contains(&ext.to_ascii_lowercase().as_str()))
}

/// A tiny calculator: + - * / % ^ and parentheses, "," as decimal point.
pub fn calc(input: &str) -> Option<f64> {
    let s: Vec<char> = input.replace(',', ".").chars().filter(|c| !c.is_whitespace()).collect();
    // Must look like arithmetic: a digit and an operator.
    if !s.iter().any(|c| c.is_ascii_digit()) || !s.iter().any(|c| "+-*/%^(".contains(*c)) {
        return None;
    }
    let mut p = Parser { s, i: 0 };
    let v = p.expr()?;
    (p.i == p.s.len() && v.is_finite()).then_some(v)
}

struct Parser {
    s: Vec<char>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        while let Some(op @ ('+' | '-')) = self.peek() {
            self.i += 1;
            let r = self.term()?;
            v = if op == '+' { v + r } else { v - r };
        }
        Some(v)
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.power()?;
        while let Some(op @ ('*' | '/' | '%')) = self.peek() {
            self.i += 1;
            let r = self.power()?;
            v = match op {
                '*' => v * r,
                '/' => v / r,
                _ => v % r,
            };
        }
        Some(v)
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if self.peek() == Some('^') {
            self.i += 1;
            let exp = self.power()?;
            return Some(base.powf(exp));
        }
        Some(base)
    }

    fn unary(&mut self) -> Option<f64> {
        if self.peek() == Some('-') {
            self.i += 1;
            return Some(-self.unary()?);
        }
        self.atom()
    }

    fn atom(&mut self) -> Option<f64> {
        if self.peek() == Some('(') {
            self.i += 1;
            let v = self.expr()?;
            if self.peek() != Some(')') {
                return None;
            }
            self.i += 1;
            return Some(v);
        }
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
            self.i += 1;
        }
        self.s[start..self.i].iter().collect::<String>().parse().ok()
    }
}

/// "42", "3.5", "0.333333" — no trailing zeros, at most 8 decimals.
pub fn format_number(v: f64) -> String {
    let s = format!("{:.8}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

// ---- Indexing --------------------------------------------------------------

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from)
}

fn stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<Item>, f: &mut dyn FnMut(&Path, bool) -> Option<Item>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.starts_with('~') || name.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let is_dir = ft.is_dir();
        if let Some(item) = f(&path, is_dir) {
            out.push(item);
        }
        if is_dir && depth > 0 && !matches!(name.as_str(), "node_modules" | "target" | ".git") {
            walk(&path, depth - 1, out, f);
        }
    }
}

pub fn build_index() -> Vec<Item> {
    let mut items = Vec::new();
    // Start menu apps (.lnk), both for this user and all users.
    let start_dirs = [
        env_path("APPDATA").map(|p| p.join("Microsoft\\Windows\\Start Menu\\Programs")),
        env_path("ProgramData").map(|p| p.join("Microsoft\\Windows\\Start Menu\\Programs")),
    ];
    for dir in start_dirs.into_iter().flatten() {
        walk(&dir, 4, &mut items, &mut |p, is_dir| {
            let ext = p.extension()?.to_string_lossy().to_lowercase();
            if is_dir || !(ext == "lnk" || ext == "url") {
                return None;
            }
            let name = stem(p);
            let lower = name.to_lowercase();
            if lower.contains("uninstall") || lower.contains("удал") {
                return None;
            }
            Some(Item::new(&name, &p.to_string_lossy(), Kind::App))
        });
    }
    // Recent files (shortcuts named after the file).
    if let Some(recent) = env_path("APPDATA").map(|p| p.join("Microsoft\\Windows\\Recent")) {
        walk(&recent, 0, &mut items, &mut |p, is_dir| {
            (!is_dir && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")))
                .then(|| Item::new(&stem(p), &p.to_string_lossy(), Kind::Recent))
        });
    }
    // Files in the usual places.
    if let Some(home) = env_path("USERPROFILE") {
        for sub in ["Desktop", "Documents", "Downloads"] {
            walk(&home.join(sub), MAX_DEPTH, &mut items, &mut |p, is_dir| {
                let name = p.file_name()?.to_string_lossy().into_owned();
                Some(Item::new(&name, &p.to_string_lossy(), if is_dir { Kind::Folder } else { Kind::File }))
            });
        }
    }
    items
}

pub struct Index {
    pub items: Arc<Mutex<Arc<Vec<Item>>>>,
    built: Arc<Mutex<Option<Instant>>>,
    building: Arc<Mutex<bool>>,
}

impl Index {
    pub fn new() -> Self {
        Self { items: Arc::new(Mutex::new(Arc::new(Vec::new()))), built: Arc::new(Mutex::new(None)), building: Arc::new(Mutex::new(false)) }
    }

    /// Rebuilds in the background if never built or older than 10 minutes.
    pub fn refresh(&self, hwnd: HWND) {
        let stale = self.built.lock().unwrap().is_none_or(|t| t.elapsed() > REFRESH);
        let mut building = self.building.lock().unwrap();
        if !stale || *building {
            return;
        }
        *building = true;
        let (items, built, flag) = (self.items.clone(), self.built.clone(), self.building.clone());
        let hwnd_raw = hwnd.0 as isize;
        let _ = std::thread::Builder::new().name("index".into()).spawn(move || {
            let fresh = Arc::new(build_index());
            *items.lock().unwrap() = fresh;
            *built.lock().unwrap() = Some(Instant::now());
            *flag.lock().unwrap() = false;
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd_raw as *mut _)), WM_INDEX, WPARAM(0), LPARAM(0));
            }
        });
    }

    pub fn snapshot(&self) -> Arc<Vec<Item>> {
        self.items.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_swap() {
        assert_eq!(swap_layout("ghbdtn"), "привет");
        assert_eq!(swap_layout("руддщ"), "hello");
        assert_eq!(swap_layout("Ntktuhfv"), "телеграм");
        assert_eq!(swap_layout("123"), "123");
    }

    #[test]
    fn scoring_order() {
        let exact = score("telegram", "telegram");
        let prefix = score("telegram desktop", "tele");
        let word = score("google chrome", "chrome");
        let inner = score("photoshop", "shop");
        let initials = score("visual studio code", "vsc");
        assert!(exact > prefix && prefix > word && word > inner && inner > initials && initials > 0);
        assert_eq!(score("notepad", "xyz"), 0);
    }

    #[test]
    fn search_prefers_apps_and_handles_wrong_layout() {
        let items = vec![
            Item::new("Телеграм отчёт.docx", "C:\\d\\t.docx", Kind::File),
            Item::new("Telegram", "C:\\s\\Telegram.lnk", Kind::App),
            Item::new("Отчёт за квартал.docx", "C:\\d\\o.docx", Kind::File),
        ];
        assert_eq!(search(&items, "tele", 5), vec![1]);
        // "jnx" typed on the EN layout means "отч".
        assert_eq!(search(&items, "jnx", 5), vec![2, 0]);
        assert!(search(&items, "   ", 5).is_empty());
        // An installer named after the app must not outrank the app.
        let items = vec![
            Item::new("ChromeSetup.exe", "C:/d/ChromeSetup.exe", Kind::File),
            Item::new("Google Chrome", "C:/s/Google Chrome.lnk", Kind::App),
            Item::new("mr-BP75JNXo.js", "C:/d/mr-BP75JNXo.js", Kind::File),
        ];
        assert_eq!(search(&items, "chrome", 5)[0], 1);
        // Wrong-layout text buried inside a word is not a match.
        assert!(search(&items, "отч", 5).is_empty());
    }

    #[test]
    fn executables_are_recognized() {
        for n in ["Telegram.exe", "setup.MSI", "run.bat", "x.ps1", "a.js", "Ярлык.lnk"] {
            assert!(is_executable(n), "{n}");
        }
        for n in ["Отчёт.docx", "photo.png", "archive.zip", "noext", "notes.txt"] {
            assert!(!is_executable(n), "{n}");
        }
    }

    #[test]
    fn calculator() {
        assert_eq!(calc("2+2*3"), Some(8.0));
        assert_eq!(calc("(1,5 + 0,5) ^ 3"), Some(8.0));
        assert_eq!(calc("10 / 4"), Some(2.5));
        assert_eq!(calc("-3 * -2"), Some(6.0));
        assert_eq!(calc("7 % 3"), Some(1.0));
        assert_eq!(calc("2+"), None);
        assert_eq!(calc("telegram"), None);
        assert_eq!(calc("42"), None); // no operator: not a calculation
        assert_eq!(calc("1/0"), None);
        assert_eq!(format_number(8.0), "8");
        assert_eq!(format_number(1.0 / 3.0), "0.33333333");
        assert_eq!(format_number(-0.0), "0");
    }
}

/// Live: `cargo test --release launcher::live -- --ignored --nocapture`.
#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore]
    fn index_this_pc() {
        let t = Instant::now();
        let items = build_index();
        let built = t.elapsed();
        let apps = items.iter().filter(|i| i.kind == Kind::App).count();
        let recent = items.iter().filter(|i| i.kind == Kind::Recent).count();
        println!("index: {} items ({apps} apps, {recent} recent) in {built:?}", items.len());
        for q in ["tele", "ntkt", "chrome", "отч", "zip"] {
            let t = Instant::now();
            let hits = search(&items, q, 5);
            let names: Vec<&str> = hits.iter().map(|&i| items[i].name.as_str()).collect();
            println!("{q:>8}: {:?} -> {} hits {:?}", t.elapsed(), names.len(), names);
        }
    }
}
