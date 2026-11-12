//! Bounded card queue and the retained, same-user agent socket server.
//! All content is untrusted plain text. Only an explicit button returns an
//! option index; dismissal, queue pressure, expiry and shutdown return None.
use std::{
    cell::RefCell,
    collections::VecDeque,
    rc::Rc,
    sync::mpsc::Sender,
    time::{Duration, Instant},
};
use tidemark_core::agent_wire::Message;

const MAX_ASKS: usize = 8;
const MAX_NOTICES: usize = 8;
const NOTICE_TTL: Duration = Duration::from_secs(10);
const ASK_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardId {
    Ask(u64),
    Notice(u64),
}
#[derive(Clone, Debug)]
pub struct Card {
    pub id: CardId,
    pub msg: Message,
    pub expires: Instant,
}
struct Pending {
    card: Card,
    reply: Option<Sender<Option<usize>>>,
    expires: Instant,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(None);
        }
    }
}
#[derive(Default)]
struct Queue {
    asks: VecDeque<Pending>,
    notices: VecDeque<Pending>,
    serial: u64,
}

fn fits_ask(msg: &Message) -> bool {
    msg.kind == "ask"
        && msg.title.chars().count() <= 80
        && msg.text.chars().count() <= 160
        && msg.source.chars().count() <= 80
        && !msg.options.is_empty()
        && msg.options.len() <= 4
        && msg
            .options
            .iter()
            .all(|s| !s.is_empty() && s.chars().count() <= 80)
        && [&msg.title, &msg.text, &msg.source]
            .into_iter()
            .chain(msg.options.iter())
            .all(|s| !s.chars().any(char::is_control))
}
fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        text.chars()
            .take(max - 1)
            .chain(std::iter::once('…'))
            .collect()
    }
}
impl Queue {
    fn ask(&mut self, id: u64, msg: Message, reply: Sender<Option<usize>>, now: Instant) {
        if self.asks.len() >= MAX_ASKS
            || !fits_ask(&msg)
            || self.asks.iter().any(|p| p.card.id == CardId::Ask(id))
        {
            let _ = reply.send(None);
            return;
        }
        self.asks.push_back(Pending {
            card: Card {
                id: CardId::Ask(id),
                msg,
                expires: now + ASK_TTL,
            },
            reply: Some(reply),
            expires: now + ASK_TTL,
        });
    }
    fn notice(&mut self, mut msg: Message, now: Instant) {
        if msg.kind != "notice" {
            return;
        }
        msg.source = short(&msg.source, 80);
        msg.title = short(&msg.title, 80);
        msg.text = short(&msg.text, 160);
        msg.options.clear();
        self.serial = self.serial.wrapping_add(1);
        if self.notices.len() >= MAX_NOTICES {
            self.notices.pop_front();
        }
        self.notices.push_back(Pending {
            card: Card {
                id: CardId::Notice(self.serial),
                msg,
                expires: now + NOTICE_TTL,
            },
            reply: None,
            expires: now + NOTICE_TTL,
        });
    }
    fn expire(&mut self, now: Instant) {
        self.asks.retain(|p| p.expires > now);
        self.notices.retain(|p| p.expires > now);
    }
    fn current(&self) -> Option<Card> {
        self.asks
            .front()
            .or_else(|| self.notices.front())
            .map(|p| p.card.clone())
    }
    fn gone(&mut self, id: u64) {
        self.asks.retain(|p| p.card.id != CardId::Ask(id));
    }
    fn choose(&mut self, id: CardId, choice: Option<usize>) -> bool {
        // A queued/stale card may never approve anything while another card is visible.
        if self.current().as_ref().map(|c| c.id) != Some(id) {
            return false;
        }
        match id {
            CardId::Ask(_) => {
                let Some(mut pending) = self.asks.pop_front() else {
                    return false;
                };
                let choice = choice.filter(|&index| index < pending.card.msg.options.len());
                if let Some(reply) = pending.reply.take() {
                    let _ = reply.send(choice);
                }
            }
            CardId::Notice(_) => {
                self.notices.pop_front();
            }
        }
        true
    }
    fn clear(&mut self) {
        self.asks.clear();
        self.notices.clear();
    }
}

#[derive(Clone, Default)]
pub struct Service(Rc<RefCell<Inner>>);
#[derive(Default)]
struct Inner {
    queue: Queue,
    #[cfg(target_os = "linux")]
    server: Option<tidemark_agent::linux::Server>,
}
impl Service {
    #[cfg(target_os = "linux")]
    pub fn start() -> std::io::Result<Self> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);
        let runtime =
            tidemark_agent::linux::resolve_runtime(runtime.as_deref(), unsafe { libc::geteuid() })?;
        let server = tidemark_agent::linux::Server::bind(
            &runtime,
            tidemark_agent::linux::Limits::default(),
        )?;
        Ok(Self(Rc::new(RefCell::new(Inner {
            queue: Queue::default(),
            server: Some(server),
        }))))
    }
    pub fn poll(&self) -> Option<Card> {
        let mut inner = self.0.borrow_mut();
        #[cfg(target_os = "linux")]
        for _ in 0..64 {
            let event = inner.server.as_ref().and_then(|s| s.try_recv());
            match event {
                Some(tidemark_agent::linux::Incoming::Notice(msg)) => {
                    inner.queue.notice(msg, Instant::now())
                }
                Some(tidemark_agent::linux::Incoming::Ask { id, msg, reply }) => {
                    inner.queue.ask(id, msg, reply, Instant::now())
                }
                Some(tidemark_agent::linux::Incoming::Gone(id)) => inner.queue.gone(id),
                None => break,
            }
        }
        inner.queue.expire(Instant::now());
        inner.queue.current()
    }
    pub fn choose(&self, id: CardId, choice: Option<usize>) -> bool {
        // Process cancellations before handling a click from a previously drawn card.
        self.poll();
        self.0.borrow_mut().queue.choose(id, choice)
    }
    pub fn shutdown(&self) {
        let mut inner = self.0.borrow_mut();
        inner.queue.clear();
        #[cfg(target_os = "linux")]
        let server = inner.server.take();
        drop(inner);
        // Server cancellation joins only bounded IPC workers; it never waits
        // for an unanswered card's fifteen-minute human response budget.
        #[cfg(target_os = "linux")]
        drop(server);
    }
    pub fn preview(&self) {
        let (tx, _rx) = std::sync::mpsc::channel();
        self.0.borrow_mut().queue.ask(u64::MAX,Message { kind:"ask".into(),source:"Проверка интерфейса — без соединения с агентом".into(),
            title:"Пример вопроса агента: полный заголовок сохраняется и переносится по ширине".into(),
            text:"Это синтетическая карточка для проверки отображения. Никакая команда не выполняется, разрешение не отправляется. Кнопки отключены в режиме проверки.".into(),
            options:vec!["Оставить в терминале".into(),"Нет".into(),"Да".into()] },tx,Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    fn msg(kind: &str) -> Message {
        Message {
            kind: kind.into(),
            source: "<b>Agent</b>".into(),
            title: "Question".into(),
            text: "Exact plain text".into(),
            options: vec!["Terminal".into(), "No".into(), "Yes".into()],
        }
    }
    #[test]
    fn only_visible_explicit_index_replies_and_dismissal_is_none() {
        let now = Instant::now();
        let mut q = Queue::default();
        let (a, ar) = mpsc::channel();
        let (b, br) = mpsc::channel();
        q.ask(1, msg("ask"), a, now);
        q.ask(2, msg("ask"), b, now);
        assert!(!q.choose(CardId::Ask(2), Some(2)));
        assert!(br.try_recv().is_err());
        assert!(q.choose(CardId::Ask(1), Some(2)));
        assert_eq!(ar.recv().unwrap(), Some(2));
        assert!(q.choose(CardId::Ask(2), None));
        assert_eq!(br.recv().unwrap(), None);
    }
    #[test]
    fn cancellation_and_timeout_remove_stale_actions() {
        let now = Instant::now();
        let mut q = Queue::default();
        let (tx, rx) = mpsc::channel();
        q.ask(1, msg("ask"), tx, now);
        q.gone(1);
        assert_eq!(rx.recv().unwrap(), None);
        assert!(!q.choose(CardId::Ask(1), Some(2)));
        let (tx, rx) = mpsc::channel();
        q.ask(2, msg("ask"), tx, now);
        q.expire(now + ASK_TTL);
        assert!(q.current().is_none());
        assert_eq!(rx.recv().unwrap(), None);
    }
    #[test]
    fn full_supported_text_is_preserved_and_oversized_ask_falls_back() {
        let now = Instant::now();
        let mut q = Queue::default();
        let (tx, rx) = mpsc::channel();
        let mut full = msg("ask");
        full.title = "я".repeat(80);
        full.text = "x".repeat(160);
        q.ask(1, full.clone(), tx, now);
        assert_eq!(q.current().unwrap().msg, full);
        let (tx, rejected) = mpsc::channel();
        full.text.push('x');
        q.ask(2, full, tx, now);
        assert_eq!(rejected.recv().unwrap(), None);
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn shutdown_dismisses_pending_cards_and_preview_has_no_external_receiver() {
        let service = Service::default();
        let (tx, rx) = mpsc::channel();
        service
            .0
            .borrow_mut()
            .queue
            .ask(1, msg("ask"), tx, Instant::now());
        service.shutdown();
        assert_eq!(rx.recv().unwrap(), None);
        assert!(service.poll().is_none());
        service.preview();
        assert!(service.poll().is_some());
        service.shutdown();
    }
    #[test]
    fn pressure_is_bounded_and_notices_expire_behind_asks() {
        let now = Instant::now();
        let mut q = Queue::default();
        let mut replies = Vec::new();
        for id in 0..MAX_ASKS as u64 {
            let (tx, rx) = mpsc::channel();
            q.ask(id, msg("ask"), tx, now);
            replies.push(rx);
        }
        let (tx, rx) = mpsc::channel();
        q.ask(99, msg("ask"), tx, now);
        assert_eq!(rx.recv().unwrap(), None);
        for _ in 0..20 {
            q.notice(msg("notice"), now);
        }
        assert_eq!(q.notices.len(), MAX_NOTICES);
        q.expire(now + NOTICE_TTL);
        assert!(q.notices.is_empty());
        q.clear();
        assert!(replies.iter().all(|rx| rx.recv().unwrap().is_none()));
    }
}
