//! Call counters, in-flight tracking and fault injection shared by every simulator.
use super::*;

/// Current and peak execute_kw calls per database.
#[derive(Default)]
pub(super) struct InFlight {
    now: std::collections::HashMap<String, usize>,
    max: std::collections::HashMap<String, usize>,
    total: usize,
    max_total: usize,
}

#[derive(Clone, Default)]
pub struct Counts {
    pub(super) held_databases: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    pub(super) open_held: Arc<Notify>,
    pub(super) in_flight: Arc<std::sync::Mutex<InFlight>>,
    pub(super) response_bytes: Arc<AtomicUsize>,
    pub(super) headers: Arc<std::sync::Mutex<Vec<HeaderMap>>>,
    pub(super) writes: Arc<AtomicUsize>,
    pub(super) logins: Arc<AtomicUsize>,
    pub(super) queries: Arc<AtomicUsize>,
    pub(super) request_bytes: Arc<AtomicUsize>,
    pub(super) lose_response: Arc<AtomicBool>,
    pub(super) pause_next_write: Arc<AtomicBool>,
    pub(super) write_started: Arc<Notify>,
    pub(super) release_write: Arc<Notify>,
    pub(super) pause_next_login: Arc<AtomicBool>,
    pub(super) fail_next_login: Arc<AtomicBool>,
    pub(super) login_started: Arc<Notify>,
    pub(super) release_login: Arc<Notify>,
    pub(super) fail_next_execute: Arc<std::sync::Mutex<Option<String>>>,
}
impl Counts {
    pub fn headers(&self) -> Vec<HeaderMap> {
        self.headers.lock().unwrap().clone()
    }
    pub fn writes(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }
    pub fn logins(&self) -> usize {
        self.logins.load(Ordering::SeqCst)
    }
    pub fn queries(&self) -> usize {
        self.queries.load(Ordering::SeqCst)
    }
    pub fn request_bytes(&self) -> usize {
        self.request_bytes.load(Ordering::SeqCst)
    }
    pub fn lose_next_response(&self) {
        self.lose_response.store(true, Ordering::SeqCst);
    }
    pub fn pause_next_write(&self) {
        self.pause_next_write.store(true, Ordering::SeqCst);
    }
    pub async fn wait_for_write(&self) {
        self.write_started.notified().await;
    }
    pub fn release_write(&self) {
        self.release_write.notify_one();
    }
    pub fn pause_next_login(&self) {
        self.pause_next_login.store(true, Ordering::SeqCst);
    }
    pub fn fail_next_login(&self) {
        self.fail_next_login.store(true, Ordering::SeqCst);
    }
    pub async fn wait_for_login(&self) {
        self.login_started.notified().await;
    }
    pub fn release_login(&self) {
        self.release_login.notify_one();
    }
    /// Fail the next execute_kw with this logical Odoo error.
    pub fn fail_next_execute(&self, message: &str) {
        *self.fail_next_execute.lock().unwrap() = Some(message.into());
    }
}
pub(super) struct InFlightGuard {
    counts: Counts,
    db: String,
}
impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let mut f = self.counts.in_flight.lock().unwrap();
        *f.now.get_mut(&self.db).unwrap() -= 1;
        f.total -= 1;
    }
}
impl Counts {
    /// Hold this database's execute_kw calls until release_databases.
    pub fn hold_database(&self, db: &str) {
        self.held_databases.lock().unwrap().insert(db.into());
    }
    pub fn release_databases(&self) {
        self.held_databases.lock().unwrap().clear();
        self.open_held.notify_waiters();
    }
    pub fn in_flight(&self, db: &str) -> usize {
        let f = self.in_flight.lock().unwrap();
        f.now.get(db).copied().unwrap_or(0)
    }
    pub fn in_flight_total(&self) -> usize {
        self.in_flight.lock().unwrap().total
    }
    pub fn max_in_flight(&self, db: &str) -> usize {
        let f = self.in_flight.lock().unwrap();
        f.max.get(db).copied().unwrap_or(0)
    }
    pub fn max_in_flight_total(&self) -> usize {
        self.in_flight.lock().unwrap().max_total
    }
    /// Bytes in simulator JSON-RPC result responses.
    pub fn response_bytes(&self) -> usize {
        self.response_bytes.load(Ordering::SeqCst)
    }
    pub(super) async fn enter(&self, db: &str) -> InFlightGuard {
        {
            let mut f = self.in_flight.lock().unwrap();
            let now = f.now.entry(db.into()).or_default();
            *now += 1;
            let now = *now;
            let max = f.max.entry(db.into()).or_default();
            *max = (*max).max(now);
            f.total += 1;
            f.max_total = f.max_total.max(f.total);
        }
        let guard = InFlightGuard {
            counts: self.clone(),
            db: db.into(),
        };
        loop {
            // Register before checking to receive later notify_waiters calls.
            let opened = self.open_held.notified();
            if !self.held_databases.lock().unwrap().contains(db) {
                return guard;
            }
            opened.await;
        }
    }
}
