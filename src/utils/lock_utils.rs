use std::collections::HashMap as StdHashMap;
use std::sync::OnceLock as StdOnceLock;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};

#[derive(Debug, Clone)]
pub struct FileLockEntry {
    pub lock: Arc<Mutex<()>>,
    pub last_used: Instant,
    pub access_count: u64,
}

impl FileLockEntry {
    pub fn new() -> Self {
        Self {
            lock: Arc::new(Mutex::new(())),
            last_used: Instant::now(),
            access_count: 0,
        }
    }
    
    pub fn update_usage(&mut self) {
        self.last_used = Instant::now();
        self.access_count += 1;
    }
}

static FILE_LOCKS: StdOnceLock<Mutex<StdHashMap<String, FileLockEntry>>> = StdOnceLock::new();
static CHUNK_SEMAPHORE: StdOnceLock<Semaphore> = StdOnceLock::new();
static MERGE_SEMAPHORE: StdOnceLock<Semaphore> = StdOnceLock::new();

pub fn init_global_semaphore(max_concurrent: usize) {
    let _ = CHUNK_SEMAPHORE.get_or_init(|| Semaphore::new(max_concurrent));
}

pub fn init_merge_semaphore(max_concurrent: usize) {
    let _ = MERGE_SEMAPHORE.get_or_init(|| Semaphore::new(max_concurrent));
}

pub fn get_merge_semaphore() -> Option<&'static Semaphore> {
    MERGE_SEMAPHORE.get()
}

pub fn get_chunk_semaphore() -> Option<&'static Semaphore> {
    CHUNK_SEMAPHORE.get()
}

// 获取或创建文件级锁
pub async fn get_file_lock(key: &str) -> Arc<Mutex<()>> {
    let map = FILE_LOCKS.get_or_init(|| Mutex::new(StdHashMap::new()));
    let mut guard = map.lock().await;
    
    // 使用默认配置的最大内存锁数量
    let max_memory_locks = 10000; // 或者从配置中获取
    
    // 自动清理：如果锁数量过多，清理最久未使用的
    if guard.len() >= max_memory_locks {
        let mut entries: Vec<(String, FileLockEntry)> = guard.drain().collect();
        entries.sort_by(|a, b| a.1.last_used.cmp(&b.1.last_used));
        
        // 保留最近使用的 80%
        let retain_count = (max_memory_locks * 8) / 10;
        for (k, v) in entries.into_iter().take(retain_count) {
            guard.insert(k, v);
        }
        log::info!("清理文件锁，当前数量: {}", guard.len());
    }
    
    let entry = guard.entry(key.to_string())
        .or_insert_with(FileLockEntry::new);
    entry.update_usage();
    entry.lock.clone()
}

// 定期清理文件锁
pub async fn cleanup_file_locks() -> usize {
    let now = Instant::now();
    let max_age = Duration::from_secs(7200); // 2小时未使用则清理
    
    if let Some(map) = FILE_LOCKS.get() {
        let mut guard = map.lock().await;
        let initial_len = guard.len();
        guard.retain(|_, entry| now.duration_since(entry.last_used) < max_age);
        let cleaned = initial_len - guard.len();
        if cleaned > 0 {
            log::info!("清理了 {} 个过期的文件锁", cleaned);
        }
        cleaned
    } else {
        0
    }
}

// 获取文件锁数量（用于监控）
pub async fn get_file_lock_count() -> usize {
    if let Some(map) = FILE_LOCKS.get() {
        let guard = map.lock().await;
        guard.len()
    } else {
        0
    }
}