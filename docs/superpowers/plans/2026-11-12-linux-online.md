# Linux online services implementation checkpoint — 2026-11-12

Scope: `crates/tidemark-net` only. This is a portable synchronous service library, not a completed Linux application. Windows source remains unchanged. Standalone `[workspace]`, private package, pinned `Cargo.lock`.

## Public integration contract

```rust
let client = tidemark_net::Client::new(); // Clone shares the ureq connection pool
let weather = client.weather("London")?; // Result<Option<weather::Weather>>
let lyrics = client.lyrics(&tidemark_net::lyrics::TrackQuery {
    artist: "Synthetic artist".into(), title: "Synthetic title".into(),
    duration: std::time::Duration::from_secs(180),
})?; // Result<Option<lyrics::Lyrics>>
// Only invoke from the user's Translate action; this submits text to providers.
let translated = client.translate("example", tidemark_net::translate::TargetLanguage::Russian)?;
```

Run calls off the GTK main thread. Calls create no threads, timers or background requests. Construction is offline. Empty weather city is `Ok(None)` with no request; blank translation returns an empty string with no request. Only these three operations submit caller-supplied city, track metadata or explicitly selected translation text. There is no browser access or music control.

`weather::Weather` supplies exactly Windows UI's `temperature: f32` (Celsius), `icon: Icon`, `is_day: bool`, and `temperature_label()`. `Icon` preserves eight WMO/WorldWeatherOnline categories. Open-Meteo geocoding and forecast precede wttr.in fallback. `weather_at(city, local_minutes_since_midnight)` enables a supplied local clock; `weather(city)` uses chrono's local clock. As on Windows, wttr day/night compares sunrise/sunset with the computer's local clock, which may differ from a remote city's clock. City coordinates are validated. Caller controls refresh and sticky fallback preferences; the library does not persist source/coordinate state.

`lyrics::TrackQuery` fields are `artist: String`, `title: String`, `duration: Duration` (zero = unknown). LRCLIB `/api/get` precedes cleaned-title `/api/search`. Duration tolerance is four seconds when both sides know it. `Lyrics` fields are `synced: Vec<Line>`, `plain: Option<String>`, `instrumental: bool`. `Line` fields are `at: Duration` and `text: String`. Empty timed lines represent instrumental gaps. `parse_lrc`, `line_at`, `until_next`, `clean_title`, `parse_search` are public. Multiple timestamps, metadata, sorting, fractional seconds and checked arithmetic are supported; malformed Unicode or overflowing timestamps are skipped. Search prefers synced lyrics, then plain lyrics. No usable record is `Ok(None)`; network failure is an error, preventing accidental negative caching. The application owns caching (Windows positive disk cache and three-day negative cache), newest-track coalescing, and rejecting stale results. These application integrations are still pending.

`translate::TargetLanguage::{English,Russian}` exposes `code()`, with `target_for(text)` preserving Windows Cyrillic/Latin heuristic. Google `gtx` remains an unofficial compatibility path; there is no official support or availability promise. On failure, MyMemory is used for the same complete input only when it fits 500 UTF-8 bytes. Unlike Windows' 450-character truncation, this does not exceed MyMemory's byte limit or silently return partial translation. Google input is capped at 5,000 UTF-8 bytes. No API keys, cookies, email registration, credentials or paid API are used. MyMemory success requires `responseStatus: 200`, avoiding display of quota/error messages as translations.

`Error` variants: `InvalidInput`, `Transport`, `Status(u16)`, `RateLimited { retry_after: Duration }`, `TooLarge`, `InvalidResponse`, `NotFound`. Display/Debug contain no upstream message, request URL or private text. `Result<T>` is the crate alias. HTTP 429 carries seconds from Retry-After (minimum one; default 60 if unavailable/unsupported). There is no automatic retry. Caller must honor the delay, add jitter and avoid batch scans. HTTP-date Retry-After is currently treated as unavailable. Other non-200 statuses are explicit. Production clients use fixed HTTPS hosts and disable redirects, including cross-host redirects; there is no arbitrary endpoint configuration.

## Transport and privacy

ureq 3.5.2, `default-features = false`, `rustls` only. No OpenSSL runtime dependency, cookie jar or decompression feature. Responses are limited to 1 MiB plus a one-byte overflow sentinel while reading (including chunked or missing-length bodies), UTF-8 validated. Each request has a 20-second overall budget, five-second connect budget and eight-second per-read budget. A weather call can perform three sequential requests; translation or lyrics can perform two. Environment HTTP proxy support follows ureq defaults; transport errors are redacted.

The crate itself emits no logs. **Application logging must disable TRACE for ureq and ureq_proto:** upstream ureq's `DebugUri` normally masks path/query but exposes it when global TRACE is enabled. Do not log request structs, URLs, query inputs or successful translation/lyrics bodies. This is a required app integration constraint, not evidence that private text was transmitted in tests. No telemetry is added.

## Verification and remaining gates

Tests were first compiled against the empty crate and failed for missing APIs. A separately added extreme-temperature regression failed with the inherited i32-negation overflow; formatting now uses floating-point magnitude. A rate-limit contract test failed for missing retry metadata, then passed after implementation.

Windows host checks: `cargo test --locked --manifest-path crates/tidemark-net/Cargo.toml` passes (12 tests), and `cargo fmt --manifest-path crates/tidemark-net/Cargo.toml --check` passes. Clippy is run with `--all-targets -- -D warnings`. Tests use synthetic text/lyrics or loopback TcpListener only, including chunked data, oversize body, status redaction and disabled redirects. No real user text, production track metadata or city requests were sent. Native Ubuntu builds and live provider smoke checks remain pending; none of this establishes GTK integration or full feature parity.

## Primary references checked

- [Open-Meteo forecast API](https://open-meteo.com/en/docs): current weather fields and WMO codes; existing endpoint/fields retained.
- [LRCLIB API](https://lrclib.net/docs), [machine-readable specification](https://lrclib.net/docs.md): stable v1 get/search; anonymous access; client identification; response fields and Retry-After requirements. No beta v2 migration.
- [MyMemory API](https://mymemory.translated.net/doc/spec.php): UTF-8 query maximum is 500 bytes, language pair syntax retained.
- [ureq 3.5.2](https://docs.rs/ureq/3.5.2/ureq/), [configuration](https://docs.rs/ureq/3.5.2/ureq/config/struct.ConfigBuilder.html): blocking rustls transport, timeouts and redirect controls.

Google gtx has no supported public API contract verified here; retaining it is a compatibility decision. wttr response parsers preserve Windows' existing schema/condition mapping. Network availability, quota limits and provider terms still need release review and native smoke verification.
