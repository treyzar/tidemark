//! `island-hook.exe`: runs from Claude Code hooks (and the opencode
//! plugin) and shows the event on Win Island.
//!
//! Claude Code: reads the hook's JSON on stdin, prints the hook answer on
//! stdout. Handles PermissionRequest (Разрешить / Нет / answer in the
//! terminal), PreToolUse for AskUserQuestion (one single-choice question),
//! Notification and Stop. If the island is not running, the card is
//! dismissed or nobody answers within `--wait` seconds, it prints nothing
//! and Claude Code asks in the terminal as usual.
//!
//! `--raw`: stdin is already an island message ({"kind","title",...});
//! prints {"choice":n|null}. Used by the opencode plugin.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;

/// SecurityIdentification << 16 (std adds SECURITY_SQOS_PRESENT).
const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;
use std::time::Duration;

use serde_json::{json, Value};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{EqualSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};

const DEFAULT_WAIT_S: u64 = 50;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let raw = args.iter().any(|a| a == "--raw");
    let wait = args
        .iter()
        .position(|a| a == "--wait")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_WAIT_S);
    let mut input = String::new();
    if std::io::stdin().take(4 << 20).read_to_string(&mut input).is_err() {
        return;
    }
    let Ok(event) = serde_json::from_str::<Value>(&input) else { return };
    let wait = Duration::from_secs(wait);
    if raw {
        let mut msg = event.clone();
        let (title, text) = (event["title"].as_str().unwrap_or_default(), event["text"].as_str().unwrap_or_default());
        msg["title"] = short(title, TITLE_MAX).into();
        msg["text"] = short(text, TEXT_MAX).into();
        // Same rule as for Claude: no ✓ unless the card shows it all.
        if !(fits_in(title, TITLE_MAX) && fits(text)) {
            if let Some(options) = msg["options"].as_array_mut() {
                options.retain(|o| o != "✓");
            }
        }
        let choice = if msg["kind"] == "ask" { ask(&msg, wait) } else { notify(&msg); None };
        println!("{}", json!({ "choice": choice }));
        return;
    }
    let Some(plan) = plan(&event) else { return };
    let out = match plan {
        Plan::Notice(msg) => {
            notify(&msg);
            None
        }
        Plan::Permission(msg) => {
            let choice = ask(&msg, wait);
            permission_answer(choice.and_then(|i| msg["options"].get(i)).and_then(Value::as_str))
        }
        Plan::Question { msg, questions, question, labels } => question_answer(ask(&msg, wait), &questions, &question, &labels),
    };
    if let Some(out) = out {
        let mut stdout = std::io::stdout();
        let _ = writeln!(stdout, "{out}");
        let _ = stdout.flush();
    }
}

// ---- What to show -------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Plan {
    Notice(Value),
    Permission(Value),
    Question { msg: Value, questions: Value, question: String, labels: Vec<String> },
}

const PERMISSION_OPTIONS: [&str; 3] = ["Терминал", "Нет", "✓"];
/// When the card cannot show everything that would be allowed.
const PERMISSION_OPTIONS_PARTIAL: [&str; 2] = ["Терминал", "Нет"];
/// The card's second line holds this many characters.
const TEXT_MAX: usize = 160;
const TITLE_MAX: usize = 80;

fn project(event: &Value) -> String {
    let cwd = event["cwd"].as_str().unwrap_or_default().trim_end_matches(['\\', '/']);
    cwd.rsplit(['\\', '/']).next().unwrap_or_default().to_string()
}

/// First line, at most `max` characters.
fn short(s: &str, max: usize) -> String {
    let line = s.trim().lines().next().unwrap_or_default().trim();
    if line.chars().count() > max {
        let cut: String = line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

/// A path relative to the project, or the full path outside it.
/// Paths with `..` or `.` parts are always shown in full.
fn shown_path(p: &str, cwd: &str) -> String {
    let path = p.replace('\\', "/");
    let base = cwd.replace('\\', "/");
    let base = base.trim_end_matches('/');
    let plain = !path.split('/').any(|part| part == ".." || part == ".");
    let inside = plain
        && !base.is_empty()
        && path.len() > base.len() + 1
        && path.is_char_boundary(base.len())
        && path[..base.len()].eq_ignore_ascii_case(base)
        && path[base.len()..].starts_with('/');
    if inside {
        path[base.len() + 1..].to_string()
    } else {
        p.to_string()
    }
}

/// "Выполнить команду?" + what exactly, and whether that is everything
/// the permission covers (false: arguments the card does not show).
fn describe_tool(tool: &str, input: &Value, cwd: &str) -> (String, String, bool) {
    let s = |k: &str| input[k].as_str().unwrap_or_default().to_string();
    match tool {
        "Bash" | "PowerShell" => ("Выполнить команду?".into(), s("command"), true),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
            let path = if s("file_path").is_empty() { s("notebook_path") } else { s("file_path") };
            ("Изменить файл?".into(), shown_path(&path, cwd), true)
        }
        "Read" => ("Прочитать файл?".into(), shown_path(&s("file_path"), cwd), true),
        "WebFetch" => ("Открыть страницу?".into(), s("url"), true),
        "WebSearch" => ("Искать в сети?".into(), s("query"), true),
        t if t.starts_with("mcp__") => {
            let mut parts = t.splitn(3, "__").skip(1);
            let server = parts.next().unwrap_or_default();
            let name = parts.next().unwrap_or_default();
            (format!("Инструмент {}?", short(name, 28)), server.to_string(), false)
        }
        t => (format!("Разрешить {}?", short(t, 28)), input.to_string(), false),
    }
}

/// Fits the card's second line in full: one line, not cut.
/// Shown whole: `short(text, max)` leaves it as is, and every character is
/// one that draws as what it is.
fn fits_in(text: &str, max: usize) -> bool {
    short(text, max) == text && text.chars().all(allowed_char)
}

fn fits(text: &str) -> bool {
    fits_in(text, TEXT_MAX)
}

/// Characters a card may show next to ✓: printable ASCII, Latin-1,
/// the Russian alphabet and common punctuation. An allowlist, so invisible,
/// bidi, combining and look-alike characters from elsewhere never pass.
fn allowed_char(c: char) -> bool {
    matches!(c as u32, 0x20..=0x7E | 0xA1..=0xAC | 0xAE..=0xFF | 0x401 | 0x410..=0x44F | 0x451 | 0x2010..=0x2027 | 0x2030..=0x205E | 0x20AC | 0x2116)
}

fn card(kind: &str, title: &str, text: &str, options: &[String]) -> Value {
    json!({ "kind": kind, "source": "Claude", "title": short(title, TITLE_MAX), "text": short(text, TEXT_MAX), "options": options })
}

fn plan(event: &Value) -> Option<Plan> {
    let project = project(event);
    let with_project = |s: &str| if project.is_empty() { s.to_string() } else if s.is_empty() { project.clone() } else { format!("{project}: {s}") };
    match event["hook_event_name"].as_str()? {
        "PermissionRequest" => {
            let tool = event["tool_name"].as_str()?;
            if tool == "AskUserQuestion" || tool == "ExitPlanMode" {
                return None; // Those are not yes/no questions.
            }
            let (title, detail, complete) = describe_tool(tool, &event["tool_input"], event["cwd"].as_str().unwrap_or_default());
            let text = with_project(&detail);
            // ✓ only when the card shows everything it would allow; a
            // multi-line or cut command can be refused or left to the terminal.
            let options: &[&str] = if complete && fits(&text) && fits_in(&title, TITLE_MAX) { &PERMISSION_OPTIONS } else { &PERMISSION_OPTIONS_PARTIAL };
            let options: Vec<String> = options.iter().map(|s| s.to_string()).collect();
            Some(Plan::Permission(card("ask", &title, &text, &options)))
        }
        "PreToolUse" if event["tool_name"] == "AskUserQuestion" => {
            let questions = event["tool_input"]["questions"].clone();
            let list = questions.as_array()?;
            let [q] = list.as_slice() else { return None };
            if q["multiSelect"].as_bool() == Some(true) {
                return None;
            }
            let question = q["question"].as_str()?.to_string();
            let labels: Vec<String> = q["options"].as_array()?.iter().filter_map(|o| o["label"].as_str().map(str::to_string)).collect();
            // Buttons must fit the pill: up to three short answers.
            if labels.is_empty() || labels.len() > 3 || labels.iter().map(|l| l.chars().count()).sum::<usize>() > 36 {
                return None;
            }
            let mut options = labels.clone();
            options.push("✕".into());
            let msg = card("ask", &question, &with_project(q["header"].as_str().unwrap_or_default()), &options);
            Some(Plan::Question { msg, questions, question, labels })
        }
        "Notification" => {
            let title = match event["notification_type"].as_str().unwrap_or_default() {
                "permission_prompt" => return None, // PermissionRequest shows it.
                "idle_prompt" => "Claude ждёт вас".to_string(),
                "elicitation_dialog" => "Claude задаёт вопрос".to_string(),
                _ => event["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("Claude Code").to_string(),
            };
            Some(Plan::Notice(card("notice", &title, &with_project(event["message"].as_str().unwrap_or_default()), &[])))
        }
        "Stop" => {
            let last = event["last_assistant_message"].as_str().unwrap_or_default();
            Some(Plan::Notice(card("notice", "Claude закончил", &with_project(&short(last, 120)), &[])))
        }
        _ => None,
    }
}

/// The pressed button's label → the hook's decision.
fn permission_answer(label: Option<&str>) -> Option<Value> {
    let decision = match label? {
        "✓" => json!({ "behavior": "allow" }),
        "Нет" => json!({ "behavior": "deny", "message": "Пользователь запретил это в Win Island." }),
        _ => return None, // "Терминал": ask there.
    };
    Some(json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision } }))
}

fn question_answer(choice: Option<usize>, questions: &Value, question: &str, labels: &[String]) -> Option<Value> {
    let label = labels.get(choice?)?;
    Some(json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": { "questions": questions, "answers": { question: label } }
        }
    }))
}

// ---- Talking to the island ---------------------------------------------

fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let user: String = user.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
    format!(r"\\.\pipe\win-island-agent-{user}")
}

fn token_user(process: HANDLE) -> Option<Vec<u8>> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let mut buf = vec![0u8; 256];
        let mut len = 0u32;
        let ok = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as *mut _), buf.len() as u32, &mut len).is_ok();
        let _ = CloseHandle(token);
        ok.then_some(buf)
    }
}

/// The pipe must belong to an island running as this same user.
fn same_user(pipe: &File) -> bool {
    unsafe {
        let mut pid = 0u32;
        if GetNamedPipeServerProcessId(HANDLE(pipe.as_raw_handle()), &mut pid).is_err() {
            return false;
        }
        let Ok(server) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let theirs = token_user(server);
        let _ = CloseHandle(server);
        let (Some(theirs), Some(ours)) = (theirs, token_user(GetCurrentProcess())) else { return false };
        let a = &*(theirs.as_ptr() as *const TOKEN_USER);
        let b = &*(ours.as_ptr() as *const TOKEN_USER);
        EqualSid(a.User.Sid, b.User.Sid).is_ok()
    }
}

fn connect() -> Option<File> {
    for _ in 0..10 {
        // Identification level only: the server cannot act as this process.
        match OpenOptions::new().read(true).write(true).security_qos_flags(SECURITY_IDENTIFICATION).open(pipe_name()) {
            Ok(f) => return same_user(&f).then_some(f),
            // All instances busy for a moment: the island makes another.
            Err(e) if e.raw_os_error() == Some(231) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return None, // Island not running.
        }
    }
    None
}

fn send(pipe: &mut File, msg: &Value) -> bool {
    pipe.write_all(format!("{msg}\n").as_bytes()).is_ok() && pipe.flush().is_ok()
}

fn notify(msg: &Value) {
    if let Some(mut pipe) = connect() {
        send(&mut pipe, msg);
    }
}

/// Shows a card and waits for the button; None if nobody answers in time.
fn ask(msg: &Value, wait: Duration) -> Option<usize> {
    let mut pipe = connect()?;
    if !send(&mut pipe, msg) {
        return None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.take(4096).read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    // Leaving early closes the pipe and the island drops the card.
    let buf = rx.recv_timeout(wait).ok()?;
    let reply: Value = serde_json::from_slice(buf.split(|&b| b == b'\n').next()?).ok()?;
    reply["choice"].as_u64().map(|c| c as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_card() {
        let e = json!({"hook_event_name":"PermissionRequest","cwd":"C:\\Users\\ivan\\Documents\\win-island","tool_name":"Bash","tool_input":{"command":"cargo build --release"}});
        let Some(Plan::Permission(msg)) = plan(&e) else { panic!() };
        assert_eq!(msg["title"], "Выполнить команду?");
        assert_eq!(msg["text"], "win-island: cargo build --release");
        assert_eq!(msg["options"].as_array().unwrap().len(), 3);
        assert_eq!(permission_answer(Some("✓")).unwrap()["hookSpecificOutput"]["decision"]["behavior"], "allow");
        assert_eq!(permission_answer(Some("Нет")).unwrap()["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert!(permission_answer(Some("Терминал")).is_none());
        assert!(permission_answer(None).is_none());
    }

    #[test]
    fn paths() {
        assert_eq!(shown_path("C:\\P\\Demo\\src\\a.rs", "c:/p/demo"), "src/a.rs");
        assert_eq!(shown_path("C:/p/demo/../secret.txt", "C:/p/demo"), "C:/p/demo/../secret.txt");
        assert_eq!(shown_path("C:/p/demox/a.rs", "C:/p/demo"), "C:/p/demox/a.rs");
        // Non-ASCII case differences are not folded (and never mis-slice).
        assert_eq!(shown_path("C:/Проект/a.rs", "C:/проект"), "C:/Проект/a.rs");
        assert_eq!(shown_path("C:/проект/a.rs", "C:/проект"), "a.rs");
    }

    #[test]
    fn hidden_characters_do_not_fit() {
        assert!(fits("cargo build --release"));
        assert!(fits("ls «проект»"));
        assert!(!fits("echo ok\u{2028}rm -rf ~"));
        assert!(!fits("echo \u{202E}txt.exe"));
        assert!(!fits("rm\u{200B} -rf"));
        assert!(!fits("a\tb"));
        assert!(!fits(" padded "));
        assert!(!fits("e\u{301}cho")); // combining mark
        assert!(!fits("ѕudo")); // Cyrillic dze: not in the Russian alphabet
        assert!(fits("Ёлка и ёж"));
        assert!(!fits(&"x".repeat(TEXT_MAX + 1)));
    }

    /// Nothing can be allowed from the island that the card does not show.
    #[test]
    fn no_allow_for_hidden_parts() {
        let partial = json!(["Терминал", "Нет"]);
        let multi = json!({"hook_event_name":"PermissionRequest","cwd":"C:/p/demo","tool_name":"Bash","tool_input":{"command":"echo hi\nrm -rf ~"}});
        let Some(Plan::Permission(m)) = plan(&multi) else { panic!() };
        assert_eq!(m["options"], partial);
        let long = json!({"hook_event_name":"PermissionRequest","cwd":"C:/p/demo","tool_name":"Bash","tool_input":{"command":"x".repeat(200)}});
        let Some(Plan::Permission(m)) = plan(&long) else { panic!() };
        assert_eq!(m["options"], partial);
        let mcp = json!({"hook_event_name":"PermissionRequest","tool_name":"mcp__telegram__send_message","tool_input":{"text":"x"}});
        let Some(Plan::Permission(m)) = plan(&mcp) else { panic!() };
        assert_eq!(m["options"], partial);
        let edit = json!({"hook_event_name":"PermissionRequest","cwd":"C:/p/demo","tool_name":"Edit","tool_input":{"file_path":"C:/Windows/System32/drivers/etc/hosts"}});
        let Some(Plan::Permission(m)) = plan(&edit) else { panic!() };
        assert_eq!(m["text"], "demo: C:/Windows/System32/drivers/etc/hosts");
    }

    #[test]
    fn tools_described() {
        assert_eq!(describe_tool("Edit", &json!({"file_path":"C:/x/src/main.rs"}), "C:\\x").1, "src/main.rs");
        assert_eq!(describe_tool("mcp__telegram__send_message", &json!({}), ""), ("Инструмент send_message?".to_string(), "telegram".to_string(), false));
    }

    #[test]
    fn single_question() {
        let questions = json!([{"question":"Какой этап?","header":"Этап","multiSelect":false,"options":[{"label":"Этап 12","description":""},{"label":"Этап 14","description":""}]}]);
        let e = json!({"hook_event_name":"PreToolUse","cwd":"C:/p/demo","tool_name":"AskUserQuestion","tool_input":{"questions":questions}});
        let Some(Plan::Question { msg, question, labels, .. }) = plan(&e) else { panic!() };
        assert_eq!(msg["options"], json!(["Этап 12", "Этап 14", "✕"]));
        let out = question_answer(Some(1), &questions, &question, &labels).unwrap();
        assert_eq!(out["hookSpecificOutput"]["updatedInput"]["answers"]["Какой этап?"], "Этап 14");
        assert!(question_answer(Some(2), &questions, &question, &labels).is_none());
    }

    #[test]
    fn skips_what_does_not_fit() {
        let multi = json!({"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"a","multiSelect":true,"options":[{"label":"x"}]}]}});
        assert!(plan(&multi).is_none());
        let two = json!({"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"a","options":[{"label":"x"}]},{"question":"b","options":[{"label":"y"}]}]}});
        assert!(plan(&two).is_none());
        let other = json!({"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{}});
        assert!(plan(&other).is_none());
        let prompt = json!({"hook_event_name":"Notification","notification_type":"permission_prompt","message":"x"});
        assert!(plan(&prompt).is_none());
    }

    #[test]
    fn notices() {
        let stop = json!({"hook_event_name":"Stop","cwd":"/a/b","last_assistant_message":"Готово.\nДетали ниже"});
        let Some(Plan::Notice(m)) = plan(&stop) else { panic!() };
        assert_eq!(m["title"], "Claude закончил");
        assert_eq!(m["text"], "b: Готово.");
        let idle = json!({"hook_event_name":"Notification","notification_type":"idle_prompt","message":"Claude is waiting for your input"});
        let Some(Plan::Notice(m)) = plan(&idle) else { panic!() };
        assert_eq!(m["title"], "Claude ждёт вас");
    }

    #[test]
    fn shortens() {
        assert_eq!(short("  абвгдеж  ", 5), "абвг…");
        assert_eq!(short("ok", 5), "ok");
    }
}
