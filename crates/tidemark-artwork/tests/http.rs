//! Transport behaviour against the local TcpListener fixture. No external network.

mod fixture;

use fixture::{header_map, image_response, redirect, serve, tiny_png};
use std::time::Duration;
use tidemark_artwork::{fetch, fetch_with_agent, transport, Error};

#[test]
fn direct_200_image_roundtrips() {
    let png = tiny_png();
    let f = serve(vec![image_response(&png)]);
    let cover = fetch(&format!("{}/cover.png", f.base_url)).unwrap();
    assert_eq!(cover.pixels.len(), 160 * 160 * 4);
    assert_eq!(f.requests().len(), 1);
}

#[test]
fn same_scheme_redirect_chain_is_followed_up_to_three_hops() {
    let png = tiny_png();
    let f = serve(vec![
        redirect("/hop2"),
        redirect("/hop3"),
        redirect("/cover.png"),
        image_response(&png),
    ]);
    let cover = fetch(&format!("{}/hop1", f.base_url)).unwrap();
    assert_eq!(cover.pixels.len(), 160 * 160 * 4);
    assert_eq!(f.requests().len(), 4);
}

#[test]
fn a_fourth_redirect_is_refused() {
    let f = serve(vec![
        redirect("/hop2"),
        redirect("/hop3"),
        redirect("/hop4"),
        redirect("/hop5"),
    ]);
    let err = fetch(&format!("{}/hop1", f.base_url)).unwrap_err();
    assert!(matches!(err, Error::Redirect(_)), "got {err:?}");
}

#[test]
fn cross_scheme_redirect_is_refused_without_a_second_request() {
    let f = serve(vec![redirect("https://127.0.0.1:1/cover.png")]);
    let err = fetch(&format!("{}/hop1", f.base_url)).unwrap_err();
    assert!(
        matches!(err, Error::Redirect("scheme change")),
        "got {err:?}"
    );
    assert_eq!(f.requests().len(), 1);
}

#[test]
fn http_status_errors_surface_as_status() {
    let f = serve(vec![
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    ]);
    let err = fetch(&format!("{}/missing.png", f.base_url)).unwrap_err();
    assert!(matches!(err, Error::Status(404)), "got {err:?}");
}

#[test]
fn oversized_content_length_is_rejected_before_reading_the_body() {
    let f = serve(vec![
        b"HTTP/1.1 200 OK\r\nContent-Length: 9000000\r\nConnection: close\r\n\r\n".to_vec(),
    ]);
    let err = fetch(&format!("{}/big.png", f.base_url)).unwrap_err();
    assert!(matches!(err, Error::TooLarge), "got {err:?}");
}

#[test]
fn close_delimited_body_over_the_cap_is_rejected() {
    let mut body = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
    body.extend(std::iter::repeat_n(0u8, 9 * 1024 * 1024));
    let f = serve(vec![body]);
    let err = fetch(&format!("{}/trickle.png", f.base_url)).unwrap_err();
    assert!(matches!(err, Error::TooLarge), "got {err:?}");
}

#[test]
fn no_authorization_or_cookie_headers_are_ever_sent() {
    let png = tiny_png();
    let f = serve(vec![image_response(&png)]);
    fetch(&format!("{}/cover.png", f.base_url)).unwrap();
    for request in f.requests() {
        let headers = header_map(&request);
        assert!(
            !headers.contains_key("authorization"),
            "leaked: {headers:?}"
        );
        assert!(!headers.contains_key("cookie"), "leaked: {headers:?}");
    }
}

#[test]
fn stalled_server_hits_the_global_timeout() {
    let f = serve(vec![Vec::new()]);
    let agent = transport::agent(Duration::from_millis(300));
    let start = std::time::Instant::now();
    let err = fetch_with_agent(&format!("{}/stall.png", f.base_url), &agent).unwrap_err();
    assert!(matches!(err, Error::Timeout), "got {err:?}");
    assert!(start.elapsed() < Duration::from_secs(10));
}

#[test]
fn redirects_share_one_total_deadline() {
    let delay = Duration::from_millis(180);
    let png = tiny_png();
    let fixture = fixture::serve_delayed(vec![
        (delay, redirect("/second")),
        (delay, redirect("/cover")),
        (delay, image_response(&png)),
    ]);
    let agent = transport::agent(Duration::from_millis(250));
    let started = std::time::Instant::now();
    let result = fetch_with_agent(&format!("{}/first", fixture.base_url), &agent);
    let error = result.err();
    assert!(
        matches!(error.as_ref(), Some(Error::Timeout)),
        "got {error:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}
