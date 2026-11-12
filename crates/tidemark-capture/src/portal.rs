//! Screenshot portal. Every call is explicit and blocking: use a UI worker.
use crate::{raster::local_image_uri, Error, Raster, Result};
use futures_lite::StreamExt;
use std::{cell::RefCell, collections::HashMap, future::Future, path::PathBuf, time::Duration};
use zbus::{Connection, MatchRule, MessageStream, Proxy};
use zvariant::{OwnedObjectPath, OwnedValue, Value};
const SERVICE: &str = "org.freedesktop.portal.Desktop";
const DESKTOP: &str = "/org/freedesktop/portal/desktop";
const SCREENSHOT: &str = "org.freedesktop.portal.Screenshot";
const REQUEST: &str = "org.freedesktop.portal.Request";
#[derive(Clone)]
pub struct Client {
    conn: Connection,
    service: String,
}
#[derive(Clone, Debug)]
pub struct ScreenshotOptions {
    pub parent_window: String,
    pub interactive: bool,
    pub timeout: Duration,
}
impl Default for ScreenshotOptions {
    fn default() -> Self {
        Self {
            parent_window: String::new(),
            interactive: true,
            timeout: Duration::from_secs(60),
        }
    }
}
/// The portal's local file result. No file is opened or deleted implicitly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screenshot {
    pub path: PathBuf,
}
impl Screenshot {
    pub fn load(&self) -> Result<Raster> {
        Raster::load(&self.path)
    }
}
async fn deadline<T>(wait: Duration, future: impl Future<Output = Result<T>>) -> Result<T> {
    futures_lite::future::or(future, async {
        async_io::Timer::after(wait).await;
        Err(Error::Timeout)
    })
    .await
}
impl Client {
    pub fn connect() -> Result<Self> {
        async_io::block_on(deadline(Duration::from_secs(2), async {
            let conn = zbus::connection::Builder::session()
                .map_err(|_| Error::Transport)?
                .method_timeout(Duration::from_secs(2))
                .build()
                .await
                .map_err(|_| Error::Transport)?;
            Ok(Self {
                conn,
                service: SERVICE.into(),
            })
        }))
    }
    /// Success means the portal confirmed a screenshot URI, not that an editor
    /// is open. Cancellation is Err(Canceled). No fallback silently captures.
    pub fn screenshot(&self, options: &ScreenshotOptions) -> Result<Screenshot> {
        if options.timeout.is_zero() {
            return Err(Error::Timeout);
        }
        if options.parent_window.len() > 1024
            || options.parent_window.chars().any(char::is_control)
            || !(options.parent_window.is_empty()
                || options.parent_window.starts_with("x11:")
                || options.parent_window.starts_with("wayland:"))
        {
            return Err(Error::Portal);
        }
        let pending = RefCell::new(None::<(String, String)>);
        async_io::block_on(async {
            let result = deadline(
                options.timeout.min(Duration::from_secs(120)),
                self.request(options, &pending),
            )
            .await;
            if result.is_err() {
                let to_close = pending.borrow_mut().take();
                if let Some((owner, path)) = to_close {
                    // A timed-out permission dialog must not linger indefinitely.
                    // Closing the exact pinned request is itself bounded.
                    let _ = deadline(Duration::from_millis(500), async {
                        self.conn
                            .call_method(
                                Some(owner.as_str()),
                                path.as_str(),
                                Some(REQUEST),
                                "Close",
                                &(),
                            )
                            .await
                            .map_err(|_| Error::Transport)?;
                        Ok(())
                    })
                    .await;
                }
            }
            result
        })
    }
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
    async fn verify_owner(&self, owner: &str) -> Result<()> {
        if !owner.starts_with(':') || self.owner().await? != owner {
            Err(Error::OwnerChanged)
        } else {
            Ok(())
        }
    }
    async fn request(
        &self,
        options: &ScreenshotOptions,
        pending: &RefCell<Option<(String, String)>>,
    ) -> Result<Screenshot> {
        let owner = match self.owner().await {
            Ok(owner) => owner,
            Err(_) if self.service == SERVICE => {
                let _: u32 = self
                    .bus()
                    .await?
                    .call("StartServiceByName", &(SERVICE, 0u32))
                    .await
                    .map_err(|_| Error::Transport)?;
                self.owner().await?
            }
            Err(error) => return Err(error),
        };
        self.verify_owner(&owner).await?;
        let token = random_token()?;
        let sender = self.conn.unique_name().ok_or(Error::Transport)?.as_str();
        let path = request_path(sender, &token)?;
        let rule = MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(owner.as_str())
            .map_err(|_| Error::OwnerChanged)?
            .path(path.as_str())
            .map_err(|_| Error::RequestPath)?
            .interface(REQUEST)
            .map_err(|_| Error::Transport)?
            .member("Response")
            .map_err(|_| Error::Transport)?
            .build();
        // Register with the bus before Screenshot; even a signal emitted before
        // its method return is retained in this bounded, specifically matched slot.
        let mut responses = MessageStream::for_match_rule(rule, &self.conn, Some(1))
            .await
            .map_err(|_| Error::Transport)?;
        self.verify_owner(&owner).await?;
        *pending.borrow_mut() = Some((owner.clone(), path.clone()));
        let proxy = Proxy::new(&self.conn, owner.as_str(), DESKTOP, SCREENSHOT)
            .await
            .map_err(|_| Error::Transport)?;
        let parent_window = options.parent_window.as_str();
        let options = HashMap::from([
            ("handle_token", Value::from(token.as_str())),
            ("interactive", Value::from(options.interactive)),
            ("modal", Value::from(true)),
        ]);
        let returned: OwnedObjectPath = proxy
            .call("Screenshot", &(parent_window, options))
            .await
            .map_err(|_| Error::Portal)?;
        if returned.as_str() != path {
            return Err(Error::RequestPath);
        }
        self.verify_owner(&owner).await?;
        let message = responses
            .next()
            .await
            .ok_or(Error::Transport)?
            .map_err(|_| Error::Transport)?;
        let header = message.header();
        if header.sender().map(|s| s.as_str()) != Some(owner.as_str()) {
            return Err(Error::OwnerChanged);
        }
        if header.path().map(|p| p.as_str()) != Some(path.as_str()) {
            return Err(Error::RequestPath);
        }
        self.verify_owner(&owner).await?;
        let (response, mut results): (u32, HashMap<String, OwnedValue>) =
            message.body().deserialize().map_err(|_| Error::Portal)?;
        match response {
            0 => {
                let uri = String::try_from(results.remove("uri").ok_or(Error::Image)?)
                    .map_err(|_| Error::Image)?;
                Ok(Screenshot {
                    path: local_image_uri(&uri)?,
                })
            }
            1 => Err(Error::Canceled),
            _ => Err(Error::Portal),
        }
    }
}
fn request_path(sender: &str, token: &str) -> Result<String> {
    if !sender.starts_with(':')
        || sender.len() > 255
        || token.is_empty()
        || token.len() > 128
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(Error::RequestPath);
    }
    let sender = sender.trim_start_matches(':').replace('.', "_");
    if !sender
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(Error::RequestPath);
    }
    Ok(format!("{DESKTOP}/request/{sender}/{token}"))
}
fn random_token() -> Result<String> {
    let mut bytes = [0u8; 16];
    let mut offset = 0;
    for _ in 0..4 {
        let got = unsafe {
            libc::getrandom(
                bytes[offset..].as_mut_ptr().cast(),
                bytes.len() - offset,
                libc::GRND_NONBLOCK,
            )
        };
        if got > 0 {
            offset += got as usize;
            if offset == bytes.len() {
                break;
            }
        } else if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return Err(Error::Transport);
        }
    }
    if offset != bytes.len() {
        return Err(Error::Transport);
    }
    use std::fmt::Write;
    let mut token = "tidemark_capture_".to_string();
    for byte in bytes {
        let _ = write!(token, "{byte:02x}");
    }
    Ok(token)
}
#[cfg(test)]
#[path = "portal_fixtures.rs"]
mod fixtures;
