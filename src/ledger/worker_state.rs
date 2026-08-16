use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use crate::config::WorkerConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerSyncStatus {
    Init,
    Syncing,
    Ready,
    Stale,
}

#[derive(Debug)]
pub struct WorkerRuntimeState {
    pub config: WorkerConfig,
    pub status: parking_lot::RwLock<WorkerSyncStatus>,
    pub active_requests: AtomicUsize,
    pub last_seq: parking_lot::RwLock<u64>,
    pub last_heartbeat: parking_lot::RwLock<Instant>,
}

impl WorkerRuntimeState {
    pub fn new(config: WorkerConfig) -> Self {
        Self {
            config,
            status: parking_lot::RwLock::new(WorkerSyncStatus::Init),
            active_requests: AtomicUsize::new(0),
            last_seq: parking_lot::RwLock::new(0),
            last_heartbeat: parking_lot::RwLock::new(Instant::now()),
        }
    }

    pub fn is_ready_for_exact_kv(&self) -> bool {
        *self.status.read() == WorkerSyncStatus::Ready
    }

    pub fn inc_active_requests(&self) -> usize {
        self.active_requests.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn dec_active_requests(&self) -> usize {
        let prev = self.active_requests.fetch_sub(1, Ordering::SeqCst);
        if prev == 0 {
            self.active_requests.store(0, Ordering::SeqCst);
            0
        } else {
            prev - 1
        }
    }

    pub fn get_active_requests(&self) -> usize {
        self.active_requests.load(Ordering::Relaxed)
    }

    pub fn set_status(&self, new_status: WorkerSyncStatus) {
        *self.status.write() = new_status;
    }

    pub fn update_heartbeat(&self) {
        *self.last_heartbeat.write() = Instant::now();
    }
}
