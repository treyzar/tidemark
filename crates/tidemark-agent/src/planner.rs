use serde_json::{json, Value};
#[derive(Debug, PartialEq)]
pub enum Plan {
    Notice(Value),
    Permission(Value),
    Question {
        msg: Value,
        questions: Value,
        question: String,
        labels: Vec<String>,
    },
}

const PERMISSION_OPTIONS: [&str; 3] = ["Терминал", "Нет", "✓"];
/// When the card cannot show everything that would be allowed.
const PERMISSION_OPTIONS_PARTIAL: [&str; 2] = ["Терминал", "Нет"];
/// The card's second line holds this many characters.
const TEXT_MAX: usize = 160;
const TITLE_MAX: usize = 80;

fn project(event: &Value) -> String {
    let separators: &[char] = if cfg!(windows) { &['\\', '/'] } else { &['/'] };
    let cwd = event["cwd"]
        .as_str()
        .unwrap_or_default()
        .trim_end_matches(separators);
    cwd.rsplit(separators)
        .next()
        .unwrap_or_default()
        .to_string()
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
    shown_path_with_platform(p, cwd, cfg!(windows))
}
fn shown_path_with_platform(p: &str, cwd: &str, windows: bool) -> String {
    let path = if windows {
        p.replace('\\', "/")
    } else {
        p.to_owned()
    };
    let base = if windows {
        cwd.replace('\\', "/")
    } else {
        cwd.to_owned()
    };
    let base = base.trim_end_matches('/');
    let plain = !path.split('/').any(|part| part == ".." || part == ".");
    let inside = plain
        && !base.is_empty()
        && path.len() > base.len() + 1
        && path.is_char_boundary(base.len())
        && if windows {
            path[..base.len()].eq_ignore_ascii_case(base)
        } else {
            &path[..base.len()] == base
        }
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
            let path = if s("file_path").is_empty() {
                s("notebook_path")
            } else {
                s("file_path")
            };
            ("Изменить файл?".into(), shown_path(&path, cwd), true)
        }
        "Read" => (
            "Прочитать файл?".into(),
            shown_path(&s("file_path"), cwd),
            true,
        ),
        "WebFetch" => ("Открыть страницу?".into(), s("url"), true),
        "WebSearch" => ("Искать в сети?".into(), s("query"), true),
        t if t.starts_with("mcp__") => {
            let mut parts = t.splitn(3, "__").skip(1);
            let server = parts.next().unwrap_or_default();
            let name = parts.next().unwrap_or_default();
            (
                format!("Инструмент {}?", short(name, 28)),
                server.to_string(),
                false,
            )
        }
        t => (
            format!("Разрешить {}?", short(t, 28)),
            input.to_string(),
            false,
        ),
    }
}

/// Fits the card's second line in full: one line, not cut.
/// Shown whole: `short(text, max)` leaves it as is, and every character is
/// one that draws as what it is.
fn fits_in(text: &str, max: usize) -> bool {
    short(text, max) == text && text.chars().all(allowed_char) && !mixed_script(text)
}

fn fits(text: &str) -> bool {
    fits_in(text, TEXT_MAX)
}

/// Characters a card may show next to ✓: printable ASCII, the Russian
/// alphabet and « » ° § € №. An allowlist, so invisible, bidi, combining
/// and ASCII look-alike characters (dashes, dots, quotes) never pass.
fn allowed_char(c: char) -> bool {
    matches!(c as u32, 0x20..=0x7E | 0xA7 | 0xAB | 0xB0 | 0xBB | 0x401 | 0x410..=0x44F | 0x451 | 0x20AC | 0x2116)
}

/// A word mixing Latin and Cyrillic letters ("pаypal" with a Cyrillic а)
/// reads as something it is not.
fn mixed_script(text: &str) -> bool {
    text.split(|c: char| !c.is_alphabetic()).any(|w| {
        w.chars().any(|c| c.is_ascii_alphabetic())
            && w.chars().any(|c| matches!(c as u32, 0x400..=0x4FF))
    })
}

fn card(kind: &str, title: &str, text: &str, options: &[String]) -> Value {
    json!({ "kind": kind, "source": "Claude", "title": short(title, TITLE_MAX), "text": short(text, TEXT_MAX), "options": options })
}

pub fn plan(event: &Value) -> Option<Plan> {
    let project = project(event);
    let with_project = |s: &str| {
        if project.is_empty() {
            s.to_string()
        } else if s.is_empty() {
            project.clone()
        } else {
            format!("{project}: {s}")
        }
    };
    match event["hook_event_name"].as_str()? {
        "PermissionRequest" => {
            let tool = event["tool_name"].as_str()?;
            if tool == "AskUserQuestion" || tool == "ExitPlanMode" {
                return None; // Those are not yes/no questions.
            }
            let (title, detail, complete) = describe_tool(
                tool,
                &event["tool_input"],
                event["cwd"].as_str().unwrap_or_default(),
            );
            let text = with_project(&detail);
            // ✓ only when the card shows everything it would allow; a
            // multi-line or cut command can be refused or left to the terminal.
            let options: &[&str] = if complete && fits(&text) && fits_in(&title, TITLE_MAX) {
                &PERMISSION_OPTIONS
            } else {
                &PERMISSION_OPTIONS_PARTIAL
            };
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
            let labels: Vec<String> = q["options"]
                .as_array()?
                .iter()
                .filter_map(|o| o["label"].as_str().map(str::to_string))
                .collect();
            // Buttons must fit the pill: up to three short answers.
            if labels.is_empty()
                || labels.len() > 3
                || labels.iter().map(|l| l.chars().count()).sum::<usize>() > 36
            {
                return None;
            }
            let mut options = labels.clone();
            options.push("✕".into());
            let msg = card(
                "ask",
                &question,
                &with_project(q["header"].as_str().unwrap_or_default()),
                &options,
            );
            Some(Plan::Question {
                msg,
                questions,
                question,
                labels,
            })
        }
        "Notification" => {
            let title = match event["notification_type"].as_str().unwrap_or_default() {
                "permission_prompt" => return None, // PermissionRequest shows it.
                "idle_prompt" => "Claude ждёт вас".to_string(),
                "elicitation_dialog" => "Claude задаёт вопрос".to_string(),
                _ => event["title"]
                    .as_str()
                    .filter(|t| !t.is_empty())
                    .unwrap_or("Claude Code")
                    .to_string(),
            };
            Some(Plan::Notice(card(
                "notice",
                &title,
                &with_project(event["message"].as_str().unwrap_or_default()),
                &[],
            )))
        }
        "Stop" => {
            let last = event["last_assistant_message"].as_str().unwrap_or_default();
            Some(Plan::Notice(card(
                "notice",
                "Claude закончил",
                &with_project(&short(last, 120)),
                &[],
            )))
        }
        _ => None,
    }
}

/// The pressed button's label → the hook's decision.
pub fn permission_answer(label: Option<&str>) -> Option<Value> {
    let decision = match label? {
        "✓" => json!({ "behavior": "allow" }),
        "Нет" => {
            json!({ "behavior": "deny", "message": "Пользователь запретил это в Win Island." })
        }
        _ => return None, // "Терминал": ask there.
    };
    Some(
        json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision } }),
    )
}

pub fn question_answer(
    choice: Option<usize>,
    questions: &Value,
    question: &str,
    labels: &[String],
) -> Option<Value> {
    let label = labels.get(choice?)?;
    Some(json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": { "questions": questions, "answers": { question: label } }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_card() {
        let cwd = if cfg!(windows) {
            "C:\\Users\\ivan\\Documents\\win-island"
        } else {
            "/home/ivan/win-island"
        };
        let e = json!({"hook_event_name":"PermissionRequest","cwd":cwd,"tool_name":"Bash","tool_input":{"command":"cargo build --release"}});
        let Some(Plan::Permission(msg)) = plan(&e) else {
            panic!()
        };
        assert_eq!(msg["title"], "Выполнить команду?");
        assert_eq!(msg["text"], "win-island: cargo build --release");
        assert_eq!(msg["options"].as_array().unwrap().len(), 3);
        assert_eq!(
            permission_answer(Some("✓")).unwrap()["hookSpecificOutput"]["decision"]["behavior"],
            "allow"
        );
        assert_eq!(
            permission_answer(Some("Нет")).unwrap()["hookSpecificOutput"]["decision"]["behavior"],
            "deny"
        );
        assert!(permission_answer(Some("Терминал")).is_none());
        assert!(permission_answer(None).is_none());
    }

    #[test]
    fn paths() {
        let shown_path = |p: &str, cwd: &str| shown_path_with_platform(p, cwd, true);
        assert_eq!(
            shown_path("C:\\P\\Demo\\src\\a.rs", "c:/p/demo"),
            "src/a.rs"
        );
        assert_eq!(
            shown_path("C:/p/demo/../secret.txt", "C:/p/demo"),
            "C:/p/demo/../secret.txt"
        );
        assert_eq!(
            shown_path("C:/p/demox/a.rs", "C:/p/demo"),
            "C:/p/demox/a.rs"
        );
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
        assert!(fits("git commit -m «Готово»"));
        assert!(!fits("curl https://pаypal.com")); // Cyrillic а inside a Latin word
        assert!(!fits("rm \u{2013}rf x")); // en dash looks like -
        assert!(!fits(&"x".repeat(TEXT_MAX + 1)));
    }

    /// Nothing can be allowed from the island that the card does not show.
    #[test]
    fn no_allow_for_hidden_parts() {
        let partial = json!(["Терминал", "Нет"]);
        let multi = json!({"hook_event_name":"PermissionRequest","cwd":"C:/p/demo","tool_name":"Bash","tool_input":{"command":"echo hi\nrm -rf ~"}});
        let Some(Plan::Permission(m)) = plan(&multi) else {
            panic!()
        };
        assert_eq!(m["options"], partial);
        let long = json!({"hook_event_name":"PermissionRequest","cwd":"C:/p/demo","tool_name":"Bash","tool_input":{"command":"x".repeat(200)}});
        let Some(Plan::Permission(m)) = plan(&long) else {
            panic!()
        };
        assert_eq!(m["options"], partial);
        let mcp = json!({"hook_event_name":"PermissionRequest","tool_name":"mcp__telegram__send_message","tool_input":{"text":"x"}});
        let Some(Plan::Permission(m)) = plan(&mcp) else {
            panic!()
        };
        assert_eq!(m["options"], partial);
        let edit = json!({"hook_event_name":"PermissionRequest","cwd":"C:/p/demo","tool_name":"Edit","tool_input":{"file_path":"C:/Windows/System32/drivers/etc/hosts"}});
        let Some(Plan::Permission(m)) = plan(&edit) else {
            panic!()
        };
        assert_eq!(m["text"], "demo: C:/Windows/System32/drivers/etc/hosts");
    }

    #[test]
    fn tools_described() {
        assert_eq!(
            describe_tool("Edit", &json!({"file_path":"/x/src/main.rs"}), "/x").1,
            "src/main.rs"
        );
        assert_eq!(
            describe_tool("mcp__telegram__send_message", &json!({}), ""),
            (
                "Инструмент send_message?".to_string(),
                "telegram".to_string(),
                false
            )
        );
    }

    #[test]
    fn single_question() {
        let questions = json!([{"question":"Какой этап?","header":"Этап","multiSelect":false,"options":[{"label":"Этап 12","description":""},{"label":"Этап 14","description":""}]}]);
        let e = json!({"hook_event_name":"PreToolUse","cwd":"C:/p/demo","tool_name":"AskUserQuestion","tool_input":{"questions":questions}});
        let Some(Plan::Question {
            msg,
            question,
            labels,
            ..
        }) = plan(&e)
        else {
            panic!()
        };
        assert_eq!(msg["options"], json!(["Этап 12", "Этап 14", "✕"]));
        let out = question_answer(Some(1), &questions, &question, &labels).unwrap();
        assert_eq!(
            out["hookSpecificOutput"]["updatedInput"]["answers"]["Какой этап?"],
            "Этап 14"
        );
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
        let Some(Plan::Notice(m)) = plan(&stop) else {
            panic!()
        };
        assert_eq!(m["title"], "Claude закончил");
        assert_eq!(m["text"], "b: Готово.");
        let idle = json!({"hook_event_name":"Notification","notification_type":"idle_prompt","message":"Claude is waiting for your input"});
        let Some(Plan::Notice(m)) = plan(&idle) else {
            panic!()
        };
        assert_eq!(m["title"], "Claude ждёт вас");
    }

    #[test]
    fn shortens() {
        assert_eq!(short("  абвгдеж  ", 5), "абвг…");
        assert_eq!(short("ok", 5), "ok");
    }
}

impl Plan {
    /// JSON card payload, with existing visibility and anti-spoofing policy applied.
    pub fn message(&self) -> &Value {
        match self {
            Self::Notice(m) | Self::Permission(m) | Self::Question { msg: m, .. } => m,
        }
    }
    /// Convert an explicitly chosen button into the original Claude hook response.
    pub fn answer(&self, choice: Option<usize>) -> Option<Value> {
        match self {
            Self::Notice(_) => None,
            Self::Permission(msg) => permission_answer(
                choice
                    .and_then(|i| msg["options"].get(i))
                    .and_then(Value::as_str),
            ),
            Self::Question {
                questions,
                question,
                labels,
                ..
            } => question_answer(choice, questions, question, labels),
        }
    }
}
/// Apply the exact original --raw visibility policy before deserializing the wire message.
pub fn raw_message(event: &Value) -> Option<crate::Message> {
    let mut msg = event.clone();
    let (title, text) = (
        event["title"].as_str().unwrap_or_default(),
        event["text"].as_str().unwrap_or_default(),
    );
    msg["title"] = short(title, TITLE_MAX).into();
    msg["text"] = short(text, TEXT_MAX).into();
    if !(fits_in(title, TITLE_MAX) && fits(text)) {
        if let Some(options) = msg["options"].as_array_mut() {
            options.retain(|o| o != "✓");
        }
    }
    serde_json::from_value(msg).ok()
}

#[cfg(test)]
mod api_tests {
    use super::*;
    #[test]
    fn raw_hidden_text_never_offers_approval() {
        let m=raw_message(&json!({"kind":"ask","title":"run","text":"echo ok\nrm -rf ~","options":["Терминал","Нет","✓"]})).unwrap();
        assert_eq!(m.text, "echo ok");
        assert_eq!(m.options, vec!["Терминал", "Нет"]);
    }
    #[test]
    fn plan_answers_require_an_existing_explicit_button() {
        let p=plan(&json!({"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"ls"}})).unwrap();
        assert!(p.answer(None).is_none());
        assert!(p.answer(Some(999)).is_none());
        assert!(p.answer(Some(0)).is_none());
        assert_eq!(
            p.answer(Some(2)).unwrap()["hookSpecificOutput"]["decision"]["behavior"],
            "allow"
        );
    }
}

#[cfg(test)]
mod platform_path_regressions {
    use super::*;
    #[test]
    fn unix_paths_are_case_sensitive_and_backslashes_literal() {
        assert_eq!(
            shown_path_with_platform("/home/u/PROJECT/secret", "/home/u/project", false),
            "/home/u/PROJECT/secret"
        );
        assert_eq!(
            shown_path_with_platform("/home/u/project\\secret", "/home/u/project", false),
            "/home/u/project\\secret"
        );
        assert_eq!(
            shown_path_with_platform("/home/u/project/a\\b", "/home/u/project", false),
            "a\\b"
        );
        assert_eq!(
            shown_path_with_platform("/home/u/project/../secret", "/home/u/project", false),
            "/home/u/project/../secret"
        );
        assert_eq!(
            shown_path_with_platform("C:\\P\\Demo\\src\\a.rs", "c:/p/demo", true),
            "src/a.rs"
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_permission_keeps_other_directory_unambiguous() {
        let p = plan(&json!({"hook_event_name":"PermissionRequest","cwd":"/home/u/project","tool_name":"Write","tool_input":{"file_path":"/home/u/PROJECT/secret"}})).unwrap();
        assert_eq!(p.message()["text"], "project: /home/u/PROJECT/secret");
        assert_eq!(
            shown_path("/home/u/project/a\\b", "/home/u/project"),
            "a\\b"
        );
    }
}
