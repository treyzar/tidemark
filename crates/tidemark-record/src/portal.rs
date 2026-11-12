//! Pinned-owner ScreenCast portal lifecycle. No persistent permissions are requested.
use crate::{Error, Options, Result};
use futures_lite::{future, StreamExt};
use std::{
    cell::RefCell,
    collections::HashMap,
    future::Future,
    os::fd::{AsRawFd, OwnedFd},
    time::Duration,
};
use zbus::{Connection, MatchRule, MessageStream, Proxy};
use zvariant::{OwnedObjectPath, OwnedValue, Value};
const SERVICE: &str = "org.freedesktop.portal.Desktop";
const DESKTOP: &str = "/org/freedesktop/portal/desktop";
const CAST: &str = "org.freedesktop.portal.ScreenCast";
const REQUEST: &str = "org.freedesktop.portal.Request";
const SESSION: &str = "org.freedesktop.portal.Session";
type Dict = HashMap<String, OwnedValue>;
pub(crate) async fn deadline<T>(wait: Duration, f: impl Future<Output = Result<T>>) -> Result<T> {
    future::or(f, async {
        async_io::Timer::after(wait).await;
        Err(Error::Timeout)
    })
    .await
}
#[derive(Clone)]
struct Client {
    conn: Connection,
    service: String,
}
pub(crate) struct Session {
    client: Client,
    owner: String,
    path: String,
    closed: MessageStream,
    active: bool,
    pub node: u32,
    pub serial: Option<u64>,
}
impl Session {
    pub fn open(options: &Options) -> Result<(Self, OwnedFd)> {
        async_io::block_on(async {
            let conn = deadline(Duration::from_secs(2), async {
                zbus::connection::Builder::session()
                    .map_err(|_| Error::Transport)?
                    .method_timeout(Duration::from_secs(2))
                    .build()
                    .await
                    .map_err(|_| Error::Transport)
            })
            .await?;
            Client {
                conn,
                service: SERVICE.into(),
            }
            .prepare(options)
            .await
        })
    }
    pub fn health(&mut self) -> Result<()> {
        async_io::block_on(deadline(Duration::from_millis(500), async {
            if let Some(message) = future::poll_once(self.closed.next()).await {
                match message {
                    Some(Ok(_)) => return Err(Error::Closed),
                    _ => return Err(Error::Transport),
                }
            }
            self.client.verify(&self.owner).await
        }))
    }
    pub fn close(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        async_io::block_on(self.client.close(&self.owner, &self.path, SESSION))
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
struct Cleanup {
    request: RefCell<Option<String>>,
    session: RefCell<Option<String>>,
}
impl Client {
    async fn bus(&self) -> Result<Proxy<'_>> {
        Proxy::new(
            &self.conn,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .await
        .map_err(|_| Error::Transport)
    }
    async fn owner(&self) -> Result<String> {
        self.bus()
            .await?
            .call("GetNameOwner", &(self.service.as_str(),))
            .await
            .map_err(|_| Error::OwnerChanged)
    }
    async fn verify(&self, owner: &str) -> Result<()> {
        if !owner.starts_with(':') || self.owner().await? != owner {
            Err(Error::OwnerChanged)
        } else {
            Ok(())
        }
    }
    async fn close(&self, owner: &str, path: &str, interface: &str) -> Result<()> {
        deadline(Duration::from_millis(500), async {
            self.conn
                .call_method(Some(owner), path, Some(interface), "Close", &())
                .await
                .map_err(|_| Error::Portal)?;
            Ok(())
        })
        .await
    }
    async fn prepare(&self, options: &Options) -> Result<(Session, OwnedFd)> {
        let owner = match self.owner().await {
            Ok(o) => o,
            Err(_) if self.service == SERVICE => {
                let _: u32 = self
                    .bus()
                    .await?
                    .call("StartServiceByName", &(SERVICE, 0u32))
                    .await
                    .map_err(|_| Error::Transport)?;
                self.owner().await?
            }
            Err(e) => return Err(e),
        };
        let cleanup = Cleanup {
            request: RefCell::new(None),
            session: RefCell::new(None),
        };
        let result = deadline(
            options.portal_timeout,
            self.configure(options, &owner, &cleanup),
        )
        .await;
        let request = cleanup.request.borrow_mut().take();
        if let Some(path) = request {
            let _ = self.close(&owner, &path, REQUEST).await;
        }
        if result.is_err() {
            let session = cleanup.session.borrow_mut().take();
            if let Some(path) = session {
                let _ = self.close(&owner, &path, SESSION).await;
            }
        }
        result
    }
    async fn configure(
        &self,
        options: &Options,
        owner: &str,
        cleanup: &Cleanup,
    ) -> Result<(Session, OwnedFd)> {
        self.verify(owner).await?;
        let proxy = Proxy::new(&self.conn, owner, DESKTOP, CAST)
            .await
            .map_err(|_| Error::Transport)?;
        let available: u32 = proxy
            .get_property("AvailableSourceTypes")
            .await
            .map_err(|_| Error::Portal)?;
        let types = available & options.sources.bits();
        if types == 0 {
            return Err(Error::Options);
        }
        if options.show_cursor {
            let modes: u32 = proxy
                .get_property("AvailableCursorModes")
                .await
                .map_err(|_| Error::Portal)?;
            if modes & 2 == 0 {
                return Err(Error::Options);
            }
        }
        let session_token = token()?;
        let session = path(
            self.conn.unique_name().ok_or(Error::Transport)?.as_str(),
            &session_token,
            "session",
        )?;
        *cleanup.session.borrow_mut() = Some(session.clone());
        let mut create = self
            .request(owner, Call::Create(&session_token), cleanup)
            .await?;
        // This historical portal field is s, intentionally not o.
        let returned = String::try_from(create.remove("session_handle").ok_or(Error::Path)?)
            .map_err(|_| Error::Path)?;
        if returned != session {
            return Err(Error::Path);
        }
        let rule = rule(owner, &session, SESSION, "Closed")?;
        let mut closed = MessageStream::for_match_rule(rule, &self.conn, Some(1))
            .await
            .map_err(|_| Error::Transport)?;
        let session_path = OwnedObjectPath::try_from(session.as_str()).map_err(|_| Error::Path)?;
        self.request(
            owner,
            Call::Select(&session_path, types, options.show_cursor),
            cleanup,
        )
        .await?;
        let mut started = self
            .request(
                owner,
                Call::Start(&session_path, &options.parent_window),
                cleanup,
            )
            .await?;
        let streams: Vec<(u32, Dict)> =
            Vec::try_from(started.remove("streams").ok_or(Error::Portal)?)
                .map_err(|_| Error::Portal)?;
        if streams.len() != 1 {
            return Err(Error::Portal);
        }
        let (node, mut properties) = streams.into_iter().next().ok_or(Error::Portal)?;
        if node == 0 || node == u32::MAX || properties.len() > 128 {
            return Err(Error::Portal);
        }
        if let Some(source) = properties.remove("source_type") {
            let source = u32::try_from(source).map_err(|_| Error::Portal)?;
            if !matches!(source, 1 | 2) || types & source == 0 {
                return Err(Error::Portal);
            }
        }
        let serial = properties
            .remove("pipewire-serial")
            .map(|s| u64::try_from(s).map_err(|_| Error::Portal))
            .transpose()?;
        if serial == Some(0) {
            return Err(Error::Portal);
        }
        self.verify(owner).await?;
        let remote: zvariant::OwnedFd = proxy
            .call(
                "OpenPipeWireRemote",
                &(&session_path, HashMap::<&str, Value<'_>>::new()),
            )
            .await
            .map_err(|_| Error::Portal)?;
        let fd: OwnedFd = remote.into();
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
        if flags < 0
            || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0
        {
            return Err(Error::Transport);
        }
        let mut socket_type: libc::c_int = 0;
        let mut len = std::mem::size_of_val(&socket_type) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut socket_type as *mut libc::c_int).cast(),
                &mut len,
            )
        } != 0
            || socket_type != libc::SOCK_STREAM
        {
            return Err(Error::Portal);
        }
        self.verify(owner).await?;
        // The portal can revoke a session even before OpenPipeWireRemote returns.
        if let Some(_message) = future::poll_once(closed.next()).await {
            return Err(Error::Closed);
        }
        cleanup.session.borrow_mut().take();
        Ok((
            Session {
                client: self.clone(),
                owner: owner.into(),
                path: session,
                closed,
                active: true,
                node,
                serial,
            },
            fd,
        ))
    }
    async fn request(&self, owner: &str, call: Call<'_>, cleanup: &Cleanup) -> Result<Dict> {
        let token = token()?;
        let path = path(
            self.conn.unique_name().ok_or(Error::Transport)?.as_str(),
            &token,
            "request",
        )?;
        let mut responses = MessageStream::for_match_rule(
            rule(owner, &path, REQUEST, "Response")?,
            &self.conn,
            Some(1),
        )
        .await
        .map_err(|_| Error::Transport)?;
        self.verify(owner).await?;
        *cleanup.request.borrow_mut() = Some(path.clone());
        let proxy = Proxy::new(&self.conn, owner, DESKTOP, CAST)
            .await
            .map_err(|_| Error::Transport)?;
        let mut opts = HashMap::from([("handle_token", Value::from(token.as_str()))]);
        let returned: OwnedObjectPath = match call {
            Call::Create(session_token) => {
                opts.insert("session_handle_token", Value::from(session_token));
                proxy.call("CreateSession", &(opts,)).await
            }
            Call::Select(session, types, cursor) => {
                opts.insert("types", Value::from(types));
                opts.insert("multiple", Value::from(false));
                if cursor {
                    opts.insert("cursor_mode", Value::from(2u32));
                }
                proxy.call("SelectSources", &(session, opts)).await
            }
            Call::Start(session, parent) => proxy.call("Start", &(session, parent, opts)).await,
        }
        .map_err(|_| Error::Portal)?;
        if returned.as_str() != path {
            return Err(Error::Path);
        }
        self.verify(owner).await?;
        let message = responses
            .next()
            .await
            .ok_or(Error::Transport)?
            .map_err(|_| Error::Transport)?;
        if message.header().sender().map(|s| s.as_str()) != Some(owner)
            || message.header().path().map(|p| p.as_str()) != Some(path.as_str())
        {
            return Err(Error::OwnerChanged);
        }
        self.verify(owner).await?;
        let (response, results): (u32, Dict) =
            message.body().deserialize().map_err(|_| Error::Portal)?;
        cleanup.request.borrow_mut().take();
        match response {
            0 => Ok(results),
            1 => Err(Error::Canceled),
            _ => Err(Error::Portal),
        }
    }
}
enum Call<'a> {
    Create(&'a str),
    Select(&'a OwnedObjectPath, u32, bool),
    Start(&'a OwnedObjectPath, &'a str),
}
fn rule(owner: &str, path: &str, interface: &str, member: &str) -> Result<MatchRule<'static>> {
    MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(owner)
        .map_err(|_| Error::OwnerChanged)?
        .path(path)
        .map_err(|_| Error::Path)?
        .interface(interface)
        .map_err(|_| Error::Transport)?
        .member(member)
        .map_err(|_| Error::Transport)
        .map(|b| b.build().to_owned())
}
fn path(sender: &str, token: &str, kind: &str) -> Result<String> {
    if !sender.starts_with(':')
        || sender.len() > 255
        || token.is_empty()
        || token.len() > 128
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(Error::Path);
    }
    let sender = sender.trim_start_matches(':').replace('.', "_");
    if sender.is_empty()
        || !sender
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(Error::Path);
    }
    Ok(format!("{DESKTOP}/{kind}/{sender}/{token}"))
}
fn token() -> Result<String> {
    let mut bytes = [0u8; 16];
    let mut offset = 0;
    for _ in 0..4 {
        let n = unsafe {
            libc::getrandom(
                bytes[offset..].as_mut_ptr().cast(),
                bytes.len() - offset,
                libc::GRND_NONBLOCK,
            )
        };
        if n > 0 {
            offset += n as usize;
            if offset == 16 {
                break;
            }
        } else if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return Err(Error::Transport);
        }
    }
    if offset != 16 {
        return Err(Error::Transport);
    }
    use std::fmt::Write;
    let mut out = "tidemark_record_".to_string();
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    Ok(out)
}

#[cfg(test)]
#[path = "portal_fixtures.rs"]
mod fixtures;
