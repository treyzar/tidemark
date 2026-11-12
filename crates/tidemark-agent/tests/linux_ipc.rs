#![cfg(target_os = "linux")]
use std::{
    fs,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    time::Duration,
};
use tidemark_agent::{
    linux::{resolve_runtime, send, Incoming, Limits, Server},
    Message,
};
fn setup() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}
fn message(kind: &str) -> Message {
    Message {
        kind: kind.into(),
        source: "test".into(),
        title: "plain".into(),
        text: "test".into(),
        options: vec!["no".into(), "yes".into()],
    }
}
fn next(s: &Server) -> Incoming {
    let start = std::time::Instant::now();
    loop {
        if let Some(e) = s.try_recv() {
            return e;
        }
        assert!(start.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn round_trip_and_validated_answers() {
    let d = setup();
    let s = Server::bind(d.path(), Limits::default()).unwrap();
    let p = d.path().to_owned();
    let t = std::thread::spawn(move || send(&p, &message("ask"), Duration::from_secs(2)).unwrap());
    match next(&s) {
        Incoming::Ask { reply, .. } => reply.send(Some(99)).unwrap(),
        _ => panic!(),
    }
    assert_eq!(t.join().unwrap(), None);
    let p = d.path().to_owned();
    let t = std::thread::spawn(move || send(&p, &message("ask"), Duration::from_secs(2)).unwrap());
    match next(&s) {
        Incoming::Ask { reply, .. } => reply.send(Some(1)).unwrap(),
        _ => panic!(),
    }
    assert_eq!(t.join().unwrap(), Some(1));
    send(d.path(), &message("notice"), Duration::from_secs(1)).unwrap();
    assert!(matches!(next(&s), Incoming::Notice(_)));
    assert_eq!(
        fs::metadata(d.path().join("tidemark/agent.sock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
#[test]
fn active_instance_and_regular_files_are_preserved() {
    let d = setup();
    let s = Server::bind(d.path(), Limits::default()).unwrap();
    assert!(Server::bind(d.path(), Limits::default()).is_err());
    drop(s);
    let p = d.path().join("tidemark/agent.sock");
    fs::write(&p, b"keep").unwrap();
    assert!(Server::bind(d.path(), Limits::default()).is_err());
    assert_eq!(fs::read(&p).unwrap(), b"keep");
}
#[test]
fn stale_owned_socket_recovers_and_drop_cleans_only_socket() {
    let d = setup();
    fs::create_dir(d.path().join("tidemark")).unwrap();
    fs::set_permissions(d.path().join("tidemark"), fs::Permissions::from_mode(0o700)).unwrap();
    let p = d.path().join("tidemark/agent.sock");
    drop(std::os::unix::net::UnixListener::bind(&p).unwrap());
    let s = Server::bind(d.path(), Limits::default()).unwrap();
    assert!(fs::metadata(&p).unwrap().file_type().is_socket());
    drop(s);
    assert!(!p.exists());
    assert!(d.path().join("tidemark").exists());
}
#[test]
fn rejects_public_relative_and_symlink_runtime() {
    let d = setup();
    assert!(
        resolve_runtime(Some(std::path::Path::new("relative")), unsafe {
            libc::geteuid()
        })
        .is_err()
    );
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Server::bind(d.path(), Limits::default()).is_err());
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let link = d.path().join("link");
    std::os::unix::fs::symlink(d.path(), &link).unwrap();
    assert!(Server::bind(&link, Limits::default()).is_err());
}
#[test]
fn timeout_disconnect_and_malformed_frames_do_not_approve() {
    use std::io::Write;
    let d = setup();
    let s = Server::bind(
        d.path(),
        Limits {
            request_timeout: Duration::from_millis(80),
            answer_timeout: Duration::from_millis(100),
            ..Limits::default()
        },
    )
    .unwrap();
    let p = d.path().to_owned();
    let t = std::thread::spawn(move || send(&p, &message("ask"), Duration::from_millis(80)));
    let _event = next(&s);
    assert!(t.join().unwrap().is_err());
    assert!(matches!(next(&s), Incoming::Gone(_)));
    let mut c =
        std::os::unix::net::UnixStream::connect(d.path().join("tidemark/agent.sock")).unwrap();
    c.write_all(b"{\"kind\":\"notice\"}").unwrap();
    drop(c);
    std::thread::sleep(Duration::from_millis(150));
    assert!(s.try_recv().is_none());
}

#[test]
fn peer_credentials_are_kernel_checked() {
    let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
    tidemark_agent::linux::verify_peer(&a).unwrap();
    tidemark_agent::linux::verify_peer(&b).unwrap();
}
#[test]
fn shutdown_interrupts_incomplete_clients_and_never_removes_replacements() {
    let d = setup();
    let s = Server::bind(d.path(), Limits::default()).unwrap();
    let _c = std::os::unix::net::UnixStream::connect(d.path().join("tidemark/agent.sock")).unwrap();
    let p = d.path().join("tidemark/agent.sock");
    fs::remove_file(&p).unwrap();
    fs::write(&p, b"replacement").unwrap();
    let start = std::time::Instant::now();
    drop(s);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(fs::read(&p).unwrap(), b"replacement");
}
#[test]
fn endpoint_and_subdirectory_symlinks_are_never_removed() {
    let d = setup();
    let target = d.path().join("keep");
    fs::write(&target, b"keep").unwrap();
    let sub = d.path().join("tidemark");
    fs::create_dir(&sub).unwrap();
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o700)).unwrap();
    let p = sub.join("agent.sock");
    std::os::unix::fs::symlink(&target, &p).unwrap();
    assert!(Server::bind(d.path(), Limits::default()).is_err());
    assert!(fs::symlink_metadata(&p).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(target).unwrap(), b"keep");
}
#[test]
fn client_rejects_out_of_range_and_oversize_reply() {
    use std::io::{Read, Write};
    for (reply, oversized) in [
        (b"{\"choice\":99}\n".to_vec(), false),
        (vec![b'x'; 4097], true),
    ] {
        let d = setup();
        let sub = d.path().join("tidemark");
        fs::create_dir(&sub).unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o700)).unwrap();
        let p = sub.join("agent.sock");
        let listener = std::os::unix::net::UnixListener::bind(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        let t = std::thread::spawn(move || {
            let (mut c, _) = listener.accept().unwrap();
            let mut buf = [0];
            while c.read(&mut buf).unwrap() == 1 && buf[0] != b'\n' {}
            let _ = c.write_all(&reply);
        });
        let result = send(d.path(), &message("ask"), Duration::from_secs(1));
        if oversized {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap(), None);
        }
        t.join().unwrap();
    }
}
