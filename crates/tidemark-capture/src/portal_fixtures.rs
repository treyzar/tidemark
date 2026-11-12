//! Canonical private-bus fixtures; never contact the real desktop portal.
use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
static NEXT: AtomicUsize = AtomicUsize::new(1);
#[derive(Clone, Copy)]
enum Mode {
    Success,
    Cancel,
    Failure,
    Timeout,
    WrongPath,
    WrongSender,
    ChangedOwner,
    RemoteUri,
}
struct RequestFixture(Arc<AtomicUsize>);
#[zbus::interface(name = "org.freedesktop.portal.Request")]
impl RequestFixture {
    fn close(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct PortalFixture {
    mode: Mode,
    closes: Arc<AtomicUsize>,
    service: String,
    other: Connection,
}
#[zbus::interface(name = "org.freedesktop.portal.Screenshot")]
impl PortalFixture {
    async fn screenshot(
        &self,
        parent: &str,
        options: HashMap<String, OwnedValue>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        assert_eq!(parent, "wayland:synthetic-handle");
        assert!(bool::try_from(options.get("interactive").unwrap()).unwrap());
        let token = <&str>::try_from(options.get("handle_token").unwrap()).unwrap();
        let sender = header.sender().unwrap().as_str();
        let path = request_path(sender, token).unwrap();
        conn.object_server()
            .at(path.as_str(), RequestFixture(self.closes.clone()))
            .await
            .unwrap();
        let mut results = HashMap::<String, Value<'_>>::new();
        let response = match self.mode {
            Mode::Cancel => 1u32,
            Mode::Failure => 2,
            _ => 0,
        };
        if response == 0 {
            results.insert(
                "uri".into(),
                Value::from(if matches!(self.mode, Mode::RemoteUri) {
                    "https://example.invalid/private.png"
                } else {
                    "file:///tmp/tidemark-synthetic-portal.png"
                }),
            );
        }
        if !matches!(self.mode, Mode::Timeout) {
            let emitter = if matches!(self.mode, Mode::WrongSender) {
                &self.other
            } else {
                conn
            };
            // Emit before the method reply: clients subscribing afterward lose it.
            emitter
                .emit_signal(
                    Some(sender),
                    path.as_str(),
                    REQUEST,
                    "Response",
                    &(response, results),
                )
                .await
                .unwrap();
        }
        if matches!(self.mode, Mode::ChangedOwner) {
            conn.release_name(self.service.as_str()).await.unwrap();
            self.other
                .request_name(self.service.as_str())
                .await
                .unwrap();
        }
        OwnedObjectPath::try_from(if matches!(self.mode, Mode::WrongPath) {
            "/unexpected/request".into()
        } else {
            path
        })
        .map_err(|_| zbus::fdo::Error::Failed("fixture path".into()))
    }
}
fn fixture(mode: Mode) -> (Connection, Connection, Client, Arc<AtomicUsize>) {
    async_io::block_on(async {
        let service = format!(
            "org.tidemark.TestCapture{}",
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let server = Connection::session().await.unwrap();
        let other = Connection::session().await.unwrap();
        let closes = Arc::new(AtomicUsize::new(0));
        server
            .object_server()
            .at(
                DESKTOP,
                PortalFixture {
                    mode,
                    closes: closes.clone(),
                    service: service.clone(),
                    other: other.clone(),
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
        (server, other, Client { conn, service }, closes)
    })
}
fn options() -> ScreenshotOptions {
    ScreenshotOptions {
        parent_window: "wayland:synthetic-handle".into(),
        interactive: true,
        timeout: Duration::from_millis(500),
    }
}
#[test]
fn response_before_method_return_is_retained() {
    let (_server, _other, client, _) = fixture(Mode::Success);
    assert_eq!(
        client.screenshot(&options()).unwrap().path,
        PathBuf::from("/tmp/tidemark-synthetic-portal.png")
    );
}
#[test]
fn cancellation_failure_and_remote_uri_are_truthful() {
    for (mode, error) in [
        (Mode::Cancel, Error::Canceled),
        (Mode::Failure, Error::Portal),
        (Mode::RemoteUri, Error::Image),
    ] {
        let (_server, _other, client, _) = fixture(mode);
        assert_eq!(client.screenshot(&options()), Err(error));
    }
}
#[test]
fn timeout_closes_exact_request() {
    let (_server, _other, client, closes) = fixture(Mode::Timeout);
    assert_eq!(client.screenshot(&options()), Err(Error::Timeout));
    assert_eq!(closes.load(Ordering::SeqCst), 1);
}
#[test]
fn returned_path_and_owner_are_pinned() {
    for (mode, error) in [
        (Mode::WrongPath, Error::RequestPath),
        (Mode::ChangedOwner, Error::OwnerChanged),
        (Mode::WrongSender, Error::Timeout),
    ] {
        let (_server, _other, client, _) = fixture(mode);
        assert_eq!(client.screenshot(&options()), Err(error));
    }
}
#[test]
fn request_paths_follow_sender_transform_and_reject_invalid_tokens() {
    assert_eq!(
        request_path(":1.42", "tidemark_abc").unwrap(),
        "/org/freedesktop/portal/desktop/request/1_42/tidemark_abc"
    );
    assert_eq!(request_path(":1.42", "bad/token"), Err(Error::RequestPath));
}
