# tidemark-artwork — plan

Bounded artwork fetch+decode for the Tidemark Ubuntu port. Not a stub: every limit maps to the
verified upstream API (docs.rs ureq 3.5.2, image 0.25.10, sha2 0.11.1, base64 0.23.1, url 2.x).

## Stack decision

- HTTP: **ureq 3.5.2**, pure Rust, `rustls` TLS (ring provider, webpki-roots bundle by default).
  Rejected: ureq 2 (superseded, different API). `default-features = false, features = ["rustls"]`
  — gzip/brotli decompression stays **off** so no compressed response can inflate past the byte
  cap, and no `Accept-Encoding` is advertised.
- Decode: **image 0.25.10** (`png`, `jpeg`, `webp` only), `ImageReader::with_guessed_format` +
  strict `Limits`. ID: **sha2 0.11.1**. Data URIs: **base64 0.23.1**. URLs: **url 2.x**.
- Crate is a **standalone `[workspace]`** (mirrors tidemark-mpris) until the parent integrates it.
- Pure Rust on all platforms, **no platform gating**: `cargo test` runs fully on the Windows host.

## API

- `fetch(url: &str) -> Result<Cover>`; `fetch_with(url: &str, agent: &ureq::Agent)` for callers
  (and tests) that need a custom agent/timeout.
- `Cover { id: u64, pixels: Vec<u8> }` — `pixels` is RGBA, `COVER_PX * COVER_PX * 4` bytes
  (160x160), parity with Windows `COVER_PX = 160` (src/media.rs:25).
- `id`: SHA-256 of the **source image bytes** (not the crop), first 8 bytes big-endian —
  deterministic across platforms (Windows uses `DefaultHasher`, which is not stable; this is the
  content-derived replacement).

## Safety contracts (per the delegated task)

- Schemes: `https`/`http`/`file`/`data` only (`Error::Scheme` otherwise).
- No userinfo: `username`/`password` must be empty in the initial URL and every redirect target.
- No auth/cookies: we never set `Authorization`; ureq `cookies` feature is off; ureq
  `redirect_auth_headers` default `Never` is doubly enforced because redirects are followed
  **manually** (see below) with fresh header-less requests.
- **max compressed 8 MiB**: checked on `Content-Length` before reading, on the decoded data-URI
  size before decode, on `file` metadata, and via `Body::with_config().limit(8 MiB)` while
  reading (overshoot becomes `Error::TooLarge`, not an unbounded buffer).
- **total timeout 5 s**: `Agent::timeout_global(5 s)` — one budget across DNS, connect, redirects
  and body read (ureq 3 docs: global budget is not restarted per redirect).
- **max 3 redirects, same-scheme only**: ureq's auto-follow cannot enforce scheme equality, so
  `max_redirects(0)` and a manual loop: each `Location` is joined onto the current URL, its
  scheme must equal the initial scheme (`Error::Redirect`), userinfo is rejected, hop count ≤ 3.
- `file://`: authority must be empty or `localhost` (`Error::FileAuthority`), then
  `Url::to_file_path()` — absolute local path only; remote UNC hosts are rejected by the
  authority rule.
- `data:` URIs: must be `image/*` + `;base64`, decoded length bounded by the 8 MiB cap **before**
  `base64::decode` allocates (`Error::DataUri`).
- **No shell-outs anywhere.** No generated fixture images served over the real network: HTTP
  tests use a local `TcpListener` on 127.0.0.1:0 serving canned bytes; synthetic PNG/JPEG are
  produced in-test with the `image` crate's encoders; WebP decode is compiled in but no synthetic
  lossy-WebP is generated (VP8L encoding is not exposed) — noted as an untested path.
- Decoder bomb bounds: `Limits { max_image_width: 4096, max_image_height: 4096, max_alloc: 96 MiB }`
  — a PNG claiming 100000x100000 fails on dimension limits before allocation.
- Geometry: centered square crop `side = min(w, h)` then `imageops::resize` to 160x160
  (CatmullRom), matching the Windows WIC pipeline (centered `WICBitmapClipper` + `BitmapScaler`
  to COVER_PX, src/media.rs:401).

## Tests

- `source.rs` (portable): scheme/userinfo/authority/file-path/data-URI validation, base64 size
  bound rejected before allocation.
- `transport.rs` tests (local TcpListener fixture, one thread per test): 200 PNG roundtrip,
  301→200 same-scheme redirect chain, 4-hop chain rejected, cross-scheme Location rejected
  (Location to a `https://` absolute URL while listening on http), 4xx status, oversized
  Content-Length rejected without reading, close-delimited body exceeding the read limit, request
  never sends Authorization/Cookie (server asserts headers absent), stalled server + short-timeout
  agent → `Error::Timeout`.
- `decode.rs` tests: PNG/JPEG roundtrip to 160x160 RGBA, non-square center-crop geometry,
  non-image bytes → `Error::Decode`, bomb-dimension PNG (CRC-correct IHDR) rejected by limits,
  id determinism (same bytes → same id; different bytes → different id; id equals SHA-256 prefix).
- `file.rs` tests: temp-file fetch via `file:///`, local file authority, remote authority
  rejected, missing file error.
- Execution: Windows `cargo test --locked` — **24/24 pass** (6 decode + 8 scheme/file/data + 10 HTTP fixture).
  Regression checks include one shared deadline across delayed redirects and errors that do not echo URL credentials.
  Local reads use a capped handle reader, not an unbounded read after a size check; Unix opens are nonblocking and reject non-regular files. The Unix FIFO regression test still requires native Linux CI.
  Windows **cross-check to x86_64-unknown-linux-gnu is blocked on this host**: `ureq`'s rustls
  stack pulls `ring`, whose build script needs an `x86_64-linux-gnu-gcc` cross compiler that is
  not installed. Unix file handling and native dependencies are validated by Linux CI.
  Authoritative validation is the parent's Ubuntu CI:
  `cargo test --manifest-path crates/tidemark-artwork/Cargo.toml --locked` (no special
  dependencies needed on the runner — unlike tidemark-mpris, no `dbus-run-session`).
- `Cargo.lock` kept in the deliverable for `--locked` CI.
