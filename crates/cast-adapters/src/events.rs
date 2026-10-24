//! Bounded host mailbox. Telemetry coalesces; overflow revokes the whole engine.
//! Producers never wait for the UI, and a terminal error always remains observable.
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc::{RecvTimeoutError, TryRecvError},
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_EVENT: usize = 128 * 1024;

struct Entry {
    text: String,
    telemetry: Option<String>,
}
#[derive(Default)]
struct Queue {
    entries: VecDeque<Entry>,
    bytes: usize,
    failed: bool,
    closed: bool,
}
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
    capacity: usize,
    producers: AtomicUsize,
    shutdown: CancellationToken,
}
pub(crate) struct Events(Arc<Shared>);
pub(crate) struct EventInbox(Arc<Shared>);

pub(crate) fn channel(capacity: usize, shutdown: CancellationToken) -> (Events, EventInbox) {
    let shared = Arc::new(Shared {
        queue: Mutex::new(Queue::default()),
        ready: Condvar::new(),
        capacity: capacity.max(1),
        producers: AtomicUsize::new(1),
        shutdown,
    });
    (Events(shared.clone()), EventInbox(shared))
}
impl Clone for Events {
    fn clone(&self) -> Self {
        self.0.producers.fetch_add(1, Ordering::Relaxed);
        Self(self.0.clone())
    }
}
impl Drop for Events {
    fn drop(&mut self) {
        if self.0.producers.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0
                .queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .closed = true;
            self.0.ready.notify_all();
        }
    }
}
impl EventInbox {
    pub(crate) fn try_recv(&self) -> Result<String, TryRecvError> {
        let mut queue = self.0.queue.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = queue.entries.pop_front() {
            queue.bytes -= entry.text.len();
            Ok(entry.text)
        } else if queue.closed {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }
    pub(crate) fn recv_timeout(&self, timeout: Duration) -> Result<String, RecvTimeoutError> {
        let deadline = Instant::now() + timeout;
        let mut queue = self.0.queue.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(entry) = queue.entries.pop_front() {
                queue.bytes -= entry.text.len();
                return Ok(entry.text);
            }
            if queue.closed || self.0.shutdown.is_cancelled() {
                return Err(RecvTimeoutError::Disconnected);
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(RecvTimeoutError::Timeout);
            };
            queue = self
                .0
                .ready
                .wait_timeout(queue, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
    pub(crate) fn wake(&self) {
        // Pair with the waiter's mutex so shutdown cannot be lost between its check and wait.
        let _queue = self.0.queue.lock().unwrap_or_else(|e| e.into_inner());
        self.0.ready.notify_all();
    }
}

fn telemetry_key(value: &Value) -> Option<String> {
    match value["type"].as_str()? {
        "live.stats" => Some("live.stats".into()),
        "message" if value["body"]["type"] == "statistics" => {
            Some(format!("statistics:{}", value["body"]["sessionId"]))
        }
        _ => None,
    }
}
pub(crate) fn emit(events: &Events, value: Value) {
    let text = value.to_string();
    let telemetry = telemetry_key(&value);
    let mut queue = events.0.queue.lock().unwrap_or_else(|e| e.into_inner());
    if queue.failed || queue.closed {
        return;
    }
    if let Some(key) = &telemetry {
        if let Some(index) = queue
            .entries
            .iter()
            .position(|entry| entry.telemetry.as_ref() == Some(key))
        {
            let old = queue.entries.remove(index).unwrap();
            queue.bytes -= old.text.len();
        }
        // Telemetry may be omitted when full; it cannot evict control events.
        if text.len() > MAX_EVENT
            || queue.entries.len() >= events.0.capacity
            || queue.bytes + text.len() > MAX_BYTES
        {
            return;
        }
    }
    if telemetry.is_none() {
        // Prefer removing expendable measurements to terminating a healthy control stream.
        while queue.entries.len() >= events.0.capacity || queue.bytes + text.len() > MAX_BYTES {
            let Some(index) = queue
                .entries
                .iter()
                .position(|entry| entry.telemetry.is_some())
            else {
                break;
            };
            let old = queue.entries.remove(index).unwrap();
            queue.bytes -= old.text.len();
        }
    }
    if text.len() > MAX_EVENT
        || queue.entries.len() >= events.0.capacity
        || queue.bytes + text.len() > MAX_BYTES
    {
        // Replace stale signals with a reserved terminal event and revoke native resources.
        let terminal = json!({"type":"error","body":{"code":"HOST_EVENT_QUEUE_OVERFLOW","terminal":true,"requiresNewConsent":true}}).to_string();
        queue.entries.clear();
        queue.bytes = terminal.len();
        queue.entries.push_back(Entry {
            text: terminal,
            telemetry: None,
        });
        queue.failed = true;
        events.0.shutdown.cancel();
    } else {
        queue.bytes += text.len();
        queue.entries.push_back(Entry { text, telemetry });
    }
    events.0.ready.notify_all();
}
pub(crate) fn event(events: &Events, kind: &str, body: Value) {
    emit(events, json!({"type":kind,"body":body}));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_revokes_engine_and_keeps_one_terminal_event() {
        let stop = CancellationToken::new();
        let (events, inbox) = channel(2, stop.clone());
        for n in 0..10 {
            event(&events, "message", json!({"sequence":n}));
        }
        assert!(stop.is_cancelled());
        let terminal: Value = serde_json::from_str(&inbox.try_recv().unwrap()).unwrap();
        assert_eq!(terminal["body"]["code"], "HOST_EVENT_QUEUE_OVERFLOW");
        assert_eq!(terminal["body"]["terminal"], true);
        assert_eq!(inbox.try_recv(), Err(TryRecvError::Empty));
        event(&events, "connected", json!({}));
        assert_eq!(inbox.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn telemetry_coalesces_without_evicting_ordered_control_events() {
        let stop = CancellationToken::new();
        let (events, inbox) = channel(3, stop.clone());
        event(&events, "session.started", json!({}));
        for n in 0..1000 {
            event(
                &events,
                "message",
                json!({"type":"statistics","sessionId":"a","body":{"n":n}}),
            );
        }
        event(&events, "message", json!({"type":"session.stop"}));
        event(&events, "live.stats", json!({"pulls":1}));
        assert!(!stop.is_cancelled());
        assert!(inbox.try_recv().unwrap().contains("session.started"));
        assert!(inbox.try_recv().unwrap().contains("999"));
        assert!(inbox.try_recv().unwrap().contains("session.stop"));
        assert_eq!(inbox.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn control_can_displace_telemetry_without_reordering_control() {
        let stop = CancellationToken::new();
        let (events, inbox) = channel(2, stop.clone());
        event(&events, "session.started", json!({}));
        event(&events, "live.stats", json!({}));
        event(&events, "stopped", json!({}));
        assert!(!stop.is_cancelled());
        assert!(inbox.try_recv().unwrap().contains("session.started"));
        assert!(inbox.try_recv().unwrap().contains("stopped"));
    }

    #[test]
    fn oversized_control_event_fails_closed_before_queue_growth() {
        let stop = CancellationToken::new();
        let (events, inbox) = channel(256, stop.clone());
        event(&events, "devices", json!({"payload":"x".repeat(MAX_EVENT)}));
        assert!(stop.is_cancelled());
        assert!(inbox.try_recv().unwrap().len() < 256);
    }

    #[test]
    fn waiting_wakes_for_delivery_shutdown_and_last_producer() {
        let stop = CancellationToken::new();
        let (events, inbox) = channel(2, stop.clone());
        let waiter = std::thread::spawn(move || {
            assert!(
                inbox
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap()
                    .contains("stopped")
            );
            assert_eq!(
                inbox.recv_timeout(Duration::from_secs(2)),
                Err(RecvTimeoutError::Disconnected)
            );
        });
        event(&events, "stopped", json!({}));
        drop(events);
        waiter.join().unwrap();
        let (_events, inbox) = channel(2, stop.clone());
        let shared = inbox.0.clone();
        let waiter = std::thread::spawn(move || inbox.recv_timeout(Duration::from_secs(5)));
        stop.cancel();
        EventInbox(shared).wake();
        assert_eq!(waiter.join().unwrap(), Err(RecvTimeoutError::Disconnected));
    }
}
