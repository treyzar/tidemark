//! Canonical private ScreenCast service. Never contacts a desktop or PipeWire server.
use super::*;
use std::{
    os::unix::net::UnixStream,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
static NEXT: AtomicUsize = AtomicUsize::new(1);
#[derive(Clone, Copy)]
enum Mode {
    Success,
    Cancel,
    SelectFailure,
    Timeout,
    WrongPath,
    WrongSender,
    ChangedOwner,
    RemoteFailure,
    Multiple,
    Closed,
    InvalidFd,
}
struct RequestFixture(Arc<AtomicUsize>);
#[zbus::interface(name = "org.freedesktop.portal.Request")]
impl RequestFixture {
    fn close(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct SessionFixture(Arc<AtomicUsize>);
#[zbus::interface(name = "org.freedesktop.portal.Session")]
impl SessionFixture {
    fn close(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct Fake {
    mode: Mode,
    service: String,
    other: Connection,
    session: Mutex<Option<String>>,
    requests: Arc<AtomicUsize>,
    sessions: Arc<AtomicUsize>,
    order: Arc<Mutex<Vec<&'static str>>>,
    peers: Mutex<Vec<UnixStream>>,
}
impl Fake {
    async fn response(
        &self,
        conn: &Connection,
        sender: &str,
        options: &Dict,
        stage: &str,
        results: HashMap<&str, Value<'_>>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let token = <&str>::try_from(options.get("handle_token").unwrap()).unwrap();
        let handle = path(sender, token, "request").unwrap();
        conn.object_server()
            .at(handle.as_str(), RequestFixture(self.requests.clone()))
            .await
            .unwrap();
        if !(stage == "Start" && matches!(self.mode, Mode::Timeout)) {
            let response = if stage == "CreateSession" && matches!(self.mode, Mode::Cancel) {
                1u32
            } else if stage == "SelectSources" && matches!(self.mode, Mode::SelectFailure) {
                2
            } else {
                0
            };
            let emitter = if stage == "Start" && matches!(self.mode, Mode::WrongSender) {
                &self.other
            } else {
                conn
            };
            emitter
                .emit_signal(
                    Some(sender),
                    handle.as_str(),
                    REQUEST,
                    "Response",
                    &(response, results),
                )
                .await
                .unwrap();
        }
        if stage == "Start" && matches!(self.mode, Mode::ChangedOwner) {
            conn.release_name(self.service.as_str()).await.unwrap();
            self.other
                .request_name(self.service.as_str())
                .await
                .unwrap();
        }
        OwnedObjectPath::try_from(
            if stage == "Start" && matches!(self.mode, Mode::WrongPath) {
                "/unexpected/request".into()
            } else {
                handle
            },
        )
        .map_err(|_| zbus::fdo::Error::Failed("fixture".into()))
    }
}
#[zbus::interface(name = "org.freedesktop.portal.ScreenCast")]
impl Fake {
    #[zbus(property)]
    fn available_source_types(&self) -> u32 {
        3
    }
    #[zbus(property)]
    fn available_cursor_modes(&self) -> u32 {
        3
    }
    async fn create_session(
        &self,
        options: Dict,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.order.lock().unwrap().push("CreateSession");
        let sender = header.sender().unwrap().as_str();
        let token = <&str>::try_from(options.get("session_handle_token").unwrap()).unwrap();
        let session = path(sender, token, "session").unwrap();
        *self.session.lock().unwrap() = Some(session.clone());
        conn.object_server()
            .at(session.as_str(), SessionFixture(self.sessions.clone()))
            .await
            .unwrap();
        self.response(
            conn,
            sender,
            &options,
            "CreateSession",
            HashMap::from([("session_handle", Value::from(session.as_str()))]),
        )
        .await
    }
    async fn select_sources(
        &self,
        session: OwnedObjectPath,
        options: Dict,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.order.lock().unwrap().push("SelectSources");
        assert_eq!(
            Some(session.as_str()),
            self.session.lock().unwrap().as_deref()
        );
        assert!(!bool::try_from(options.get("multiple").unwrap()).unwrap());
        assert_eq!(u32::try_from(options.get("types").unwrap()).unwrap(), 3);
        self.response(
            conn,
            header.sender().unwrap().as_str(),
            &options,
            "SelectSources",
            HashMap::new(),
        )
        .await
    }
    async fn start(
        &self,
        session: OwnedObjectPath,
        parent: &str,
        options: Dict,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.order.lock().unwrap().push("Start");
        assert_eq!(
            Some(session.as_str()),
            self.session.lock().unwrap().as_deref()
        );
        assert_eq!(parent, "wayland:synthetic");
        let mut streams = vec![(
            42u32,
            HashMap::from([
                ("source_type", Value::from(1u32)),
                ("pipewire-serial", Value::from(999u64)),
            ]),
        )];
        if matches!(self.mode, Mode::Multiple) {
            streams.push((43, HashMap::new()));
        }
        if matches!(self.mode, Mode::Closed) {
            conn.emit_signal(
                Some(header.sender().unwrap().as_str()),
                session.as_str(),
                SESSION,
                "Closed",
                &(HashMap::<&str, Value<'_>>::new(),),
            )
            .await
            .unwrap();
        }
        self.response(
            conn,
            header.sender().unwrap().as_str(),
            &options,
            "Start",
            HashMap::from([("streams", Value::from(streams))]),
        )
        .await
    }
    fn open_pipe_wire_remote(
        &self,
        session: OwnedObjectPath,
        _options: Dict,
    ) -> zbus::fdo::Result<zvariant::OwnedFd> {
        self.order.lock().unwrap().push("OpenPipeWireRemote");
        assert_eq!(
            Some(session.as_str()),
            self.session.lock().unwrap().as_deref()
        );
        if matches!(self.mode, Mode::RemoteFailure) {
            return Err(zbus::fdo::Error::Failed("fixture denied".into()));
        }
        if matches!(self.mode, Mode::InvalidFd) {
            let fd: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
            return Ok(fd.into());
        }
        let (socket, peer) = UnixStream::pair().unwrap();
        self.peers.lock().unwrap().push(peer);
        let fd: OwnedFd = socket.into();
        Ok(fd.into())
    }
}
struct Fixture {
    _server: Connection,
    _other: Connection,
    client: Client,
    requests: Arc<AtomicUsize>,
    sessions: Arc<AtomicUsize>,
    order: Arc<Mutex<Vec<&'static str>>>,
}
fn fixture(mode: Mode) -> Fixture {
    async_io::block_on(async {
        let service = format!(
            "org.tidemark.TestRecord{}",
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let server = Connection::session().await.unwrap();
        let other = Connection::session().await.unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let sessions = Arc::new(AtomicUsize::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));
        server
            .object_server()
            .at(
                DESKTOP,
                Fake {
                    mode,
                    service: service.clone(),
                    other: other.clone(),
                    session: Mutex::new(None),
                    requests: requests.clone(),
                    sessions: sessions.clone(),
                    order: order.clone(),
                    peers: Mutex::new(Vec::new()),
                },
            )
            .await
            .unwrap();
        server.request_name(service.as_str()).await.unwrap();
        let conn = zbus::connection::Builder::session()
            .unwrap()
            .method_timeout(Duration::from_millis(500))
            .build()
            .await
            .unwrap();
        Fixture {
            _server: server,
            _other: other,
            client: Client { conn, service },
            requests,
            sessions,
            order,
        }
    })
}
fn options() -> Options {
    Options {
        parent_window: "wayland:synthetic".into(),
        portal_timeout: Duration::from_millis(800),
        ..Options::default()
    }
}
#[test]
fn canonical_lifecycle_and_fd_transfer_are_verified() {
    let f = fixture(Mode::Success);
    let (mut session, fd) = async_io::block_on(f.client.prepare(&options())).unwrap();
    assert_eq!(session.node, 42);
    assert_eq!(session.serial, Some(999));
    assert!(fd.as_raw_fd() >= 0);
    session.health().unwrap();
    session.close().unwrap();
    assert_eq!(f.sessions.load(Ordering::SeqCst), 1);
    assert_eq!(
        *f.order.lock().unwrap(),
        vec![
            "CreateSession",
            "SelectSources",
            "Start",
            "OpenPipeWireRemote"
        ]
    );
    assert_eq!(f.requests.load(Ordering::SeqCst), 0);
}
#[test]
fn canceled_and_failed_lifecycle_close_only_owned_session() {
    for (mode, error) in [
        (Mode::Cancel, Error::Canceled),
        (Mode::SelectFailure, Error::Portal),
        (Mode::RemoteFailure, Error::Portal),
        (Mode::InvalidFd, Error::Portal),
        (Mode::Multiple, Error::Portal),
        (Mode::Closed, Error::Closed),
    ] {
        let f = fixture(mode);
        assert!(matches!(async_io::block_on(f.client.prepare(&options())),Err(e)if e==error));
        assert_eq!(f.sessions.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn timeout_wrong_path_sender_and_owner_fail_closed_with_cleanup() {
    for (mode, error) in [
        (Mode::Timeout, Error::Timeout),
        (Mode::WrongPath, Error::Path),
        (Mode::WrongSender, Error::Timeout),
        (Mode::ChangedOwner, Error::OwnerChanged),
    ] {
        let f = fixture(mode);
        assert!(matches!(async_io::block_on(f.client.prepare(&options())),Err(e)if e==error));
        assert_eq!(f.sessions.load(Ordering::SeqCst), 1);
        assert_eq!(f.requests.load(Ordering::SeqCst), 1);
    }
}
