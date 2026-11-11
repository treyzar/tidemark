//! Bridge for coding agents (Claude Code hooks, the opencode plugin): a
//! named pipe that `island-hook.exe` talks to. Each connection carries one
//! JSON line: a notice to show, or a card with buttons whose answer goes
//! back on the same connection. Only this user's processes can open it.
//!
//! Wire format (one line each way):
//!   → {"kind":"notice"|"ask","source":"Claude","title":"…","text":"…","options":["…"]}
//!   ← {"choice":1} or {"choice":null}   (only for "ask")

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{FlushFileBuffers, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Longest a card may wait for an answer (the hook gives up earlier).
const MAX_WAIT: Duration = Duration::from_secs(15 * 60);
const MAX_LINE: usize = 64 * 1024;

pub fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let user: String = user.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
    format!(r"\\.\pipe\win-island-agent-{user}")
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Message {
    pub kind: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub options: Vec<String>,
}

/// Something for the UI thread.
pub enum Event {
    Notice(Message),
    Ask { id: u64, msg: Message, reply: Sender<Option<usize>> },
    /// The asker went away (answered in the terminal, timed out).
    Gone(u64),
}

#[derive(Clone)]
pub struct Inbox(Arc<Mutex<VecDeque<Event>>>);

impl Inbox {
    pub fn take(&self) -> Vec<Event> {
        self.0.lock().map(|mut q| q.drain(..).collect()).unwrap_or_default()
    }
}

/// Starts the pipe server; events arrive as `msg` posted to `hwnd`.
pub fn start(hwnd: isize, msg: u32) -> Inbox {
    let inbox = Inbox(Arc::new(Mutex::new(VecDeque::new())));
    let q = inbox.clone();
    let _ = std::thread::Builder::new().name("agent-pipe".into()).stack_size(64 * 1024).spawn(move || serve(hwnd, msg, q));
    inbox
}

fn post(hwnd: isize, msg: u32, inbox: &Inbox, e: Event) {
    if let Ok(mut q) = inbox.0.lock() {
        q.push_back(e);
    }
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(0), LPARAM(0));
    }
}

fn serve(hwnd: isize, msg: u32, inbox: Inbox) {
    let name: Vec<u16> = pipe_name().encode_utf16().chain(Some(0)).collect();
    // Full access for the owner (this user) and SYSTEM, nobody else.
    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(w!("D:P(A;;GA;;;OW)(A;;GA;;;SY)"), SDDL_REVISION_1, &mut sd, None).is_err() {
            return;
        }
    }
    let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: false.into() };
    let mut first = true;
    let mut next_id = 1u64;
    loop {
        let open_mode = if first { PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE } else { PIPE_ACCESS_DUPLEX };
        let pipe = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                Some(&sa),
            )
        };
        if pipe.is_invalid() {
            if first {
                return; // Someone else owns the name: stay out of it.
            }
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        first = false;
        let connected = unsafe { ConnectNamedPipe(pipe, None).is_ok() || GetLastError() == ERROR_PIPE_CONNECTED };
        // SAFETY: the handle is ours; File closes it when dropped.
        let file = unsafe { File::from_raw_handle(pipe.0) };
        if !connected {
            continue;
        }
        let id = next_id;
        next_id += 1;
        let q = inbox.clone();
        let _ = std::thread::Builder::new().stack_size(64 * 1024).spawn(move || handle(file, id, hwnd, msg, q));
    }
}

fn read_line(file: &mut File) -> Option<String> {
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 1024];
    loop {
        let n = file.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.contains(&b'\n') || buf.len() > MAX_LINE {
            break;
        }
    }
    let end = buf.iter().position(|&b| b == b'\n').unwrap_or(buf.len());
    String::from_utf8(buf[..end].to_vec()).ok()
}

/// Is the other end still connected?
fn alive(file: &File) -> bool {
    let mut avail = 0u32;
    unsafe { PeekNamedPipe(HANDLE(file.as_raw_handle()), None, 0, None, Some(&mut avail), None).is_ok() }
}

fn handle(mut file: File, id: u64, hwnd: isize, wm: u32, inbox: Inbox) {
    let Some(line) = read_line(&mut file) else { return };
    let Ok(msg) = serde_json::from_str::<Message>(&line) else { return };
    if msg.kind != "ask" || msg.options.is_empty() {
        post(hwnd, wm, &inbox, Event::Notice(msg));
        return;
    }
    let (tx, rx): (Sender<Option<usize>>, Receiver<Option<usize>>) = channel();
    post(hwnd, wm, &inbox, Event::Ask { id, msg, reply: tx });
    let start = Instant::now();
    let choice = loop {
        match rx.recv_timeout(Duration::from_millis(400)) {
            Ok(c) => break Some(c),
            Err(RecvTimeoutError::Disconnected) => break None,
            Err(RecvTimeoutError::Timeout) => {
                if !alive(&file) || start.elapsed() > MAX_WAIT {
                    post(hwnd, wm, &inbox, Event::Gone(id));
                    return;
                }
            }
        }
    };
    let reply = match choice.flatten() {
        Some(i) => format!("{{\"choice\":{i}}}\n"),
        None => "{\"choice\":null}\n".to_string(),
    };
    let _ = file.write_all(reply.as_bytes());
    let _ = file.flush();
    unsafe {
        let _ = FlushFileBuffers(HANDLE(file.as_raw_handle()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_messages() {
        let m: Message = serde_json::from_str(r#"{"kind":"ask","source":"Claude","title":"Выполнить команду?","text":"x: ls","options":["Терминал","Нет","✓"]}"#).unwrap();
        assert_eq!(m.options.len(), 3);
        let n: Message = serde_json::from_str(r#"{"kind":"notice","title":"Готово"}"#).unwrap();
        assert!(n.options.is_empty() && n.text.is_empty());
    }

    #[test]
    fn pipe_name_is_clean() {
        assert!(pipe_name().starts_with(r"\\.\pipe\win-island-agent-"));
        assert!(!pipe_name()[9..].contains('\\'));
    }
}
