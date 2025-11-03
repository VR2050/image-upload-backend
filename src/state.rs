use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Semaphore;

// 全局统计
pub static TOTAL_UPLOADED: AtomicU64 = AtomicU64::new(0);
pub static ACTIVE_UPLOADS: AtomicU64 = AtomicU64::new(0);
pub static SERVER_START_TIME: AtomicU64 = AtomicU64::new(0);

// 应用状态管理
#[derive(Debug, Clone)]
pub struct AppState {
    pub global_semaphore: Arc<Semaphore>,
    pub request_count: Arc<AtomicU64>,
    pub error_count: Arc<AtomicU64>,
}

impl AppState {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            global_semaphore: Arc::new(Semaphore::new(max_concurrent)),
            request_count: Arc::new(AtomicU64::new(0)),
            error_count: Arc::new(AtomicU64::new(0)),
        }
    }
    
    pub fn record_request(&self) {
        self.request_count.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_error(&self) {
        self.error_count.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn get_stats(&self) -> serde_json::Value {
        serde_json::json!({
            "total_requests": self.request_count.load(Ordering::Relaxed),
            "total_errors": self.error_count.load(Ordering::Relaxed),
            "available_permits": self.global_semaphore.available_permits(),
            "active_uploads": ACTIVE_UPLOADS.load(Ordering::Relaxed),
            "total_uploaded": TOTAL_UPLOADED.load(Ordering::Relaxed),
        })
    }
}