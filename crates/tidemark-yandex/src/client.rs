//! Blocking AT-SPI2 adapter. Create and call from a bounded UI worker, never GTK.
use crate::{
    artist_name, has_state, like_state, metadata_matches, normalized, trusted_music_url, Error,
    Evidence, Object, Result,
};
use std::{
    collections::{HashSet, VecDeque},
    time::{Duration, Instant},
};
use zbus::blocking::{Connection, Proxy};
use zvariant::OwnedObjectPath;
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const DOCUMENT: &str = "org.a11y.atspi.Document";
const ACTION: &str = "org.a11y.atspi.Action";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const REGISTRY: &str = "org.a11y.atspi.Registry";
const CALL_TIMEOUT: Duration = Duration::from_millis(700);
const OPERATION_TIMEOUT: Duration = Duration::from_secs(12);
const BROWSERS: &[&str] = &[
    "chrome",
    "chromium",
    "chromium-browser",
    "google-chrome",
    "firefox",
    "firefox-bin",
    "brave",
    "brave-browser",
    "browser",
    "yandex-browser",
    "yandex_browser",
    "opera",
    "vivaldi",
    "vivaldi-bin",
    "msedge",
];
const MAX_NODES: usize = 4096;
const MAX_DEPTH: usize = 64;
const MAX_CHILDREN: usize = 256;
fn connect_bounded(builder: zbus::connection::Builder<'_>) -> Result<Connection> {
    async_io::block_on(futures_lite::future::or(
        async move {
            builder
                .method_timeout(CALL_TIMEOUT)
                .build()
                .await
                .map(Connection::from)
                .map_err(|_| Error::Transport)
        },
        async {
            async_io::Timer::after(Duration::from_secs(2)).await;
            Err(Error::Transport)
        },
    ))
}
struct Budget {
    deadline: Instant,
    calls: usize,
}
impl Budget {
    fn check(&mut self) -> Result<()> {
        self.calls += 1;
        if self.calls > 12000 || Instant::now() >= self.deadline {
            Err(Error::Limit)
        } else {
            Ok(())
        }
    }
}
#[derive(Clone)]
struct Node {
    object: Object,
    name: String,
    role: u32,
    states: Vec<u32>,
    interfaces: Vec<String>,
}
fn visible(n: &Node) -> bool {
    n.states.len() == 2
        && !has_state(&n.states, 6)
        && !has_state(&n.states, 27)
        && has_state(&n.states, 25)
        && has_state(&n.states, 30)
}
pub struct Client {
    conn: Connection,
    registry: Object,
    registry_name: String,
    browser_names: Vec<String>,
}
impl Client {
    /// Resolve the dedicated AT-SPI bus through org.a11y.Bus, without changing
    /// accessibility settings, selecting browser tabs, or raising windows.
    pub fn connect() -> Result<Self> {
        let session =
            connect_bounded(zbus::connection::Builder::session().map_err(|_| Error::Transport)?)?;
        Self::from_session(&session, "org.a11y.Bus", REGISTRY)
    }
    fn from_session(session: &Connection, discovery: &str, registry: &str) -> Result<Self> {
        let bus = Proxy::new(
            session,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .map_err(|_| Error::Transport)?;
        let owner: String = bus
            .call("GetNameOwner", &(discovery,))
            .map_err(|_| Error::OwnerChanged)?;
        let proxy = Proxy::new(session, owner.as_str(), "/org/a11y/bus", "org.a11y.Bus")
            .map_err(|_| Error::Transport)?;
        let address: String = proxy
            .call("GetAddress", &())
            .map_err(|_| Error::Transport)?;
        // Never connect to remote TCP addresses supplied by an unexpected bus provider.
        if !address.starts_with("unix:") || address.contains(';') {
            return Err(Error::Transport);
        }
        let conn = connect_bounded(
            zbus::connection::Builder::address(address.as_str()).map_err(|_| Error::Transport)?,
        )?;
        Self::on_bus(conn, registry)
    }
    fn on_bus(conn: Connection, registry_name: &str) -> Result<Self> {
        let bus = Proxy::new(
            &conn,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .map_err(|_| Error::Transport)?;
        let owner: String = bus
            .call("GetNameOwner", &(registry_name,))
            .map_err(|_| Error::OwnerChanged)?;
        Ok(Self {
            conn,
            registry_name: registry_name.into(),
            registry: Object {
                owner,
                path: ROOT.into(),
            },
            browser_names: BROWSERS.iter().map(|s| s.to_string()).collect(),
        })
    }
    /// Changes a favorite only after complete origin, track, uniqueness and
    /// state checks. Never retries the mutating action on another candidate.
    /// Ok means the observed accessibility state was confirmed after the action.
    pub fn set_liked(&self, title: &str, artist: &str, want: bool) -> Result<bool> {
        self.set_liked_with_guard(title, artist, want, || true)
    }
    /// Re-check cancellation after the final evidence/owner checks, immediately
    /// before the sole mutating call. A false guard never invokes DoAction.
    pub fn set_liked_with_guard(
        &self,
        title: &str,
        artist: &str,
        want: bool,
        mut guard: impl FnMut() -> bool,
    ) -> Result<bool> {
        if normalized(title).is_empty() || artist_name(artist).is_empty() {
            return Err(Error::Track);
        }
        let mut b = Budget {
            deadline: Instant::now() + OPERATION_TIMEOUT,
            calls: 0,
        };
        let first = self.evidence(title, artist, &mut b)?;
        let second = self.evidence(title, artist, &mut b)?;
        if first != second {
            return Err(Error::Changed);
        }
        if second.liked != want {
            let action = self.proxy(&second.button, ACTION, &mut b)?;
            let count: i32 = action.get_property("NActions").map_err(|_| Error::Action)?;
            if count != 1 {
                return Err(Error::Button);
            }
            // Check metadata once more after querying the action interface.
            if self.evidence(title, artist, &mut b)? != second {
                return Err(Error::Changed);
            }
            self.verify_owner(&second.button.owner, &mut b)?;
            let action = self.proxy(&second.button, ACTION, &mut b)?;
            if !guard() {
                return Err(Error::Changed);
            }
            b.check()?;
            let applied: bool = action
                .call("DoAction", &(0i32,))
                .map_err(|_| Error::Action)?;
            if !applied {
                return Err(Error::Action);
            }
        }
        b.deadline = b.deadline.min(Instant::now() + Duration::from_secs(2));
        loop {
            let actual = self.evidence(title, artist, &mut b)?;
            if actual.document != second.document
                || actual.player != second.player
                || actual.button != second.button
                || actual.url != second.url
            {
                return Err(Error::Changed);
            }
            if actual.liked == want {
                return Ok(want);
            }
            if Instant::now() + Duration::from_millis(100) >= b.deadline {
                return Err(Error::NotConfirmed);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    /// Read-only verified state. As with set_liked, call outside the UI thread.
    pub fn liked(&self, title: &str, artist: &str) -> Result<bool> {
        let mut b = Budget {
            deadline: Instant::now() + OPERATION_TIMEOUT,
            calls: 0,
        };
        Ok(self.evidence(title, artist, &mut b)?.liked)
    }
    fn bus(&self) -> Result<Proxy<'_>> {
        Proxy::new(
            &self.conn,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .map_err(|_| Error::Transport)
    }
    fn verify_owner(&self, owner: &str, b: &mut Budget) -> Result<()> {
        b.check()?;
        if !owner.starts_with(':') {
            return Err(Error::OwnerChanged);
        }
        let current: String = self
            .bus()?
            .call("GetNameOwner", &(owner,))
            .map_err(|_| Error::OwnerChanged)?;
        if current != owner {
            Err(Error::OwnerChanged)
        } else {
            Ok(())
        }
    }
    fn proxy<'a>(
        &'a self,
        obj: &'a Object,
        iface: &'static str,
        b: &mut Budget,
    ) -> Result<Proxy<'a>> {
        b.check()?;
        Proxy::new(&self.conn, obj.owner.as_str(), obj.path.as_str(), iface)
            .map_err(|_| Error::Transport)
    }
    fn children(&self, obj: &Object, b: &mut Budget) -> Result<Vec<Object>> {
        let children: Vec<(String, OwnedObjectPath)> = self
            .proxy(obj, ACCESSIBLE, b)?
            .call("GetChildren", &())
            .map_err(|_| Error::Transport)?;
        if children.len() > MAX_CHILDREN {
            return Err(Error::Limit);
        }
        children
            .into_iter()
            .map(|(owner, path)| {
                if !owner.starts_with(':') || path.as_str() == "/org/a11y/atspi/null" {
                    return Err(Error::OwnerChanged);
                }
                Ok(Object {
                    owner,
                    path: path.to_string(),
                })
            })
            .collect()
    }
    fn node(&self, obj: &Object, b: &mut Budget) -> Result<Node> {
        let p = self.proxy(obj, ACCESSIBLE, b)?;
        let name: String = p.get_property("Name").map_err(|_| Error::Transport)?;
        if name.len() > 8192 {
            return Err(Error::Limit);
        }
        b.check()?;
        let role: u32 = p.call("GetRole", &()).map_err(|_| Error::Transport)?;
        b.check()?;
        let states: Vec<u32> = p.call("GetState", &()).map_err(|_| Error::Transport)?;
        b.check()?;
        let interfaces: Vec<String> = p.call("GetInterfaces", &()).map_err(|_| Error::Transport)?;
        if states.len() != 2 || interfaces.len() > 32 {
            return Err(Error::Limit);
        }
        Ok(Node {
            object: obj.clone(),
            name,
            role,
            states,
            interfaces,
        })
    }
    fn browser(&self, owner: &str, b: &mut Budget) -> Result<bool> {
        self.verify_owner(owner, b)?;
        b.check()?;
        let user: u32 = self
            .bus()?
            .call("GetConnectionUnixUser", &(owner,))
            .map_err(|_| Error::Transport)?;
        if user != unsafe { libc::geteuid() } {
            return Err(Error::OwnerChanged);
        }
        b.check()?;
        let pid: u32 = self
            .bus()?
            .call("GetConnectionUnixProcessID", &(owner,))
            .map_err(|_| Error::Transport)?;
        let exe = std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|_| Error::Transport)?;
        let name = exe
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(Error::Transport)?;
        Ok(self.browser_names.iter().any(|allowed| allowed == name))
    }
    fn walk(&self, root: &Object, b: &mut Budget, stop_at_documents: bool) -> Result<Vec<Node>> {
        let mut pending = VecDeque::from([(root.clone(), 0usize)]);
        let mut seen = HashSet::new();
        let mut nodes = Vec::new();
        while let Some((obj, depth)) = pending.pop_front() {
            if obj.owner != root.owner {
                return Err(Error::OwnerChanged);
            }
            if !seen.insert(obj.clone()) {
                return Err(Error::Limit);
            }
            if depth > MAX_DEPTH || seen.len() > MAX_NODES {
                return Err(Error::Limit);
            }
            let n = self.node(&obj, b)?;
            let document = n.interfaces.iter().any(|s| s == DOCUMENT);
            nodes.push(n);
            if !(stop_at_documents && document && obj != *root) {
                for child in self.children(&obj, b)? {
                    pending.push_back((child, depth + 1));
                    if pending.len() > MAX_NODES {
                        return Err(Error::Limit);
                    }
                }
            }
        }
        Ok(nodes)
    }
    fn uri(&self, doc: &Object, b: &mut Budget) -> Result<String> {
        let url: String = self
            .proxy(doc, DOCUMENT, b)?
            .call("GetAttributeValue", &("URI",))
            .map_err(|_| Error::Origin)?;
        if url.len() > 8192 || !trusted_music_url(&url) {
            return Err(Error::Origin);
        }
        Ok(url)
    }
    fn evidence(&self, title: &str, artist: &str, b: &mut Budget) -> Result<Evidence> {
        b.check()?;
        let registry_owner: String = self
            .bus()?
            .call("GetNameOwner", &(self.registry_name.as_str(),))
            .map_err(|_| Error::OwnerChanged)?;
        if registry_owner != self.registry.owner {
            return Err(Error::OwnerChanged);
        }
        self.verify_owner(&self.registry.owner, b)?;
        let apps = self.children(&self.registry, b)?;
        if apps.len() > 64 {
            return Err(Error::Limit);
        }
        let mut documents = Vec::new();
        for app in apps {
            if !self.browser(&app.owner, b)? {
                continue;
            }
            for n in self.walk(&app, b, true)? {
                if !visible(&n) || !n.interfaces.iter().any(|s| s == DOCUMENT) {
                    continue;
                }
                // URI is provided by the browser's document interface, never a
                // page-authored input, text label, or title mentioning the host.
                if let Ok(url) = self.uri(&n.object, b) {
                    documents.push((n.object, url));
                }
            }
        }
        if documents.len() != 1 {
            return Err(Error::Origin);
        }
        let (doc, url) = documents.pop().unwrap();
        let mut players = Vec::new();
        for n in self.walk(&doc, b, true)? {
            if n.name != "Плеер" || !matches!(n.role, 39 | 72 | 110) || !visible(&n) {
                continue;
            }
            let contents = self.walk(&n.object, b, true)?;
            let names: Vec<String> = contents
                .iter()
                .filter(|n| matches!(n.role, 29 | 61 | 116) && visible(n))
                .map(|n| n.name.clone())
                .collect();
            if metadata_matches(&names, title, artist) {
                players.push((n.object, contents));
            }
        }
        if players.len() != 1 {
            return Err(Error::Track);
        }
        let (player, contents) = players.pop().unwrap();
        let mut buttons: Vec<Node> = contents
            .into_iter()
            .filter(|n| n.name == "Нравится" && matches!(n.role, 7 | 43 | 62))
            .collect();
        if buttons.len() != 1 {
            return Err(Error::Button);
        }
        let button = buttons.pop().unwrap();
        if !button.interfaces.iter().any(|s| s == ACTION) {
            return Err(Error::Button);
        }
        let liked = like_state(button.role, &button.states)?;
        self.verify_owner(&button.object.owner, b)?;
        if self.uri(&doc, b)? != url {
            return Err(Error::Changed);
        }
        Ok(Evidence {
            document: doc,
            player,
            button: button.object,
            url,
            liked,
        })
    }
}

#[cfg(test)]
#[path = "fixtures.rs"]
mod fixtures;
