//! Private-bus fixtures. They expose canonical AT-SPI interfaces, never a desktop.
use super::*;
use std::sync::{Arc, Mutex};
use zbus::blocking::connection::Builder;
#[derive(Default)]
struct State {
    liked: bool,
    actions: usize,
    spoof: bool,
    ambiguous: bool,
    remote_error: bool,
    no_effect: bool,
    changed_track: bool,
    change_after_action: bool,
    title_reads: usize,
    missing_uri: bool,
    indefinite: bool,
    cycle: bool,
}
struct Accessible {
    kind: &'static str,
    owner: String,
    state: Arc<Mutex<State>>,
}
impl Accessible {
    fn reference(&self, kind: &str) -> (String, OwnedObjectPath) {
        (
            self.owner.clone(),
            OwnedObjectPath::try_from(format!("/test/{kind}")).unwrap(),
        )
    }
}
#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl Accessible {
    #[zbus(property)]
    fn name(&self) -> String {
        let mut s = self.state.lock().unwrap();
        match self.kind {
            "doc" => "Яндекс Музыка".into(),
            "player" => "Плеер".into(),
            "title" => {
                s.title_reads += 1;
                if (s.changed_track && s.title_reads >= 2)
                    || (s.change_after_action && s.actions > 0)
                {
                    "Changed song".into()
                } else {
                    "Song".into()
                }
            }
            "artist" => "Artist —".into(),
            "like" | "like2" => "Нравится".into(),
            _ => "fixture".into(),
        }
    }
    fn get_children(&self) -> Vec<(String, OwnedObjectPath)> {
        let s = self.state.lock().unwrap();
        let kinds: Vec<&str> = match self.kind {
            "registry" => vec!["app"],
            "app" => {
                if s.cycle {
                    vec!["app"]
                } else {
                    vec!["doc"]
                }
            }
            "doc" => vec!["player"],
            "player" => {
                if s.ambiguous {
                    vec!["title", "artist", "like", "like2"]
                } else {
                    vec!["title", "artist", "like"]
                }
            }
            _ => vec![],
        };
        kinds.into_iter().map(|k| self.reference(k)).collect()
    }
    fn get_role(&self) -> u32 {
        match self.kind {
            "player" => 110,
            "title" | "artist" => 116,
            "like" | "like2" => 62,
            "doc" => 95,
            _ => 75,
        }
    }
    fn get_state(&self) -> Vec<u32> {
        let s = self.state.lock().unwrap();
        let mut word = (1 << 8) | (1 << 24) | (1 << 25) | (1 << 30);
        if matches!(self.kind, "like" | "like2") && s.liked {
            word |= 1 << 20;
        }
        vec![
            word,
            u32::from(s.indefinite && matches!(self.kind, "like" | "like2")),
        ]
    }
    fn get_interfaces(&self) -> Vec<String> {
        let mut interfaces = vec![ACCESSIBLE.into()];
        match self.kind {
            "doc" => interfaces.push(DOCUMENT.into()),
            "like" | "like2" => interfaces.push(ACTION.into()),
            _ => {}
        }
        interfaces
    }
}
struct Document(Arc<Mutex<State>>);
#[zbus::interface(name = "org.a11y.atspi.Document")]
impl Document {
    fn get_attribute_value(&self, name: &str) -> String {
        assert_eq!(name, "URI");
        if self.0.lock().unwrap().missing_uri {
            return String::new();
        }
        if self.0.lock().unwrap().spoof {
            "https://music.yandex.ru.evil.test/".into()
        } else {
            "https://music.yandex.ru/album/1".into()
        }
    }
}
struct Action(Arc<Mutex<State>>);
#[zbus::interface(name = "org.a11y.atspi.Action")]
impl Action {
    #[zbus(property)]
    fn n_actions(&self) -> i32 {
        1
    }
    fn do_action(&self, index: i32) -> zbus::fdo::Result<bool> {
        assert_eq!(index, 0);
        let mut s = self.0.lock().unwrap();
        if s.remote_error {
            return Err(zbus::fdo::Error::Failed("fixture rejected".into()));
        }
        s.actions += 1;
        if !s.no_effect {
            s.liked = !s.liked;
        }
        Ok(true)
    }
}
struct Bus(String);
#[zbus::interface(name = "org.a11y.Bus")]
impl Bus {
    fn get_address(&self) -> &str {
        &self.0
    }
}
struct Fixture {
    _conn: Connection,
    client: Client,
    state: Arc<Mutex<State>>,
}
impl Fixture {
    fn new(state: State) -> Self {
        let conn = Connection::session().unwrap();
        let owner = conn.unique_name().unwrap().to_string();
        let state = Arc::new(Mutex::new(state));
        conn.object_server()
            .at(
                ROOT,
                Accessible {
                    kind: "registry",
                    owner: owner.clone(),
                    state: state.clone(),
                },
            )
            .unwrap();
        for kind in ["app", "doc", "player", "title", "artist", "like", "like2"] {
            conn.object_server()
                .at(
                    format!("/test/{kind}"),
                    Accessible {
                        kind,
                        owner: owner.clone(),
                        state: state.clone(),
                    },
                )
                .unwrap();
        }
        conn.object_server()
            .at("/test/doc", Document(state.clone()))
            .unwrap();
        conn.object_server()
            .at("/test/like", Action(state.clone()))
            .unwrap();
        conn.object_server()
            .at("/test/like2", Action(state.clone()))
            .unwrap();
        let address =
            std::env::var("DBUS_SESSION_BUS_ADDRESS").expect("private dbus-run-session bus");
        conn.object_server()
            .at("/org/a11y/bus", Bus(address))
            .unwrap();
        let session = Builder::session()
            .unwrap()
            .method_timeout(CALL_TIMEOUT)
            .build()
            .unwrap();
        let mut client = Client::from_session(&session, &owner, &owner).unwrap();
        // Exercise real PID/UID/proc verification with the fixture executable. This
        // private field is inaccessible to applications; production stays allowlisted.
        client.browser_names = vec![std::env::current_exe()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()];
        Self {
            _conn: conn,
            client,
            state,
        }
    }
}
#[test]
fn private_bus_discovers_verifies_and_changes_once() {
    let f = Fixture::new(State::default());
    assert_eq!(f.client.liked("Song", "Artist"), Ok(false));
    assert_eq!(f.client.set_liked("Song", "Artist", true), Ok(true));
    assert_eq!(f.state.lock().unwrap().actions, 1);
    assert_eq!(f.client.set_liked("Song", "Artist", true), Ok(true));
    assert_eq!(f.state.lock().unwrap().actions, 1);
    assert_eq!(f.client.set_liked("Song", "Artist", false), Ok(false));
    assert_eq!(f.state.lock().unwrap().actions, 2);
}
#[test]
fn spoofed_host_never_reaches_action() {
    let f = Fixture::new(State {
        spoof: true,
        ..State::default()
    });
    assert_eq!(
        f.client.set_liked("Song", "Artist", true),
        Err(Error::Origin)
    );
    assert_eq!(f.state.lock().unwrap().actions, 0);
}
#[test]
fn changed_track_before_mutation_cancels_action() {
    let f = Fixture::new(State {
        changed_track: true,
        ..State::default()
    });
    assert_eq!(
        f.client.set_liked("Song", "Artist", true),
        Err(Error::Track)
    );
    assert_eq!(f.state.lock().unwrap().actions, 0);
}
#[test]
fn ambiguous_like_button_never_mutates() {
    let f = Fixture::new(State {
        ambiguous: true,
        ..State::default()
    });
    assert_eq!(
        f.client.set_liked("Song", "Artist", true),
        Err(Error::Button)
    );
    assert_eq!(f.state.lock().unwrap().actions, 0);
}
#[test]
fn remote_action_rejection_cannot_report_success() {
    let f = Fixture::new(State {
        remote_error: true,
        ..State::default()
    });
    assert_eq!(
        f.client.set_liked("Song", "Artist", true),
        Err(Error::Action)
    );
    assert!(!f.state.lock().unwrap().liked);
}
#[test]
fn post_action_track_change_cannot_report_success() {
    let f = Fixture::new(State {
        change_after_action: true,
        ..State::default()
    });
    assert_eq!(
        f.client.set_liked("Song", "Artist", true),
        Err(Error::Track)
    );
    assert_eq!(f.state.lock().unwrap().actions, 1);
}
#[test]
fn ack_without_state_change_cannot_report_success() {
    let f = Fixture::new(State {
        no_effect: true,
        ..State::default()
    });
    assert!(matches!(
        f.client.set_liked("Song", "Artist", true),
        Err(Error::Limit | Error::NotConfirmed)
    ));
    assert_eq!(f.state.lock().unwrap().actions, 1);
}
#[test]
fn ordinary_process_cannot_masquerade_as_a_browser() {
    let mut f = Fixture::new(State::default());
    f.client.browser_names = BROWSERS.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        f.client.set_liked("Song", "Artist", true),
        Err(Error::Origin)
    );
    assert_eq!(f.state.lock().unwrap().actions, 0);
}

#[test]
fn missing_uri_cannot_be_replaced_by_branded_document_title() {
    let f = Fixture::new(State {
        missing_uri: true,
        ..State::default()
    });
    assert_eq!(
        f.client.set_liked("Song", "Artist", true),
        Err(Error::Origin)
    );
    assert_eq!(f.state.lock().unwrap().actions, 0);
}
#[test]
fn indeterminate_state_never_mutates() {
    let f = Fixture::new(State {
        indefinite: true,
        ..State::default()
    });
    assert_eq!(
        f.client.set_liked("Song", "Artist", true),
        Err(Error::Button)
    );
    assert_eq!(f.state.lock().unwrap().actions, 0);
}
#[test]
fn malformed_cyclic_tree_fails_with_a_bound() {
    let f = Fixture::new(State {
        cycle: true,
        ..State::default()
    });
    assert_eq!(
        f.client.set_liked("Song", "Artist", true),
        Err(Error::Limit)
    );
    assert_eq!(f.state.lock().unwrap().actions, 0);
}
#[test]
fn captured_registry_owner_is_checked_again_before_queries() {
    let mut f = Fixture::new(State::default());
    f.client.registry.owner = ":999.999".into();
    assert_eq!(
        f.client.set_liked("Song", "Artist", true),
        Err(Error::OwnerChanged)
    );
    assert_eq!(f.state.lock().unwrap().actions, 0);
}

#[test]
fn cancellation_guard_blocks_the_only_mutating_call() {
    let f = Fixture::new(State::default());
    let mut checked = 0;
    assert_eq!(
        f.client.set_liked_with_guard("Song", "Artist", true, || {
            checked += 1;
            false
        }),
        Err(Error::Changed)
    );
    assert_eq!(checked, 1);
    let state = f.state.lock().unwrap();
    assert!(
        state.title_reads > 0,
        "guard runs after final evidence collection"
    );
    assert_eq!(state.actions, 0);
    assert!(!state.liked);
}
