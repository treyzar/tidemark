//! Claude/opencode bridge. Any unavailable UI, invalid reply, or timeout falls back to terminal.
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    time::Duration,
};
use tidemark_agent::{
    planner,
    protocol::{MAX_REQUEST, MAX_WAIT},
    Message,
};
fn exchange(msg: &Message, wait: Duration) -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        let xdg = std::env::var_os("XDG_RUNTIME_DIR");
        let runtime = tidemark_agent::linux::resolve_runtime(
            xdg.as_deref().map(std::path::Path::new),
            unsafe { libc::geteuid() },
        )
        .ok()?;
        tidemark_agent::linux::send(&runtime, msg, wait)
            .ok()
            .flatten()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (msg, wait);
        None
    }
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let raw = args.iter().any(|a| a == "--raw");
    let wait = args
        .iter()
        .position(|a| a == "--wait")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let wait = Duration::from_secs(wait).min(MAX_WAIT);
    let mut input = String::new();
    if std::io::stdin()
        .take((MAX_REQUEST + 1) as u64)
        .read_to_string(&mut input)
        .is_err()
        || input.len() > MAX_REQUEST
    {
        return;
    }
    let Ok(event) = serde_json::from_str::<Value>(&input) else {
        return;
    };
    let output = if raw {
        let choice = planner::raw_message(&event).and_then(|msg| exchange(&msg, wait));
        Some(json!({"choice":choice}))
    } else {
        let Some(plan) = planner::plan(&event) else {
            return;
        };
        let Ok(msg) = serde_json::from_value::<Message>(plan.message().clone()) else {
            return;
        };
        plan.answer(exchange(&msg, wait))
    };
    if let Some(out) = output {
        let _ = writeln!(std::io::stdout(), "{out}");
    }
}
