use actix_files::Files;
use actix_multipart::Multipart;
use actix_web::{middleware::Logger, web, App, Error, HttpResponse, HttpServer};
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use uuid::Uuid;
use tokio::fs as tokio_fs;
use tokio::io::AsyncWriteExt;
use actix_web::web::block;
use regex::Regex;
use scopeguard;

// 全局配置
const CHUNK_SIZE: usize = 5 * 1024 * 1024; // 5MB 分片大小
const MAX_CONCURRENT_CHUNKS: usize = 3; // 最大并发分片数
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024 * 1024; // 10GB 最大文件大小
const TEMP_FILE_CLEANUP_INTERVAL: Duration = Duration::from_secs(3600); // 临时文件清理间隔
const GLOBAL_MAX_CONCURRENT: usize = 64; // 全局并发限制
const MAX_MEMORY_LOCKS: usize = 10000; // 最大内存中文件锁数量
const LOCK_CLEANUP_INTERVAL: Duration = Duration::from_secs(1800); // 锁清理间隔

// 全局统计
static TOTAL_UPLOADED: AtomicU64 = AtomicU64::new(0);
static ACTIVE_UPLOADS: AtomicU64 = AtomicU64::new(0);
static SERVER_START_TIME: AtomicU64 = AtomicU64::new(0);

// 全局并发控制与文件级锁集合（改进版，支持自动清理）
use std::collections::HashMap as StdHashMap;
use std::sync::OnceLock as StdOnceLock;

static CHUNK_SEMAPHORE: StdOnceLock<Semaphore> = StdOnceLock::new();

#[derive(Debug, Clone)]
struct FileLockEntry {
    lock: Arc<Mutex<()>>,
    last_used: Instant,
    access_count: u64,
}

impl FileLockEntry {
    fn new() -> Self {
        Self {
            lock: Arc::new(Mutex::new(())),
            last_used: Instant::now(),
            access_count: 0,
        }
    }
    
    fn update_usage(&mut self) {
        self.last_used = Instant::now();
        self.access_count += 1;
    }
}

static FILE_LOCKS: StdOnceLock<Mutex<StdHashMap<String, FileLockEntry>>> = StdOnceLock::new();

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Module {
    name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApiResponse<T> {
    success: bool,
    message: String,
    data: Option<T>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FileInfo {
    filename: String,
    url: String,
    module: String,
    upload_time: String,
    size: u64,
    file_type: String,
    relative_path: Option<String>,
    file_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ModuleInfo {
    name: String,
    file_count: usize,
    created_time: String,
    total_size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChunkUploadRequest {
    chunk_number: usize,
    total_chunks: usize,
    filename: String,
    module: String,
    chunk_size: usize,
    relative_path: Option<String>,
    file_hash: Option<String>,
    chunk_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChunkUploadResponse {
    success: bool,
    message: String,
    chunk_number: usize,
    total_chunks: usize,
    filename: String,
    next_chunk: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FolderUploadRequest {
    module: String,
    folder_name: String,
    files: Vec<FolderFileInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FolderFileInfo {
    filename: String,
    relative_path: String,
    size: u64,
    file_type: String,
    file_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct UploadProgress {
    filename: String,
    module: String,
    uploaded_chunks: usize,
    total_chunks: usize,
    total_size: u64,
    uploaded_size: u64,
    speed: f64,
    estimated_time: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ResumeUploadRequest {
    filename: String,
    module: String,
    file_hash: String,
    total_size: u64,
}

// 应用状态管理
#[derive(Debug, Clone)]
struct AppState {
    global_semaphore: Arc<Semaphore>,
    request_count: Arc<AtomicU64>,
    error_count: Arc<AtomicU64>,
}

impl AppState {
    fn new(max_concurrent: usize) -> Self {
        Self {
            global_semaphore: Arc::new(Semaphore::new(max_concurrent)),
            request_count: Arc::new(AtomicU64::new(0)),
            error_count: Arc::new(AtomicU64::new(0)),
        }
    }
    
    fn record_request(&self) {
        self.request_count.fetch_add(1, Ordering::Relaxed);
    }
    
    fn record_error(&self) {
        self.error_count.fetch_add(1, Ordering::Relaxed);
    }
    
    fn get_stats(&self) -> serde_json::Value {
        serde_json::json!({
            "total_requests": self.request_count.load(Ordering::Relaxed),
            "total_errors": self.error_count.load(Ordering::Relaxed),
            "available_permits": self.global_semaphore.available_permits(),
            "active_uploads": ACTIVE_UPLOADS.load(Ordering::Relaxed),
            "total_uploaded": TOTAL_UPLOADED.load(Ordering::Relaxed),
        })
    }
}

// 上传进度管理器（改进版，支持自动清理）
#[derive(Debug)]
struct UploadManager {
    progresses: Mutex<HashMap<String, (UploadProgress, Instant)>>,
}

impl UploadManager {
    fn new() -> Self {
        Self {
            progresses: Mutex::new(HashMap::new()),
        }
    }

    async fn update_progress(&self, key: String, progress: UploadProgress) {
        let mut progresses = self.progresses.lock().await;
        progresses.insert(key, (progress, Instant::now()));
    }

    async fn get_progress(&self, key: &str) -> Option<UploadProgress> {
        let progresses = self.progresses.lock().await;
        progresses.get(key).map(|(progress, _)| progress.clone())
    }

    async fn remove_progress(&self, key: &str) {
        let mut progresses = self.progresses.lock().await;
        progresses.remove(key);
    }
    
    async fn cleanup_expired(&self, max_age: Duration) -> usize {
        let now = Instant::now();
        let mut progresses = self.progresses.lock().await;
        let initial_len = progresses.len();
        
        progresses.retain(|_, (_, last_updated)| {
            now.duration_since(*last_updated) < max_age
        });
        
        initial_len - progresses.len()
    }
    
    async fn get_progress_count(&self) -> usize {
        let progresses = self.progresses.lock().await;
        progresses.len()
    }
}

// 全局上传管理器
static UPLOAD_MANAGER: StdOnceLock<UploadManager> = StdOnceLock::new();

fn get_upload_manager() -> &'static UploadManager {
    UPLOAD_MANAGER.get_or_init(UploadManager::new)
}

// 获取或创建文件级锁（改进版，支持自动清理）
async fn get_file_lock(key: &str) -> Arc<Mutex<()>> {
    let map = FILE_LOCKS.get_or_init(|| Mutex::new(StdHashMap::new()));
    let mut guard = map.lock().await;
    
    // 自动清理：如果锁数量过多，清理最久未使用的
    if guard.len() >= MAX_MEMORY_LOCKS {
        let mut entries: Vec<(String, FileLockEntry)> = guard.drain().collect();
        entries.sort_by(|a, b| a.1.last_used.cmp(&b.1.last_used));
        
        // 保留最近使用的 80%
        let retain_count = (MAX_MEMORY_LOCKS * 8) / 10;
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
async fn cleanup_file_locks() -> usize {
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

// 检查文件扩展名是否为有效的文件格式
fn is_valid_file_extension(ext: &str) -> bool {
    let ext_lower = ext.to_lowercase();
    matches!(
        ext_lower.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "ico" |
        "zip" | "rar" | "7z" | "tar" | "gz" |
        "pdf" | "doc" | "docx" | "txt" | "md" | "json" | "xml" | "csv" | "xls" | "xlsx" | "ppt" | "pptx" |
        "mp4" | "avi" | "mov" | "wmv" | "flv" | "mkv" |
        "mp3" | "wav" | "ogg" | "flac"
    )
}

// 获取文件类型分类
fn get_file_type(ext: &str) -> String {
    let ext_lower = ext.to_lowercase();
    match ext_lower.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "ico" => "image".to_string(),
        "zip" | "rar" | "7z" | "tar" | "gz" => "archive".to_string(),
        "pdf" | "doc" | "docx" | "txt" | "md" | "json" | "xml" | "csv" | "xls" | "xlsx" | "ppt" | "pptx" => "document".to_string(),
        "mp4" | "avi" | "mov" | "wmv" | "flv" | "mkv" => "video".to_string(),
        "mp3" | "wav" | "ogg" | "flac" => "audio".to_string(),
        _ => "other".to_string(),
    }
}

// 系统资源监控
async fn monitor_system_resources() -> serde_json::Value {
    // 简单的资源监控
    let memory_info = if cfg!(target_os = "linux") {
        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            let mut total = 0;
            let mut free = 0;
            for line in content.lines() {
                if line.starts_with("MemTotal:") {
                    total = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                } else if line.starts_with("MemFree:") {
                    free = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                }
            }
            format!("{}MB/{}MB", free / 1024, total / 1024)
        } else {
            "unknown".to_string()
        }
    } else {
        "unknown".to_string()
    };

    serde_json::json!({
        "timestamp": Utc::now().to_rfc3339(),
        "file_locks_count": FILE_LOCKS.get().map(|m| m.try_lock().map(|g| g.len()).unwrap_or(0)).unwrap_or(0),
        "upload_progress_count": get_upload_manager().get_progress_count().await,
        "uptime_seconds": Utc::now().timestamp() as u64 - SERVER_START_TIME.load(Ordering::Relaxed),
        "memory_usage": memory_info,
    })
}

// 后台清理任务
async fn start_background_cleanup() {
    let mut cleanup_interval = tokio::time::interval(Duration::from_secs(1800)); // 30分钟
    
    loop {
        cleanup_interval.tick().await;
        
        log::info!("执行后台清理任务...");
        
        // 清理过期的文件锁
        let locks_cleaned = cleanup_file_locks().await;
        
        // 清理过期的上传进度（1小时未更新）
        let progress_cleaned = get_upload_manager().cleanup_expired(Duration::from_secs(3600)).await;
        
        // 清理临时文件
        let _ = cleanup_temp_files_internal().await;
        
        // 记录资源状态
        let resources = monitor_system_resources().await;
        log::info!("清理完成 - 文件锁: {}, 进度: {}, 资源: {:?}", 
                  locks_cleaned, progress_cleaned, resources);
    }
}

// 创建模块目录
async fn create_module(
    state: web::Data<AppState>,
    module: web::Json<Module>,
) -> HttpResponse {
    state.record_request();
    
    let module_name = module.name.trim();

    if module_name.is_empty() {
        state.record_error();
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "模块名称不能为空".to_string(),
            data: None,
        });
    }

    if module_name.contains("..") || module_name.contains("/") || module_name.contains("\\") {
        state.record_error();
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "模块名称包含非法字符".to_string(),
            data: None,
        });
    }

    let module_path = format!("./uploads/{}", module_name);

    if let Err(e) = tokio_fs::create_dir_all(&module_path).await {
        log::error!("创建模块失败: {}", e);
        state.record_error();
        return HttpResponse::InternalServerError().json(ApiResponse::<()> {
            success: false,
            message: format!("创建模块失败: {}", e),
            data: None,
        });
    }

    let temp_dir = format!("./temp/{}", module_name);
    let _ = tokio_fs::create_dir_all(&temp_dir).await;

    log::info!("模块 '{}' 创建成功", module_name);
    HttpResponse::Ok().json(ApiResponse {
        success: true,
        message: format!("模块 '{}' 创建成功", module_name),
        data: Some(module_name.to_string()),
    })
}

// 获取所有模块的详细信息
async fn get_modules(state: web::Data<AppState>) -> HttpResponse {
    state.record_request();
    
    let uploads_dir = String::from("./uploads");

    let modules_info = block(move || -> Result<Vec<ModuleInfo>, std::io::Error> {
        let mut modules_info = Vec::new();
        for entry in fs::read_dir(&uploads_dir)? {
            let entry = entry?;
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name != "." && name != ".." {
                        let module_path = entry.path();
                        let mut file_count = 0;
                        let mut total_size = 0;

                        let _ = count_files_recursive(&module_path, &mut file_count, &mut total_size);

                        let created_time = match entry.metadata() {
                            Ok(metadata) => {
                                if let Ok(created) = metadata.created() {
                                    let datetime: DateTime<Utc> = created.into();
                                    datetime.format("%Y-%m-%d %H:%M:%S").to_string()
                                } else {
                                    "未知".to_string()
                                }
                            }
                            Err(_) => "未知".to_string(),
                        };

                        modules_info.push(ModuleInfo {
                            name,
                            file_count,
                            created_time,
                            total_size,
                        });
                    }
                }
            }
        }
        Ok(modules_info)
    })
    .await;

    match modules_info {
        Ok(Ok(vec)) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: "获取模块列表成功".to_string(),
            data: Some(vec),
        }),
        Ok(Err(e)) => {
            log::error!("读取模块目录失败 (io): {}", e);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<Vec<ModuleInfo>> {
                success: false,
                message: format!("读取模块目录失败: {}", e),
                data: None,
            })
        }
        Err(e) => {
            log::error!("读取模块目录失败 (blocking): {}", e);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<Vec<ModuleInfo>> {
                success: false,
                message: format!("读取模块目录失败: {}", e),
                data: None,
            })
        }
    }
}

// 递归统计文件数量和大小
fn count_files_recursive(
    path: &Path,
    file_count: &mut usize,
    total_size: &mut u64,
) -> std::io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;

        if file_type.is_file() {
            *file_count += 1;
            if let Ok(metadata) = entry.metadata() {
                *total_size += metadata.len();
            }
        } else if file_type.is_dir() {
            count_files_recursive(&entry.path(), file_count, total_size)?;
        }
    }
    Ok(())
}

// 普通文件上传 - 支持文件夹结构
async fn upload_file(
    state: web::Data<AppState>,
    mut payload: Multipart,
    web::Query(params): web::Query<HashMap<String, String>>,
) -> Result<HttpResponse, Error> {
    state.record_request();
    
    // 获取全局并发许可
    let _permit = state.global_semaphore.acquire().await
        .map_err(|e| {
            log::error!("获取全局并发许可失败: {}", e);
            actix_web::error::ErrorServiceUnavailable("服务器繁忙，请稍后重试")
        })?;

    ACTIVE_UPLOADS.fetch_add(1, Ordering::Relaxed);
    
    // 使用 scopeguard 确保计数器递减
    let _guard = scopeguard::guard((), |_| {
        ACTIVE_UPLOADS.fetch_sub(1, Ordering::Relaxed);
    });

    let module = params
        .get("module")
        .unwrap_or(&"default".to_string())
        .clone();
    let module_path = format!("./uploads/{}", module);

    log::info!("=== 开始文件上传过程 ===");
    log::info!("目标模块: {}", module);
    log::info!("模块路径: {}", module_path);

    // 确保模块目录存在（异步）
    if let Err(e) = tokio_fs::create_dir_all(&module_path).await {
        log::error!("创建模块目录失败: {}", e);
        state.record_error();
        return Ok(HttpResponse::InternalServerError().json(ApiResponse::<()> {
            success: false,
            message: format!("创建模块目录失败: {}", e),
            data: None,
        }));
    }

    let mut uploaded_files = Vec::new();
    let current_time = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut field_count = 0;

    while let Some(mut field) = payload.try_next().await? {
        field_count += 1;
        log::info!("处理第 {} 个文件字段", field_count);

        let content_disposition = field.content_disposition();
        let original_filename = content_disposition
            .as_ref()
            .and_then(|cd| cd.get_filename())
            .map(|f| f.to_string())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        // 获取相对路径
        let relative_path = content_disposition
            .as_ref()
            .and_then(|cd| cd.get_name())
            .map(|s| s.to_string())
            .and_then(|s| {
                if s.contains('/') {
                    let path = Path::new(&s);
                    if let Some(parent) = path.parent() {
                        let parent_str = parent.to_string_lossy().to_string();
                        if !parent_str.is_empty() && parent_str != "." {
                            return Some(parent_str);
                        }
                    }
                }
                None
            });

        log::info!(
            "原始文件名: {}, 相对路径: {:?}",
            original_filename,
            relative_path
        );

        // 获取文件扩展名
        let file_extension = Path::new(&original_filename)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        log::info!("文件扩展名: {}", file_extension);

        // 检查文件类型
        if !is_valid_file_extension(&file_extension) {
            log::warn!("不支持的文件类型: {}", file_extension);
            state.record_error();
            continue;
        }

        let saved_filename = original_filename.clone();

        // 构建完整文件路径
        let filepath = if let Some(rel_path) = &relative_path {
            let full_path = Path::new(&module_path)
                .join(rel_path)
                .join(&saved_filename);
            if let Some(parent) = full_path.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    log::error!("创建子目录失败 {}: {}", parent.display(), e);
                    state.record_error();
                    continue;
                }
            }
            full_path.to_string_lossy().to_string()
        } else {
            format!("{}/{}", module_path, saved_filename)
        };

        // 检查文件是否已存在，如果存在则添加后缀
        let final_filepath = if Path::new(&filepath).exists() {
            let mut counter = 1;
            let stem = Path::new(&original_filename)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("file");
            
            let new_filename = loop {
                let new_name = if file_extension.is_empty() {
                    format!("{}_{}", stem, counter)
                } else {
                    format!("{}_{}.{}", stem, counter, file_extension)
                };
                
                let new_path = if let Some(rel_path) = &relative_path {
                    Path::new(&module_path).join(rel_path).join(&new_name).to_string_lossy().to_string()
                } else {
                    format!("{}/{}", module_path, new_name)
                };
                
                if !Path::new(&new_path).exists() {
                    break new_name;
                }
                counter += 1;
            };
            
            if let Some(rel_path) = &relative_path {
                Path::new(&module_path).join(rel_path).join(&new_filename).to_string_lossy().to_string()
            } else {
                format!("{}/{}", module_path, new_filename)
            }
        } else {
            filepath
        };

        let final_filename = Path::new(&final_filepath)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&saved_filename)
            .to_string();

        log::info!("目标文件路径: {}", final_filepath);

        // 使用 scopeguard 确保文件清理
        let file_cleanup_guard = scopeguard::guard(final_filepath.clone(), |filepath| {
            // 如果文件大小为0，清理空文件
            if let Ok(metadata) = std::fs::metadata(&filepath) {
                if metadata.len() == 0 {
                    if let Err(e) = std::fs::remove_file(&filepath) {
                        log::warn!("清理空文件失败 {}: {}", filepath, e);
                    } else {
                        log::info!("清理空文件: {}", filepath);
                    }
                }
            }
        });

        // 创建并异步写入文件
        let mut total_size: u64 = 0;
        let mut chunk_count: usize = 0;
        let start_time = Instant::now();

        match tokio_fs::File::create(&final_filepath).await {
            Ok(mut async_file) => {
                log::info!("异步文件创建成功: {}", final_filepath);
                
                if let Some(sem) = CHUNK_SEMAPHORE.get() {
                    let _permit = sem.acquire().await;
                    while let Some(chunk) = field.try_next().await? {
                        chunk_count += 1;
                        total_size += chunk.len() as u64;

                        if let Err(e) = async_file.write_all(&chunk).await {
                            log::error!("异步写入文件失败 {} (第{}块): {}", final_filepath, chunk_count, e);
                            state.record_error();
                            break;
                        }

                        if chunk_count % 50 == 0 {
                            let elapsed = start_time.elapsed().as_secs_f64();
                            let speed = (total_size as f64 / 1024.0) / elapsed;
                            log::info!(
                                "已写入 {} 个数据块，大小: {} bytes, 速度: {:.2} KB/s",
                                chunk_count,
                                total_size,
                                speed
                            );
                        }
                    }
                } else {
                    while let Some(chunk) = field.try_next().await? {
                        chunk_count += 1;
                        total_size += chunk.len() as u64;

                        if let Err(e) = async_file.write_all(&chunk).await {
                            log::error!("异步写入文件失败 {} (第{}块): {}", final_filepath, chunk_count, e);
                            state.record_error();
                            break;
                        }

                        if chunk_count % 50 == 0 {
                            let elapsed = start_time.elapsed().as_secs_f64();
                            let speed = (total_size as f64 / 1024.0) / elapsed;
                            log::info!(
                                "已写入 {} 个数据块，大小: {} bytes, 速度: {:.2} KB/s",
                                chunk_count,
                                total_size,
                                speed
                            );
                        }
                    }
                }

                if let Err(e) = async_file.flush().await {
                    log::error!("异步 flush 文件失败 {}: {}", final_filepath, e);
                    state.record_error();
                }
            }
            Err(e) => {
                log::error!("创建文件失败 {}: {}", final_filepath, e);
                state.record_error();
                continue;
            }
        }

        // 检查文件是否成功写入
        if total_size == 0 {
            log::warn!("文件大小为0，跳过: {}", final_filepath);
            state.record_error();
            continue;
        }

        let elapsed = start_time.elapsed().as_secs_f64();
        let speed = if elapsed > 0.0 {
            (total_size as f64 / 1024.0) / elapsed
        } else {
            0.0
        };

        log::info!(
            "文件写入完成，总大小: {} bytes, 总块数: {}, 平均速度: {:.2} KB/s",
            total_size,
            chunk_count,
            speed
        );

        TOTAL_UPLOADED.fetch_add(total_size, Ordering::Relaxed);

        // 构建URL
        let url = if let Some(rel_path) = &relative_path {
            format!("/uploads/{}/{}/{}", module, rel_path, final_filename)
        } else {
            format!("/uploads/{}/{}", module, final_filename)
        };

        let file_info = FileInfo {
            filename: final_filename.clone(),
            url,
            module: module.clone(),
            upload_time: current_time.clone(),
            size: total_size,
            file_type: get_file_type(&file_extension),
            relative_path,
            file_hash: None,
        };

        uploaded_files.push(file_info);
        
        // 确保文件不会被清理
        scopeguard::ScopeGuard::into_inner(file_cleanup_guard);
        
        log::info!("文件上传成功: {} (大小: {} bytes, 平均速度: {:.2} KB/s)", final_filepath, total_size, speed);
    }

    log::info!("=== 文件上传过程结束 ===");
    log::info!("总共处理字段数: {}", field_count);
    log::info!("成功上传文件数: {}", uploaded_files.len());

    if uploaded_files.is_empty() {
        log::warn!("没有成功上传任何文件，处理字段数: {}", field_count);
        state.record_error();
        Ok(HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "没有有效的文件上传".to_string(),
            data: None,
        }))
    } else {
        log::info!("上传成功，返回 {} 个文件信息", uploaded_files.len());
        Ok(HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("成功上传 {} 个文件", uploaded_files.len()),
            data: Some(uploaded_files),
        }))
    }
}

// 分块上传文件 - 支持文件夹结构
async fn upload_chunk(
    state: web::Data<AppState>,
    mut payload: Multipart,
    web::Query(params): web::Query<HashMap<String, String>>,
) -> Result<HttpResponse, Error> {
    state.record_request();
    
    let _permit = state.global_semaphore.acquire().await
        .map_err(|e| {
            log::error!("获取全局并发许可失败: {}", e);
            actix_web::error::ErrorServiceUnavailable("服务器繁忙，请稍后重试")
        })?;

    ACTIVE_UPLOADS.fetch_add(1, Ordering::Relaxed);
    
    let _guard = scopeguard::guard((), |_| {
        ACTIVE_UPLOADS.fetch_sub(1, Ordering::Relaxed);
    });

    let chunk_number = params
        .get("chunk_number")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let total_chunks = params
        .get("total_chunks")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let filename = params
        .get("filename")
        .map(|s| s.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let module = params
        .get("module")
        .map(|s| s.to_string())
        .unwrap_or_else(|| "default".to_string());
    let relative_path = params.get("relative_path").map(|s| s.to_string());
    let file_hash = params.get("file_hash").map(|s| s.to_string());

    // 安全检查
    if filename.contains("..") || filename.contains("/") || filename.contains("\\") {
        log::error!("文件名包含非法字符: {}", filename);
        state.record_error();
        return Ok(HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "文件名包含非法字符".to_string(),
            data: None,
        }));
    }

    // 检查文件大小限制
    if let Some(total_size_str) = params.get("total_size") {
        if let Ok(total_size) = total_size_str.parse::<u64>() {
            if total_size > MAX_FILE_SIZE {
                state.record_error();
                return Ok(HttpResponse::BadRequest().json(ApiResponse::<()> {
                    success: false,
                    message: format!("文件大小超过限制 {}GB", MAX_FILE_SIZE / 1024 / 1024 / 1024),
                    data: None,
                }));
            }
        }
    }

    log::info!("=== 开始分块上传 ===");
    log::info!(
        "文件名: {}, 模块: {}, 分块: {}/{}, 相对路径: {:?}, 文件哈希: {:?}",
        filename,
        module,
        chunk_number + 1,
        total_chunks,
        relative_path,
        file_hash
    );

    // 创建临时目录
    let temp_dir = format!("./temp/{}", module);
    if let Err(e) = fs::create_dir_all(&temp_dir) {
        log::error!("创建临时目录失败: {}", e);
        state.record_error();
        return Ok(HttpResponse::InternalServerError().json(ApiResponse::<()> {
            success: false,
            message: format!("创建临时目录失败: {}", e),
            data: None,
        }));
    }

    let temp_filename = if let Some(rel_path) = &relative_path {
        let safe_path = rel_path.replace('/', "_").replace('\\', "_");
        format!("{}_{}.part{}", safe_path, filename, chunk_number)
    } else {
        format!("{}.part{}", filename, chunk_number)
    };

    let temp_filepath = format!("{}/{}", temp_dir, temp_filename);

    log::info!("临时文件路径: {}", temp_filepath);

    // 使用 scopeguard 确保临时文件清理
    let temp_cleanup_guard = scopeguard::guard(temp_filepath.clone(), |filepath| {
        // 如果分片上传失败，清理临时文件
        if let Ok(metadata) = std::fs::metadata(&filepath) {
            if metadata.len() == 0 {
                if let Err(e) = std::fs::remove_file(&filepath) {
                    log::warn!("清理空临时文件失败 {}: {}", filepath, e);
                } else {
                    log::info!("清理空临时文件: {}", filepath);
                }
            }
        }
    });

    // 检查分片是否已存在
    if Path::new(&temp_filepath).exists() {
        log::info!("分片已存在，跳过上传: {}", temp_filename);
        return Ok(HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: "分片已存在".to_string(),
            data: Some(ChunkUploadResponse {
                success: true,
                message: "分片已存在".to_string(),
                chunk_number,
                total_chunks,
                filename: filename.clone(),
                next_chunk: Some(chunk_number + 1),
            }),
        }));
    }

    // 处理分块数据
    let mut field = match payload.try_next().await? {
        Some(field) => field,
        None => {
            log::error!("没有找到文件字段");
            state.record_error();
            return Ok(HttpResponse::BadRequest().json(ApiResponse::<()> {
                success: false,
                message: "没有找到文件字段".to_string(),
                data: None,
            }));
        }
    };

    // 创建并异步写入临时文件
    let mut chunk_size = 0usize;
    let mut chunk_count = 0usize;
    let start_time = Instant::now();

    match tokio_fs::File::create(&temp_filepath).await {
        Ok(mut async_file) => {
            log::info!("临时文件创建成功 (async): {}", temp_filepath);

            if let Some(sem) = CHUNK_SEMAPHORE.get() {
                let _permit = sem.acquire().await;
                while let Some(chunk) = field.try_next().await? {
                    chunk_count += 1;
                    chunk_size += chunk.len();

                    if let Err(e) = async_file.write_all(&chunk).await {
                        log::error!("异步写入分块数据失败 {}: {}", temp_filepath, e);
                        state.record_error();
                        let tp = temp_filepath.clone();
                        let _ = block(move || fs::remove_file(tp)).await;
                        return Ok(HttpResponse::InternalServerError().json(ApiResponse::<()> {
                            success: false,
                            message: format!("写入分块数据失败: {}", e),
                            data: None,
                        }));
                    }
                }
            } else {
                while let Some(chunk) = field.try_next().await? {
                    chunk_count += 1;
                    chunk_size += chunk.len();

                    if let Err(e) = async_file.write_all(&chunk).await {
                        log::error!("异步写入分块数据失败 {}: {}", temp_filepath, e);
                        state.record_error();
                        let tp = temp_filepath.clone();
                        let _ = block(move || fs::remove_file(tp)).await;
                        return Ok(HttpResponse::InternalServerError().json(ApiResponse::<()> {
                            success: false,
                            message: format!("写入分块数据失败: {}", e),
                            data: None,
                        }));
                    }
                }
            }

            if let Err(e) = async_file.flush().await {
                log::error!("异步 flush 分块文件失败 {}: {}", temp_filepath, e);
                state.record_error();
            }
        }
        Err(e) => {
            log::error!("创建临时文件失败 {}: {}", temp_filepath, e);
            state.record_error();
            return Ok(HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: format!("创建临时文件失败: {}", e),
                data: None,
            }));
        }
    }

    let elapsed = start_time.elapsed().as_secs_f64();
    let speed = if elapsed > 0.0 {
        (chunk_size as f64 / 1024.0) / elapsed
    } else {
        0.0
    };

    log::info!(
        "分块上传成功: {} (大小: {} bytes, 块数: {}, 速度: {:.2} KB/s)",
        temp_filename,
        chunk_size,
        chunk_count,
        speed
    );

    TOTAL_UPLOADED.fetch_add(chunk_size as u64, Ordering::Relaxed);

    // 确保临时文件不会被清理
    scopeguard::ScopeGuard::into_inner(temp_cleanup_guard);

    log::info!("=== 分块上传完成 ===");

    Ok(HttpResponse::Ok().json(ApiResponse {
        success: true,
        message: format!("分块 {} 上传成功", chunk_number + 1),
        data: Some(ChunkUploadResponse {
            success: true,
            message: "分块上传成功".to_string(),
            chunk_number,
            total_chunks,
            filename: filename.clone(),
            next_chunk: if chunk_number + 1 < total_chunks {
                Some(chunk_number + 1)
            } else {
                None
            },
        }),
    }))
}

// 合并分块文件 - 支持文件夹结构
async fn merge_chunks(
    state: web::Data<AppState>,
    info: web::Json<ChunkUploadRequest>,
) -> HttpResponse {
    state.record_request();
    
    let module_path = format!("./uploads/{}", info.module);
    let temp_dir = format!("./temp/{}", info.module);

    // 构建最终文件路径
    let final_filepath = if let Some(rel_path) = &info.relative_path {
        let full_path = Path::new(&module_path).join(rel_path).join(&info.filename);
        if let Some(parent) = full_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                log::error!("创建子目录失败 {}: {}", parent.display(), e);
                state.record_error();
                return HttpResponse::InternalServerError().json(ApiResponse::<()> {
                    success: false,
                    message: format!("创建子目录失败: {}", e),
                    data: None,
                });
            }
        }
        full_path.to_string_lossy().to_string()
    } else {
        format!("{}/{}", module_path, info.filename)
    };

    log::info!("=== 开始合并分块文件 ===");
    log::info!("目标文件: {}", final_filepath);
    log::info!("总分块数: {}", info.total_chunks);
    log::info!("相对路径: {:?}", info.relative_path);

    // 确保模块目录存在
    if let Err(e) = fs::create_dir_all(&module_path) {
        log::error!("创建模块目录失败: {}", e);
        state.record_error();
        return HttpResponse::InternalServerError().json(ApiResponse::<()> {
            success: false,
            message: format!("创建模块目录失败: {}", e),
            data: None,
        });
    }

    // 获取文件级锁
    let file_lock_key = format!("{}_{}", info.module, info.filename);
    let file_lock = get_file_lock(&file_lock_key).await;

    let final_path = final_filepath.clone();
    let temp = temp_dir.clone();
    let filename_clone = info.filename.clone();
    let rel_clone = info.relative_path.clone();
    let total_chunks = info.total_chunks;

    let merge_result = {
        let _fl = file_lock.lock().await;
        let start_time = Instant::now();

        block(move || -> Result<(u64, f64), std::io::Error> {
            // 先写入临时最终文件
            let tmp_final = format!("{}.tmp.{}", final_path, Uuid::new_v4());
            let mut tmp_file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_final)?;

            let mut total_merged_size: u64 = 0;

            for i in 0..total_chunks {
                let temp_filename = if let Some(rel_path) = &rel_clone {
                    let safe_path = rel_path.replace('/', "_").replace('\\', "_");
                    format!("{}_{}.part{}", safe_path, filename_clone, i)
                } else {
                    format!("{}.part{}", filename_clone, i)
                };

                let chunk_filepath = format!("{}/{}", temp, temp_filename);

                if !Path::new(&chunk_filepath).exists() {
                    let _ = std::fs::remove_file(&tmp_final);
                    return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("chunk {} not found", i)));
                }

                let mut chunk_file = std::fs::File::open(&chunk_filepath)?;
                let chunk_size = chunk_file.metadata()?.len();
                std::io::copy(&mut chunk_file, &mut tmp_file)?;

                total_merged_size += chunk_size;

                // 删除临时分片文件
                if let Err(e) = std::fs::remove_file(&chunk_filepath) {
                    log::warn!("删除临时分片文件失败 {}: {}", chunk_filepath, e);
                }
            }

            let _ = tmp_file.sync_all();

            // 原子重命名
            std::fs::rename(&tmp_final, &final_path)?;

            let elapsed = start_time.elapsed().as_secs_f64();
            log::info!("合并完成，耗时: {:.2}s", elapsed);
            Ok((total_merged_size, elapsed))
        })
        .await
    };

    let (total_merged_size, elapsed) = match merge_result {
        Ok(Ok((size, time))) => (size, time),
        Ok(Err(e)) => {
            log::error!("合并失败 (io): {}", e);
            state.record_error();
            return HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: format!("合并失败: {}", e),
                data: None,
            });
        }
        Err(e) => {
            log::error!("合并失败 (blocking): {}", e);
            state.record_error();
            return HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: format!("合并失败: {}", e),
                data: None,
            });
        }
    };

    let merge_speed = if elapsed > 0.0 {
        (total_merged_size as f64 / 1024.0 / 1024.0) / elapsed
    } else {
        0.0
    };

    // 获取文件信息
    let metadata = match fs::metadata(&final_filepath) {
        Ok(meta) => meta,
        Err(e) => {
            log::error!("获取文件元数据失败 {}: {}", final_filepath, e);
            state.record_error();
            return HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: format!("获取文件元数据失败: {}", e),
                data: None,
            });
        }
    };

    let file_extension = Path::new(&info.filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    // 构建URL
    let url = if let Some(rel_path) = &info.relative_path {
        format!("/uploads/{}/{}/{}", info.module, rel_path, info.filename)
    } else {
        format!("/uploads/{}/{}", info.module, info.filename)
    };

    let file_info = FileInfo {
        filename: info.filename.clone(),
        url,
        module: info.module.clone(),
        upload_time: Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        size: metadata.len(),
        file_type: get_file_type(&file_extension),
        relative_path: info.relative_path.clone(),
        file_hash: info.file_hash.clone(),
    };

    log::info!(
        "文件合并成功: {} (大小: {} bytes, 合并速度: {:.2} MB/s, 耗时: {:.2}秒)",
        final_filepath,
        metadata.len(),
        merge_speed,
        elapsed
    );
    log::info!("=== 分块合并完成 ===");

    // 清理上传进度
    let progress_key = format!("{}_{}", info.module, info.filename);
    get_upload_manager().remove_progress(&progress_key).await;

    HttpResponse::Ok().json(ApiResponse {
        success: true,
        message: "文件合并成功".to_string(),
        data: Some(file_info),
    })
}

// 获取上传进度
async fn get_upload_progress(
    state: web::Data<AppState>,
    path: web::Path<(String, String)>,
) -> HttpResponse {
    state.record_request();
    
    let (module, filename) = path.into_inner();
    let progress_key = format!("{}_{}", module, filename);

    if let Some(progress) = get_upload_manager().get_progress(&progress_key).await {
        HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: "获取上传进度成功".to_string(),
            data: Some(progress),
        })
    } else {
        HttpResponse::NotFound().json(ApiResponse::<UploadProgress> {
            success: false,
            message: "未找到上传进度".to_string(),
            data: None,
        })
    }
}

// 检查文件是否存在（秒传功能）
async fn check_file_exists(
    state: web::Data<AppState>,
    info: web::Json<ResumeUploadRequest>,
) -> HttpResponse {
    state.record_request();
    
    let module_path = format!("./uploads/{}", info.module);
    
    let filepath = if Path::new(&info.filename).is_absolute() {
        format!("{}/{}", module_path, info.filename)
    } else {
        format!("{}/{}", module_path, info.filename)
    };

    log::info!("检查文件是否存在: {}", filepath);

    if Path::new(&filepath).exists() {
        let metadata = match fs::metadata(&filepath) {
            Ok(meta) => meta,
            Err(e) => {
                log::error!("获取文件元数据失败: {}", e);
                state.record_error();
                return HttpResponse::InternalServerError().json(ApiResponse::<()> {
                    success: false,
                    message: format!("获取文件元数据失败: {}", e),
                    data: None,
                });
            }
        };

        if metadata.len() == info.total_size {
            log::info!("文件已存在，可秒传: {}", filepath);
            return HttpResponse::Ok().json(ApiResponse {
                success: true,
                message: "文件已存在".to_string(),
                data: Some(serde_json::json!({
                    "exists": true,
                    "size": metadata.len(),
                    "can_instant_upload": true
                })),
            });
        }
    }

    // 检查部分上传的分片
    let temp_dir = format!("./temp/{}", info.module);
    let mut uploaded_chunks = Vec::new();

    let part_re = Regex::new(r"\.part(\d+)$").unwrap();
    if let Ok(entries) = fs::read_dir(&temp_dir) {
        for entry in entries.flatten() {
            if let Ok(file_name) = entry.file_name().into_string() {
                if file_name.contains(&info.filename) {
                    if let Some(cap) = part_re.captures(&file_name) {
                        if let Some(m) = cap.get(1) {
                            if let Ok(chunk_num) = m.as_str().parse::<usize>() {
                                uploaded_chunks.push(chunk_num);
                            }
                        }
                    }
                }
            }
        }
    }

    uploaded_chunks.sort();

    HttpResponse::Ok().json(ApiResponse {
        success: true,
        message: "文件不存在".to_string(),
        data: Some(serde_json::json!({
            "exists": false,
            "uploaded_chunks": uploaded_chunks,
            "can_resume": !uploaded_chunks.is_empty()
        })),
    })
}

// 内部清理临时文件函数
async fn cleanup_temp_files_internal() -> Result<(usize, u64), String> {
    block(move || -> Result<(usize, u64), String> {
        let temp_dir = "./temp";
        let mut cleaned_count = 0usize;
        let mut total_size = 0u64;

        if let Ok(entries) = fs::read_dir(temp_dir) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        let module_temp_dir = entry.path();
                        if let Ok(files) = fs::read_dir(&module_temp_dir) {
                            for file_entry in files.flatten() {
                                if let Ok(metadata) = file_entry.metadata() {
                                    if let Ok(created) = metadata.created() {
                                        let age = created.elapsed().unwrap_or_default();
                                        if age > Duration::from_secs(24 * 3600) {
                                            if let Ok(file_name) = file_entry.file_name().into_string() {
                                                if file_name.ends_with(".part") || file_name.contains(".part") || file_name.contains(".tmp.") {
                                                    if let Err(e) = fs::remove_file(file_entry.path()) {
                                                        log::warn!("清理临时文件失败 {}: {}", file_name, e);
                                                    } else {
                                                        cleaned_count += 1;
                                                        total_size += metadata.len();
                                                        log::debug!("清理临时文件: {}", file_name);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok((cleaned_count, total_size))
    })
    .await
    .map_err(|e| format!("Blocking error: {}", e))?
}

// 清理临时文件
async fn cleanup_temp_files(state: web::Data<AppState>) -> HttpResponse {
    state.record_request();
    
    log::info!("开始清理临时文件...");

    match cleanup_temp_files_internal().await {
        Ok((cleaned_count, total_size)) => {
            log::info!("临时文件清理完成: 清理了 {} 个文件, 释放 {} bytes", cleaned_count, total_size);
            HttpResponse::Ok().json(ApiResponse::<()> {
                success: true,
                message: format!("清理了 {} 个临时文件, 释放 {} bytes", cleaned_count, total_size),
                data: None,
            })
        }
        Err(err) => {
            log::error!("cleanup_temp_files 错误: {}", err);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: err,
                data: None,
            })
        }
    }
}

// 获取系统统计信息
async fn get_stats(state: web::Data<AppState>) -> HttpResponse {
    state.record_request();
    
    let res = block(move || -> Result<serde_json::Value, String> {
        let uploads_dir = "./uploads";
        let temp_dir = "./temp";
        let mut total_modules = 0usize;
        let mut total_files = 0usize;
        let mut total_size = 0u64;
        let mut temp_files_count = 0usize;
        let mut temp_files_size = 0u64;

        // 统计上传文件
        if let Ok(entries) = fs::read_dir(uploads_dir) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name != "." && name != ".." {
                            total_modules += 1;
                            let _ = count_files_recursive(&entry.path(), &mut total_files, &mut total_size);
                        }
                    }
                }
            }
        }

        // 统计临时文件
        if let Ok(entries) = fs::read_dir(temp_dir) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        if let Ok(files) = fs::read_dir(entry.path()) {
                            for file_entry in files.flatten() {
                                if let Ok(metadata) = file_entry.metadata() {
                                    if metadata.is_file() {
                                        temp_files_count += 1;
                                        temp_files_size += metadata.len();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let stats = serde_json::json!({
            "total_modules": total_modules,
            "total_files": total_files,
            "total_size": total_size,
            "total_size_mb": (total_size as f64 / 1024.0 / 1024.0).round() as u64,
            "total_size_gb": (total_size as f64 / 1024.0 / 1024.0 / 1024.0).round() as f64,
            "temp_files_count": temp_files_count,
            "temp_files_size": temp_files_size,
            "active_uploads": ACTIVE_UPLOADS.load(Ordering::Relaxed),
            "total_uploaded": TOTAL_UPLOADED.load(Ordering::Relaxed),
            "chunk_size_mb": CHUNK_SIZE / 1024 / 1024,
            "max_file_size_gb": MAX_FILE_SIZE / 1024 / 1024 / 1024
        });

        Ok(stats)
    })
    .await;

    match res {
        Ok(Ok(stats)) => {
            // 合并应用状态统计
            let mut stats_value = stats;
            if let Some(obj) = stats_value.as_object_mut() {
                obj.extend(state.get_stats().as_object().unwrap().clone());
            }
            HttpResponse::Ok().json(ApiResponse {
                success: true,
                message: "获取统计信息成功".to_string(),
                data: Some(stats_value),
            })
        }
        Ok(Err(err_msg)) => {
            log::error!("get_stats 错误: {}", err_msg);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: err_msg,
                data: None,
            })
        }
        Err(e) => {
            log::error!("blocking get_stats 失败: {}", e);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: format!("blocking get_stats 失败: {}", e),
                data: None,
            })
        }
    }
}

// 获取模块中的文件列表
async fn get_module_files(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    state.record_request();
    
    let module = path.into_inner();
    let module_path = format!("./uploads/{}", module);
    
    log::info!("获取模块文件列表: {}", module);

    let module_clone = module.clone();
    let module_path_clone = module_path.clone();
    let res = block(move || -> Result<Vec<FileInfo>, String> {
        if !Path::new(&module_path_clone).exists() {
            return Err(format!("模块 '{}' 不存在", module_clone));
        }

        let mut files_local: Vec<FileInfo> = Vec::new();
        if let Err(e) = collect_files_recursive(&PathBuf::from(&module_path_clone), "", &mut files_local, &module_clone) {
            return Err(format!("收集文件失败: {}", e));
        }

        files_local.sort_by(|a, b| b.upload_time.cmp(&a.upload_time));
        Ok(files_local)
    })
    .await;

    match res {
        Ok(Ok(mut files_local)) => {
            log::info!("找到 {} 个文件", files_local.len());
            HttpResponse::Ok().json(ApiResponse {
                success: true,
                message: format!("获取模块 '{}' 的文件列表成功", module),
                data: Some(files_local),
            })
        }
        Ok(Err(err_msg)) => {
            log::error!("{}", err_msg);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<Vec<FileInfo>> {
                success: false,
                message: err_msg,
                data: None,
            })
        }
        Err(e) => {
            log::error!("blocking 执行失败: {}", e);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<Vec<FileInfo>> {
                success: false,
                message: format!("blocking 执行失败: {}", e),
                data: None,
            })
        }
    }
}

// 递归收集文件信息
fn collect_files_recursive(
    base_path: &Path,
    current_path: &str,
    files: &mut Vec<FileInfo>,
    module: &str,
) -> std::io::Result<()> {
    let full_path = if current_path.is_empty() {
        base_path.to_path_buf()
    } else {
        base_path.join(current_path)
    };

    for entry in fs::read_dir(full_path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;

        if file_type.is_file() {
            let path = entry.path();
            if let Some(file_name) = path.file_name() {
                let filename = file_name.to_string_lossy().to_string();

                let metadata = entry.metadata()?;
                let size = metadata.len();
                let created = metadata
                    .created()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                let upload_time: DateTime<Utc> = created.into();

                let file_extension = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();

                let relative_path = if current_path.is_empty() {
                    None
                } else {
                    Some(current_path.to_string())
                };

                let url = if let Some(rel_path) = &relative_path {
                    format!("/uploads/{}/{}/{}", module, rel_path, filename)
                } else {
                    format!("/uploads/{}/{}", module, filename)
                };

                let file_info = FileInfo {
                    filename: filename.clone(),
                    url,
                    module: module.to_string(),
                    upload_time: upload_time.format("%Y-%m-%d %H:%M:%S").to_string(),
                    size,
                    file_type: get_file_type(&file_extension),
                    relative_path,
                    file_hash: None,
                };
                files.push(file_info);
            }
        } else if file_type.is_dir() {
            let dir_name = entry.file_name().to_string_lossy().to_string();
            let new_path = if current_path.is_empty() {
                dir_name
            } else {
                format!("{}/{}", current_path, dir_name)
            };
            collect_files_recursive(base_path, &new_path, files, module)?;
        }
    }
    Ok(())
}

// 删除文件
async fn delete_file(
    state: web::Data<AppState>,
    path: web::Path<(String, String)>,
) -> HttpResponse {
    state.record_request();
    
    let (module, filename) = path.into_inner();
    let file_path = format!("./uploads/{}/{}", module, filename);

    log::info!("删除文件: {}", file_path);

    if filename.contains("..") || filename.contains("//") {
        state.record_error();
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "文件名包含非法字符".to_string(),
            data: None,
        });
    }

    match fs::remove_file(&file_path) {
        Ok(_) => {
            log::info!("文件删除成功: {}", file_path);
            HttpResponse::Ok().json(ApiResponse::<()> {
                success: true,
                message: "文件删除成功".to_string(),
                data: None,
            })
        }
        Err(e) => {
            log::error!("删除文件失败: {}", e);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: format!("删除文件失败: {}", e),
                data: None,
            })
        }
    }
}

// 删除文件夹
async fn delete_folder(
    state: web::Data<AppState>,
    path: web::Path<(String, String)>,
) -> HttpResponse {
    state.record_request();
    
    let (module, folder_path) = path.into_inner();
    let full_path = format!("./uploads/{}/{}", module, folder_path);

    log::info!("删除文件夹: {}", full_path);

    if folder_path.contains("..") || folder_path.contains("//") {
        state.record_error();
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "文件夹路径包含非法字符".to_string(),
            data: None,
        });
    }

    match fs::remove_dir_all(&full_path) {
        Ok(_) => {
            log::info!("文件夹删除成功: {}", full_path);
            HttpResponse::Ok().json(ApiResponse::<()> {
                success: true,
                message: "文件夹删除成功".to_string(),
                data: None,
            })
        }
        Err(e) => {
            log::error!("删除文件夹失败: {}", e);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: format!("删除文件夹失败: {}", e),
                data: None,
            })
        }
    }
}

// 删除模块
async fn delete_module(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    state.record_request();
    
    let module = path.into_inner();
    let module_path = format!("./uploads/{}", module);
    let temp_dir = format!("./temp/{}", module);

    log::info!("删除模块: {}", module);

    if module.contains("..") || module.contains("/") || module.contains("\\") {
        state.record_error();
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "模块名称包含非法字符".to_string(),
            data: None,
        });
    }

    let result1 = fs::remove_dir_all(&module_path);
    let _result2 = fs::remove_dir_all(&temp_dir);

    match result1 {
        Ok(_) => {
            log::info!("模块删除成功: {}", module);
            HttpResponse::Ok().json(ApiResponse::<()> {
                success: true,
                message: format!("模块 '{}' 删除成功", module),
                data: None,
            })
        }
        Err(e) => {
            log::error!("删除模块失败: {}", e);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: format!("删除模块失败: {}", e),
                data: None,
            })
        }
    }
}

// 健康检查
async fn health_check(state: web::Data<AppState>) -> HttpResponse {
    state.record_request();
    
    let resources = monitor_system_resources().await;
    let app_stats = state.get_stats();
    
    let health_info = serde_json::json!({
        "status": "healthy",
        "timestamp": Utc::now().to_rfc3339(),
        "resources": resources,
        "app_stats": app_stats,
    });

    HttpResponse::Ok().json(ApiResponse {
        success: true,
        message: "服务运行正常".to_string(),
        data: Some(health_info),
    })
}

// 优雅关闭处理
async fn graceful_shutdown() {
    log::info!("接收到关闭信号，开始优雅关闭...");
    
    // 执行清理操作
    log::info!("清理文件锁...");
    let locks_cleaned = cleanup_file_locks().await;
    
    log::info!("清理上传进度...");
    let progress_cleaned = get_upload_manager().cleanup_expired(Duration::from_secs(0)).await;
    
    log::info!("清理临时文件...");
    let _ = cleanup_temp_files_internal().await;
    
    log::info!("优雅关闭完成 - 清理文件锁: {}, 进度: {}", locks_cleaned, progress_cleaned);
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let mut args: Vec<String> = std::env::args().collect();
    let port = args.pop().unwrap_or("2233".to_string());
    let address = args.pop().unwrap_or("127.0.0.1".to_string());

    // 初始化日志
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    // 记录服务器启动时间
    SERVER_START_TIME.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    // 创建上传目录和临时目录
    if let Err(e) = fs::create_dir_all("./uploads") {
        log::error!("创建上传目录失败: {}", e);
        return Err(e);
    }

    if let Err(e) = fs::create_dir_all("./uploads/default") {
        log::error!("创建默认模块目录失败: {}", e);
        return Err(e);
    }

    if let Err(e) = fs::create_dir_all("./temp") {
        log::error!("创建临时目录失败: {}", e);
        return Err(e);
    }

    // 创建前端目录
    if let Err(e) = fs::create_dir_all("./frontend") {
        log::warn!("创建前端目录失败: {}", e);
    }

    log::info!("启动优化的文件上传管理系统...");
    log::info!("配置信息:");
    log::info!("  - 分片大小: {}MB", CHUNK_SIZE / 1024 / 1024);
    log::info!("  - 最大文件大小: {}GB", MAX_FILE_SIZE / 1024 / 1024 / 1024);
    log::info!("  - 最大并发分片数: {}", MAX_CONCURRENT_CHUNKS);
    log::info!("  - 全局并发限制: {}", GLOBAL_MAX_CONCURRENT);
    println!("服务器运行在：http://{}:{}", address, port);
    log::info!("上传目录: ./uploads/");
    log::info!("临时目录: ./temp/");

    // 初始化全局并发控制
    let _ = CHUNK_SEMAPHORE.get_or_init(|| Semaphore::new(GLOBAL_MAX_CONCURRENT));

    // 创建应用状态
    let app_state = web::Data::new(AppState::new(GLOBAL_MAX_CONCURRENT));

    // 启动后台清理任务
    let _cleanup_handle = actix_web::rt::spawn(start_background_cleanup());

    let server = HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .wrap(Logger::default())
            .app_data(web::PayloadConfig::new(MAX_FILE_SIZE as usize))
            .service(
                web::scope("/api")
                    .route("/health", web::get().to(health_check))
                    .route("/stats", web::get().to(get_stats))
                    .route("/modules", web::get().to(get_modules))
                    .route("/modules", web::post().to(create_module))
                    .route("/modules/{module}", web::delete().to(delete_module))
                    .route("/upload", web::post().to(upload_file))
                    .route("/upload/chunk", web::post().to(upload_chunk))
                    .route("/upload/merge", web::post().to(merge_chunks))
                    .route("/upload/progress/{module}/{filename}", web::get().to(get_upload_progress))
                    .route("/upload/check", web::post().to(check_file_exists))
                    .route("/cleanup", web::post().to(cleanup_temp_files))
                    .route("/files/{module}", web::get().to(get_module_files))
                    .route("/file/{module}/{filename}", web::delete().to(delete_file))
                    .route(
                        "/folder/{module}/{folder_path:.*}",
                        web::delete().to(delete_folder),
                    ),
            )
            .service(
                Files::new("/uploads", "./uploads")
                    .show_files_listing()
                    .use_last_modified(true),
            )
            .service(
                Files::new("/", "./frontend")
                    .index_file("index.html")
                    .prefer_utf8(true),
            )
    })
    .bind(format!("{}:{}", address, port))?
    .run();

    // 设置优雅关闭
    let server_handle = server.handle();
    let shutdown_signal = async {
        tokio::signal::ctrl_c().await.expect("Failed to install CTRL+C handler");
        log::info!("接收到关闭信号");
    };

    tokio::select! {
        _ = server => {
            log::info!("服务器正常退出");
        }
        _ = shutdown_signal => {
            log::info!("开始优雅关闭流程");
            // 先停止接收新请求
            server_handle.stop(true).await;
            // 执行清理
            graceful_shutdown().await;
            log::info!("优雅关闭完成");
        }
    }

    Ok(())
}