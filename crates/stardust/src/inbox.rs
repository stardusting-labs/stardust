use stardust_core::policy::Presentation;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;

#[derive(Debug, Clone, Copy)]
pub struct Wake;

pub enum UiMsg {
    Menu(String),
    Checked(Presentation),
    Pet,
}

#[derive(Clone)]
pub struct Inbox {
    inner: Arc<Inner>,
}

struct Inner {
    queue: Mutex<VecDeque<UiMsg>>,
    cv: Condvar,
    proxy: Mutex<Option<EventLoopProxy<Wake>>>,
}

impl Inbox {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                queue: Mutex::new(VecDeque::new()),
                cv: Condvar::new(),
                proxy: Mutex::new(None),
            }),
        }
    }

    pub fn push(&self, message: UiMsg) {
        {
            let mut queue = self
                .inner
                .queue
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            queue.push_back(message);
        }
        self.inner.cv.notify_one();
        if let Some(proxy) = self
            .inner
            .proxy
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .as_ref()
        {
            let _ = proxy.send_event(Wake);
        }
    }

    pub fn has_bubble(&self) -> bool {
        self.inner
            .queue
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .iter()
            .any(|message| matches!(message, UiMsg::Checked(presentation) if presentation.show_bubble))
    }

    pub fn try_pop(&self) -> Option<UiMsg> {
        self.inner
            .queue
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .pop_front()
    }

    pub fn wait_timeout(&self, timeout: Duration) -> Option<UiMsg> {
        let mut queue = self
            .inner
            .queue
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = queue.pop_front() {
                return Some(message);
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (guard, status) = self
                .inner
                .cv
                .wait_timeout(queue, deadline.saturating_duration_since(now))
                .unwrap_or_else(|err| err.into_inner());
            queue = guard;
            if status.timed_out() && queue.is_empty() {
                return None;
            }
        }
    }

    pub fn set_proxy(&self, proxy: EventLoopProxy<Wake>) {
        *self
            .inner
            .proxy
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = Some(proxy);
    }

    pub fn clear_proxy(&self) {
        *self
            .inner
            .proxy
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = None;
    }
}
