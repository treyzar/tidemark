//! Scheme dispatch, file:// and data:// handling, and userinfo rejection.

use base64::Engine as _;
use tidemark_artwork::{fetch, Error};

fn tiny_png() -> Vec<u8> {
    let img = image::ImageBuffer::from_fn(2, 2, |_, _| image::Rgba([1, 2, 3, 255]));
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

#[test]
fn unsupported_schemes_are_rejected() {
    assert!(
        matches!(fetch("ftp://example.com/a.png").unwrap_err(), Error::Scheme(s) if s == "ftp")
    );
    assert!(matches!(fetch("::not a url").unwrap_err(), Error::Url(_)));
}

#[test]
fn userinfo_is_rejected_in_the_initial_url() {
    assert!(matches!(
        fetch("http://user:pass@example.com/a.png").unwrap_err(),
        Error::Userinfo
    ));
    assert!(matches!(
        fetch("https://user@example.com/a.png").unwrap_err(),
        Error::Userinfo
    ));
}

#[test]
fn invalid_url_errors_do_not_echo_credentials() {
    let error = fetch("http://user:private-value@[invalid-ipv6")
        .err()
        .unwrap();
    assert!(!error.to_string().contains("private-value"));
}

#[cfg(unix)]
#[test]
fn fifo_input_is_refused_without_waiting_for_a_writer() {
    use std::os::unix::ffi::OsStrExt;
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("tidemark-art-fifo-{}-{suffix}", std::process::id()));
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    struct Remove(std::path::PathBuf);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _remove = Remove(path.clone());
    let started = std::time::Instant::now();
    let error = fetch(url::Url::from_file_path(path).unwrap().as_str()).err();
    assert!(
        matches!(error.as_ref(), Some(Error::NonRegularFile)),
        "got {error:?}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn data_uris_decode_to_a_cover() {
    let b64 = base64::engine::general_purpose::STANDARD.encode(tiny_png());
    let cover = fetch(&format!("data:image/png;base64,{b64}")).unwrap();
    assert_eq!(cover.pixels.len(), 160 * 160 * 4);
    assert_eq!(cover.pixels[0..4], [1, 2, 3, 255]);
}

#[test]
fn data_uris_are_strictly_validated() {
    let b64 = base64::engine::general_purpose::STANDARD.encode(tiny_png());
    assert!(fetch(&format!("data:text/plain;base64,{b64}"))
        .unwrap_err()
        .to_string()
        .contains("data uri"));
    assert!(fetch(&format!("data:image/png,{b64}")).is_err());
    assert!(fetch("data:image/png;base64,!!!not base64!!!").is_err());
    assert!(fetch("data:image/png;base64,AAAA").is_err());
}

#[test]
fn oversized_data_uris_are_rejected_before_allocation() {
    let oversized = "A".repeat(12 * 1024 * 1024);
    let err = fetch(&format!("data:image/png;base64,{oversized}")).unwrap_err();
    assert!(matches!(err, Error::TooLarge), "got {err:?}");
}

#[test]
fn file_uris_accept_local_absolute_paths() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("tidemark-artwork-test-{}.png", std::process::id()));
    std::fs::write(&path, tiny_png()).unwrap();
    let url = url::Url::from_file_path(&path).unwrap().to_string();
    let cover = fetch(&url).unwrap();
    assert_eq!(cover.pixels[0..4], [1, 2, 3, 255]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn file_uris_reject_remote_authorities_and_missing_files() {
    assert!(matches!(
        fetch("file://host.example.com/share/a.png").unwrap_err(),
        Error::FileAuthority(h) if h == "host.example.com"
    ));
    assert!(fetch("file:///definitely/missing/tidemark-artwork.png").is_err());
}
