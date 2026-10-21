use crate::audit::QueueSlot;
use crate::error::{OdxError, Result};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const WAIT: Duration = Duration::from_secs(10);
struct EnvironmentSlots {
    active: Arc<Semaphore>,
    /// Current semaphore permits; `set_concurrency` converges to the target.
    applied: tokio::sync::Mutex<usize>,
    queued: AtomicUsize,
}
#[derive(Clone)]
pub struct Limits {
    global: Arc<Semaphore>,
    global_applied: Arc<tokio::sync::Mutex<usize>>,
    global_target: Arc<AtomicUsize>,
    admission: Arc<Semaphore>,
    admission_cap: u32,
    global_queued: Arc<AtomicUsize>,
    per_environment: Arc<Mutex<HashMap<String, Arc<EnvironmentSlots>>>>,
    per_environment_target: Arc<AtomicUsize>,
    global_queue_limit: usize,
    environment_queue_limit: usize,
    /// Downloads buffer ~130 MiB each (response, string, decoded copy).
    downloads: Arc<Semaphore>,
}
pub struct UpstreamPermits {
    _admission: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
    _environment: OwnedSemaphorePermit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueDepth {
    pub global: usize,
    pub environment: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self::with_caps(16, 4, 64, 16, 180)
    }
}
impl Limits {
    #[doc(hidden)]
    pub fn with_caps(
        global: usize,
        per_environment: usize,
        global_queue_limit: usize,
        environment_queue_limit: usize,
        admission: usize,
    ) -> Self {
        Self {
            global: Arc::new(Semaphore::new(global)),
            global_applied: Arc::new(tokio::sync::Mutex::new(global)),
            global_target: Arc::new(AtomicUsize::new(global)),
            admission: Arc::new(Semaphore::new(admission)),
            admission_cap: u32::try_from(admission).expect("bounded admission"),
            global_queued: Arc::new(AtomicUsize::new(0)),
            per_environment: Arc::new(Mutex::new(HashMap::new())),
            per_environment_target: Arc::new(AtomicUsize::new(per_environment)),
            global_queue_limit,
            environment_queue_limit,
            downloads: Arc::new(Semaphore::new(2)),
        }
    }
    /// Bound concurrent downloads to cap peak memory; others wait up to the queue deadline.
    pub async fn download_permit(&self) -> Result<OwnedSemaphorePermit> {
        match tokio::time::timeout(WAIT, self.downloads.clone().acquire_owned()).await {
            Ok(Ok(p)) => Ok(p),
            _ => Err(OdxError::Capacity),
        }
    }
    pub fn queue_depth(&self, environment_id: &str) -> Result<QueueDepth> {
        let slots = self.slots(environment_id)?;
        Ok(QueueDepth {
            global: self.global_queued.load(Ordering::Acquire),
            environment: slots.queued.load(Ordering::Acquire),
        })
    }
    fn slots(&self, id: &str) -> Result<Arc<EnvironmentSlots>> {
        let mut slots = self
            .per_environment
            .lock()
            .map_err(|_| OdxError::Capacity)?;
        Ok(slots
            .entry(id.to_owned())
            .or_insert_with(|| {
                let cap = self.per_environment_target.load(Ordering::Acquire);
                Arc::new(EnvironmentSlots {
                    active: Arc::new(Semaphore::new(cap)),
                    applied: tokio::sync::Mutex::new(cap),
                    queued: AtomicUsize::new(0),
                })
            })
            .clone())
    }
    async fn global_permit(&self) -> Result<OwnedSemaphorePermit> {
        if let Ok(p) = self.global.clone().try_acquire_owned() {
            return Ok(p);
        }
        let old = self.global_queued.fetch_add(1, Ordering::AcqRel);
        // Decrement even when a waiting call is cancelled and this future is dropped.
        let _queued = QueueSlot(&self.global_queued);
        if old >= self.global_queue_limit {
            return Err(OdxError::Capacity);
        }
        let result = tokio::time::timeout(WAIT, self.global.clone().acquire_owned()).await;
        match result {
            Ok(Ok(p)) => Ok(p),
            _ => Err(OdxError::Capacity),
        }
    }
    async fn environment_permit(
        &self,
        slots: &Arc<EnvironmentSlots>,
    ) -> Result<OwnedSemaphorePermit> {
        if let Ok(p) = slots.active.clone().try_acquire_owned() {
            return Ok(p);
        }
        let old = slots.queued.fetch_add(1, Ordering::AcqRel);
        let _queued = QueueSlot(&slots.queued);
        if old >= self.environment_queue_limit {
            return Err(OdxError::Capacity);
        }
        let result = tokio::time::timeout(WAIT, slots.active.clone().acquire_owned()).await;
        match result {
            Ok(Ok(p)) => Ok(p),
            _ => Err(OdxError::Capacity),
        }
    }
    /// Set global/environment concurrency in call order. Increases apply immediately; decreases retire permits as calls finish, without cancellation. The returned future converges all semaphores to the latest target.
    pub fn set_concurrency(
        &self,
        global: usize,
        per_environment: usize,
    ) -> impl std::future::Future<Output = ()> + Send + 'static {
        self.global_target.store(global, Ordering::Release);
        self.per_environment_target
            .store(per_environment, Ordering::Release);
        let this = self.clone();
        async move {
            converge(&this.global, &this.global_applied, &this.global_target).await;
            let slots: Vec<_> = match this.per_environment.lock() {
                Ok(slots) => slots.values().cloned().collect(),
                Err(_) => return,
            };
            for slot in slots {
                converge(&slot.active, &slot.applied, &this.per_environment_target).await;
            }
        }
    }
    /// Drain queued and sent operations up to `within`; reject new admissions until the drain returns.
    pub async fn wait_idle(&self, within: Duration) -> bool {
        tokio::time::timeout(
            within,
            self.admission
                .clone()
                .acquire_many_owned(self.admission_cap),
        )
        .await
        .is_ok_and(|permits| permits.is_ok())
    }
    pub async fn acquire(&self, environment_id: &str) -> Result<UpstreamPermits> {
        let admission = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| OdxError::Capacity)?;
        let slots = self.slots(environment_id)?;
        let mut global = self.global_permit().await?;
        let environment = match slots.active.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                drop(global);
                let environment = self.environment_permit(&slots).await?;
                global = self.global_permit().await?;
                environment
            }
        };
        Ok(UpstreamPermits {
            _admission: admission,
            _global: global,
            _environment: environment,
        })
    }
}

async fn converge(
    semaphore: &Arc<Semaphore>,
    applied: &tokio::sync::Mutex<usize>,
    target: &AtomicUsize,
) {
    let mut applied = applied.lock().await;
    let want = target.load(Ordering::Acquire);
    if want > *applied {
        semaphore.add_permits(want - *applied);
        *applied = want;
    }
    // Retire permits individually without overtaking existing waiters.
    while *applied > want {
        match semaphore.clone().acquire_owned().await {
            Ok(permit) => permit.forget(),
            Err(_) => return,
        }
        *applied -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn caps_active_queue_and_isolates_environments() {
        let l = Limits::with_caps(1, 1, 1, 1, 3);
        let first = l.acquire("a").await.unwrap();
        let waiting = {
            let l = l.clone();
            tokio::spawn(async move { l.acquire("a").await })
        };
        tokio::task::yield_now().await;
        assert!(matches!(l.acquire("a").await, Err(OdxError::Capacity)));
        assert!(
            l.acquire("b").await.is_err(),
            "global active/queue/admission limits remain finite"
        );
        drop(first);
        let queued = waiting.await.unwrap().unwrap();
        drop(queued);
    }
    #[tokio::test]
    async fn wait_idle_waits_for_held_permits_within_the_deadline() {
        let l = Limits::with_caps(2, 2, 1, 1, 3);
        assert!(l.wait_idle(Duration::from_millis(10)).await);
        let held = l.acquire("a").await.unwrap();
        assert!(!l.wait_idle(Duration::from_millis(20)).await);
        let waiting = {
            let l = l.clone();
            tokio::spawn(async move { l.wait_idle(Duration::from_secs(5)).await })
        };
        tokio::task::yield_now().await;
        drop(held);
        assert!(waiting.await.unwrap());
        // Release drain permits on return to resume admissions.
        drop(l.acquire("a").await.unwrap());
    }
    #[tokio::test]
    async fn set_concurrency_lowers_after_in_flight_calls_and_raises_again() {
        // Without a queue, excess active calls fail immediately.
        let l = Limits::with_caps(2, 2, 0, 0, 10);
        let a = l.acquire("a").await.unwrap();
        let b = l.acquire("a").await.unwrap();
        let lowering = {
            let l = l.clone();
            tokio::spawn(async move { l.set_concurrency(1, 1).await })
        };
        tokio::task::yield_now().await;
        assert!(!lowering.is_finished(), "does not cancel in-flight calls");
        drop(a);
        tokio::time::timeout(Duration::from_secs(5), lowering)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(l.acquire("c").await, Err(OdxError::Capacity)));
        drop(b);
        let only = l.acquire("a").await.unwrap();
        assert!(matches!(l.acquire("a").await, Err(OdxError::Capacity)));
        assert!(matches!(l.acquire("b").await, Err(OdxError::Capacity)));
        drop(only);
        // New environments inherit current concurrency.
        l.set_concurrency(2, 1).await;
        let new_env = l.acquire("d").await.unwrap();
        assert!(matches!(l.acquire("d").await, Err(OdxError::Capacity)));
        let other = l.acquire("e").await.unwrap();
        drop((new_env, other));
        l.set_concurrency(2, 2).await;
        let x = l.acquire("a").await.unwrap();
        let y = l.acquire("a").await.unwrap();
        drop((x, y));
    }
    #[tokio::test(start_paused = true)]
    async fn downloads_are_limited_to_two_at_once() {
        let l = Limits::default();
        let held = (
            l.download_permit().await.unwrap(),
            l.download_permit().await.unwrap(),
        );
        assert!(matches!(l.download_permit().await, Err(OdxError::Capacity)));
        drop(held);
        assert!(l.download_permit().await.is_ok());
    }
}
