# Linux agent IPC implementation

The `tidemark-agent` standalone workspace provides the Linux `island-hook` executable and a transport library for the GTK host. It is part of the Ubuntu port, not a complete application.

## Integration API

- `planner::plan(&Value) -> Option<Plan>` preserves the Windows hook's existing Claude event planning. `Plan::message()` returns its card JSON; `Plan::answer(Option<usize>)` returns the original Claude hook answer. Only explicit valid button selections produce decisions.
- `planner::raw_message(&Value) -> Option<Message>` preserves the original raw truncation/visibility policy. A hidden, mixed-script, multiline or truncated command cannot offer the approval tick.
- `linux::resolve_runtime(Option<&Path>, uid)` validates XDG runtime input, or `/run/user/<uid>` when absent. Invalid explicit inputs fail closed.
- `linux::Server::bind(&runtime, Limits)` starts the service. Keep this handle alive; dropping it interrupts and joins listener/client workers. Poll `try_recv()` from the GTK main loop.
- `Incoming::Notice(Message)`, `Incoming::Ask { id, msg, reply: Sender<Option<usize>> }`, and `Incoming::Gone(id)` connect transport to user cards. Send a selected index, or `None` on dismissal. Handle Gone by removing the matching card. Never render payloads as GTK markup, and never synthesize an approval.
- `linux::send(&runtime, &Message, wait)` performs one authenticated exchange. Error, missing index, invalid index or timeout means terminal fallback.

The CLI reads Claude events from stdin, supports `--raw` and `--wait SECONDS` (50 seconds by default), emits no Claude response on fallback, and emits `{"choice":null}` for raw fallback. It does not edit Claude/opencode configuration or send live user messages.

## Security and resource bounds

The endpoint is `$XDG_RUNTIME_DIR/tidemark/agent.sock`, otherwise `/run/user/<uid>/tidemark/agent.sock`. Runtime and subdirectory must be owned by the effective UID with mode 0700. Relative paths, symlinks, foreign owners and public directories are refused. Directory descriptors, no-follow opens, and `/proc/self/fd` paths anchor operations to validated directories. The socket is owned and mode 0600. Linux `SO_PEERCRED` verifies the current effective UID at both ends.

An owned regular 0600 lock file holds nonblocking exclusive `flock` throughout the server lifetime. An active endpoint is never replaced. Only an owned socket producing ECONNREFUSED can be recovered while this lock is held; inode/device identity is checked before removing it. Symlinks, directories and regular files are preserved. Shutdown removes only the socket inode created by this instance; lock and runtime directories remain. Processes with the same UID are the trust boundary, matching the Windows same-user pipe model.

Requests are newline framed with a 4 MiB body limit; replies have a 4096-byte limit. Unterminated frames fail. Allocation stops at the limit. Absolute I/O deadlines prevent slow-drip extension; connect is nonblocking and capped at three seconds. Request reads default to three seconds (hard cap ten); answers default/hard cap to fifteen minutes. The hook wait is capped to fifteen minutes. Default simultaneous clients: 16; configurable hard cap: 64. Pending UI events: 64, using nonblocking enqueue. Full queues reject new events. Ask lifecycle state has a separate hard limit of 128 entries; exhaustion rejects new asks and preserves terminal fallback. Cancellation updates existing lifecycle state rather than competing with notices for queue capacity. `try_recv()` prioritizes cancellation of already delivered cards; a canceled queued Ask is replaced with Gone when dequeued, so it can never appear later as an actionable stale card. Lifecycle slots are released after completion or cancellation delivery. Workers never wait for queue space, including shutdown. Reply indexes are validated independently on server and client. Shutdown interrupts pending reads/answers and waits only for polling workers. Error text excludes message payloads.

## Validation and remaining platform gate

Planner RED was observed by replacing planning with terminal fallback: four original behavior tests failed. Restoring the unchanged planning logic passed all nine original tests. Windows `cargo test --locked --manifest-path crates/tidemark-agent/Cargo.toml` passes 16 portable tests after the review fixes below. Added tests cover raw hidden content, explicit-index answers and bounded newline framing. `cargo check --locked --target x86_64-unknown-linux-gnu --tests` and Linux-target clippy with `-D warnings` pass on the Windows host.

Native Linux tests cover actual Unix socket ask/notice exchange, explicit valid and invalid answers, SO_PEERCRED checks/mismatch, active versus stale endpoint handling, preservation of files/symlinks/replaced inodes, runtime permission validation, timeout/disconnect/missing newline, oversized reply, and bounded shutdown. They use private temp fixtures and never mutate HOME/XDG environment or contact live agents. Native tests were not executed on this Windows host: only docker-desktop WSL was installed and its Docker engine was unavailable. Ubuntu CI must execute `cargo test --locked --manifest-path crates/tidemark-agent/Cargo.toml` before merging. Linux transport tests have not yet had a native RED/GREEN run.

## Review corrections: Linux paths and saturated cancellation

Linux path presentation now treats case differences as significant and backslashes as literal filename characters, including the project label. Absolute paths outside the actual current directory remain fully qualified on permission cards. Windows retains its previous slash normalization and ASCII case-insensitive comparison. The portable regression exercises both policies; a native Linux regression checks the complete permission card for `/home/u/PROJECT/secret` with cwd `/home/u/project` and literal backslash paths. Existing Windows path examples still pass under the explicit Windows policy.

Cancellation no longer uses a best-effort `Gone` send into the event queue. A bounded lifecycle registry tracks whether each accepted ask was delivered and canceled, with RAII cancellation on every premature handler exit. Successful answers and rejected enqueues release their slots directly. Portable tests cover full lifecycle-state capacity, cancellation of visible/queued asks, completion, and slot reuse. A native regression drives the real handler through a UnixStream pair, queues its Ask, deterministically fills all 64 event slots, disconnects the client, waits for the worker to finish, and checks Gone precedes notices (delivered case) or replaces the stale Ask (queued case). It checks worker completion before draining the saturated queue and prompt server drop, with no blocking worker enqueue.

Review-fix verification on Windows: 16 portable tests pass; native Linux target `cargo check --locked --all-targets` passes. Native execution of the Unix regression remains pending Ubuntu CI. This checkpoint does not claim a native RED/GREEN run.
