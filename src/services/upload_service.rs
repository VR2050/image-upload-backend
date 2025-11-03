use actix_web::{web, HttpResponse, Error};
use actix_multipart::{Multipart, Field};
use futures_util::TryStreamExt;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tokio::fs as tokio_fs;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::{
    models::{FileInfo, ChunkUploadRequest, ChunkUploadResponse, ResumeUploadRequest, UploadProgress},
    state::{AppState, TOTAL_UPLOADED},
    utils::{file_utils, lock_utils, validation_utils},
};
use crate::services::file_service;

// 上传进度管理器
use std::collections::HashMap as StdHashMap;
use std::sync::OnceLock as StdOnceLock;
use tokio::sync::Mutex;

static UPLOAD_MANAGER: StdOnceLock<UploadManager> = StdOnceLock::new();

#[derive(Debug)]
struct UploadManager {
    progresses: Mutex<StdHashMap<String, (UploadProgress, Instant)>>,
}

impl UploadManager {
    fn new() -> Self {
        Self {
            progresses: Mutex::new(StdHashMap::new()),
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

fn get_upload_manager() -> &'static UploadManager {
    UPLOAD_MANAGER.get_or_init(UploadManager::new)
}

pub async fn handle_file_upload(
    state: web::Data<AppState>,
    mut payload: Multipart,
    params: web::Query<HashMap<String, String>>,
) -> Result<HttpResponse, Error> {
    let module = params
        .get("module")
        .unwrap_or(&"default".to_string())
        .clone();

    log::info!("=== 开始文件上传过程 ===");
    log::info!("目标模块: {}", module);

    let mut uploaded_files = Vec::new();
    let current_time = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
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
        // sanitize filename to basename to avoid path traversal
        let original_filename = Path::new(&original_filename)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
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

        // 获取文件扩展名
        let file_extension = Path::new(&original_filename)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        // 检查文件类型
        if !file_utils::is_valid_file_extension(&file_extension) {
            log::warn!("不支持的文件类型: {}", file_extension);
            state.record_error();
            continue;
        }

        // 构建文件路径并处理上传
        match process_single_file_upload(
            &state,
            &module,
            &original_filename,
            &relative_path,
            &file_extension,
            &current_time,
            &mut field,
        ).await {
            Ok(Some(file_info)) => {
                uploaded_files.push(file_info);
            }
            Ok(None) => {
                // 文件被跳过
            }
            Err(e) => {
                log::error!("文件上传失败: {}", e);
                state.record_error();
            }
        }
    }

    log::info!("=== 文件上传过程结束 ===");
    log::info!("总共处理字段数: {}", field_count);
    log::info!("成功上传文件数: {}", uploaded_files.len());

    if uploaded_files.is_empty() {
        Ok(HttpResponse::BadRequest().json(crate::models::ApiResponse::<()> {
            success: false,
            message: "没有有效的文件上传".to_string(),
            data: None,
        }))
    } else {
        Ok(HttpResponse::Ok().json(crate::models::ApiResponse {
            success: true,
            message: format!("成功上传 {} 个文件", uploaded_files.len()),
            data: Some(uploaded_files),
        }))
    }
}

// 处理单个文件上传的辅助函数
async fn process_single_file_upload(
    _state: &web::Data<AppState>,
    module: &str,
    original_filename: &str,
    relative_path: &Option<String>,
    file_extension: &str,
    current_time: &str,
    field: &mut Field,
) -> Result<Option<FileInfo>, Error> {
    // 构建文件路径
    let final_filepath = file_service::build_file_path(module, original_filename, relative_path)
        .await
        .map_err(|e| actix_web::error::ErrorInternalServerError(e))?;
    let final_filename = Path::new(&final_filepath)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(original_filename)
        .to_string();

    log::info!("目标文件路径: {}", final_filepath);

    // 上传文件内容
    let total_size = upload_file_content(&final_filepath, field).await?;

    if total_size == 0 {
        log::warn!("文件大小为0，跳过: {}", final_filepath);
        return Ok(None);
    }

    // 构建文件信息
    let url = if let Some(rel_path) = relative_path {
        format!("/uploads/{}/{}/{}", module, rel_path, final_filename)
    } else {
        format!("/uploads/{}/{}", module, final_filename)
    };

    let file_info = FileInfo {
        filename: final_filename,
        url,
        module: module.to_string(),
        upload_time: current_time.to_string(),
        size: total_size,
        file_type: file_utils::get_file_type(file_extension),
        relative_path: relative_path.clone(),
        file_hash: None,
    };

    TOTAL_UPLOADED.fetch_add(total_size, Ordering::Relaxed);

    log::info!("文件上传成功: {} (大小: {} bytes)", final_filepath, total_size);
    Ok(Some(file_info))
}

// 上传文件内容的辅助函数
async fn upload_file_content(
    filepath: &str,
    field: &mut Field,
) -> Result<u64, Error> {
    let mut total_size: u64 = 0;
    let mut chunk_count: usize = 0;
    let start_time = Instant::now();

    let mut async_file = tokio_fs::File::create(filepath).await
        .map_err(|e| {
            log::error!("创建文件失败 {}: {}", filepath, e);
            actix_web::error::ErrorInternalServerError(format!("创建文件失败: {}", e))
        })?;

    if let Some(sem) = lock_utils::get_chunk_semaphore() {
        let _permit = sem.acquire().await;
        while let Some(chunk) = field.try_next().await? {
            chunk_count += 1;
            total_size += chunk.len() as u64;

            async_file.write_all(&chunk).await
                .map_err(|e| {
                    log::error!("写入文件失败 {} (第{}块): {}", filepath, chunk_count, e);
                    // 删除部分写入的文件
                    let fp = filepath.to_string();
                    tokio::spawn(async move { let _ = tokio::fs::remove_file(fp).await; });
                    actix_web::error::ErrorInternalServerError(format!("写入文件失败: {}", e))
                })?;

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

            async_file.write_all(&chunk).await
                .map_err(|e| {
                    log::error!("写入文件失败 {} (第{}块): {}", filepath, chunk_count, e);
                    // 删除部分写入的文件
                    let fp = filepath.to_string();
                    tokio::spawn(async move { let _ = tokio::fs::remove_file(fp).await; });
                    actix_web::error::ErrorInternalServerError(format!("写入文件失败: {}", e))
                })?;
        }
    }

    async_file.flush().await
        .map_err(|e| {
            log::error!("flush文件失败 {}: {}", filepath, e);
            let fp = filepath.to_string();
            tokio::spawn(async move { let _ = tokio::fs::remove_file(fp).await; });
            actix_web::error::ErrorInternalServerError(format!("flush文件失败: {}", e))
        })?;

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

    Ok(total_size)
}

// 由于篇幅限制，分块上传、合并等函数的实现将在下一个回复中继续

// 接上面的 upload_service.rs

pub async fn handle_chunk_upload(
    state: web::Data<AppState>,
    mut payload: Multipart,
    params: web::Query<HashMap<String, String>>,
) -> Result<HttpResponse, Error> {
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
    let _file_hash = params.get("file_hash").map(|s| s.to_string());

    // 安全检查
    if !validation_utils::is_valid_filename(&filename) {
        log::error!("文件名包含非法字符: {}", filename);
        state.record_error();
        return Ok(HttpResponse::BadRequest().json(crate::models::ApiResponse::<()> {
            success: false,
            message: "文件名包含非法字符".to_string(),
            data: None,
        }));
    }

    // 检查文件大小限制
    if let Some(total_size_str) = params.get("total_size") {
        if let Ok(total_size) = total_size_str.parse::<u64>() {
            if !validation_utils::is_valid_file_size(total_size, crate::config::ServerConfig::default().max_file_size) {
                state.record_error();
                return Ok(HttpResponse::BadRequest().json(crate::models::ApiResponse::<()> {
                    success: false,
                    message: format!("文件大小超过限制 {}GB", 
                        crate::config::ServerConfig::default().max_file_size / 1024 / 1024 / 1024),
                    data: None,
                }));
            }
        }
    }

    log::info!("=== 开始分块上传 ===");
    log::info!(
        "文件名: {}, 模块: {}, 分块: {}/{}, 相对路径: {:?}",
        filename,
        module,
        chunk_number + 1,
        total_chunks,
        relative_path
    );

    // 创建临时目录
    let temp_dir = format!("./temp/{}", module);
    if let Err(e) = tokio::fs::create_dir_all(&temp_dir).await {
        log::error!("创建临时目录失败: {}", e);
        state.record_error();
        return Ok(HttpResponse::InternalServerError().json(crate::models::ApiResponse::<()> {
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

    // 检查分片是否已存在
    if Path::new(&temp_filepath).exists() {
        log::info!("分片已存在，跳过上传: {}", temp_filename);
        return Ok(HttpResponse::Ok().json(crate::models::ApiResponse {
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
            return Ok(HttpResponse::BadRequest().json(crate::models::ApiResponse::<()> {
                success: false,
                message: "没有找到文件字段".to_string(),
                data: None,
            }));
        }
    };

    // 上传分块数据
    let chunk_size = upload_chunk_content(&temp_filepath, &mut field).await?;

    TOTAL_UPLOADED.fetch_add(chunk_size as u64, std::sync::atomic::Ordering::Relaxed);

    log::info!("=== 分块上传完成 ===");

    Ok(HttpResponse::Ok().json(crate::models::ApiResponse {
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

// 上传分块内容的辅助函数
async fn upload_chunk_content(
    temp_filepath: &str,
    field: &mut Field,
) -> Result<usize, Error> {
    let mut chunk_size = 0usize;
    let mut chunk_count = 0usize;
    let start_time = Instant::now();

    let mut async_file = tokio_fs::File::create(temp_filepath).await
        .map_err(|e| {
            log::error!("创建临时文件失败 {}: {}", temp_filepath, e);
            actix_web::error::ErrorInternalServerError(format!("创建临时文件失败: {}", e))
        })?;

    if let Some(sem) = lock_utils::get_chunk_semaphore() {
        let _permit = sem.acquire().await;
        while let Some(chunk) = field.try_next().await? {
            chunk_count += 1;
            chunk_size += chunk.len();

            async_file.write_all(&chunk).await
                .map_err(|e| {
                    log::error!("写入分块数据失败 {}: {}", temp_filepath, e);
                    // 清理临时文件
                    let tp = temp_filepath.to_string();
                    tokio::spawn(async move {
                        let _ = tokio::fs::remove_file(tp).await;
                    });
                    actix_web::error::ErrorInternalServerError(format!("写入分块数据失败: {}", e))
                })?;
        }
    } else {
        while let Some(chunk) = field.try_next().await? {
            chunk_count += 1;
            chunk_size += chunk.len();

            async_file.write_all(&chunk).await
                .map_err(|e| {
                    log::error!("写入分块数据失败 {}: {}", temp_filepath, e);
                    let tp = temp_filepath.to_string();
                    tokio::spawn(async move {
                        let _ = tokio::fs::remove_file(tp).await;
                    });
                    actix_web::error::ErrorInternalServerError(format!("写入分块数据失败: {}", e))
                })?;
        }
    }

    async_file.flush().await
        .map_err(|e| {
            log::error!("flush分块文件失败 {}: {}", temp_filepath, e);
            actix_web::error::ErrorInternalServerError(format!("flush分块文件失败: {}", e))
        })?;

    let elapsed = start_time.elapsed().as_secs_f64();
    let speed = if elapsed > 0.0 {
        (chunk_size as f64 / 1024.0) / elapsed
    } else {
        0.0
    };

    log::info!(
        "分块上传成功: {} (大小: {} bytes, 块数: {}, 速度: {:.2} KB/s)",
        temp_filepath,
        chunk_size,
        chunk_count,
        speed
    );

    Ok(chunk_size)
}

pub async fn merge_chunk_files(
    state: web::Data<AppState>,
    info: ChunkUploadRequest,
) -> Result<FileInfo, String> {
    let module_path = format!("./uploads/{}", info.module);
    let temp_dir = format!("./temp/{}", info.module);

    // 构建最终文件路径
    let final_filepath = if let Some(rel_path) = &info.relative_path {
        let full_path = Path::new(&module_path).join(rel_path).join(&info.filename);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建子目录失败: {}", e))?;
        }
        full_path.to_string_lossy().to_string()
    } else {
        format!("{}/{}", module_path, info.filename)
    };

    log::info!("=== 开始合并分块文件 ===");
    log::info!("目标文件: {}", final_filepath);
    log::info!("总分块数: {}", info.total_chunks);

    // 确保模块目录存在
    std::fs::create_dir_all(&module_path)
        .map_err(|e| format!("创建模块目录失败: {}", e))?;

    // 获取文件级锁
    let file_lock_key = format!("{}_{}", info.module, info.filename);
    let file_lock = lock_utils::get_file_lock(&file_lock_key).await;

    let _fl = file_lock.lock().await;
    let start_time = Instant::now();

    // 执行合并
    let (total_merged_size, elapsed) = merge_chunks_internal(
        &final_filepath,
        &temp_dir,
        &info.filename,
        &info.relative_path,
        info.total_chunks,
    ).await?;

    let merge_speed = if elapsed > 0.0 {
        (total_merged_size as f64 / 1024.0 / 1024.0) / elapsed
    } else {
        0.0
    };

    // 获取文件信息
    let metadata = std::fs::metadata(&final_filepath)
        .map_err(|e| format!("获取文件元数据失败: {}", e))?;

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
        upload_time: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        size: metadata.len(),
        file_type: file_utils::get_file_type(&file_extension),
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

    Ok(file_info)
}

// 合并分块的内部实现
async fn merge_chunks_internal(
    final_path: &str,
    temp_dir: &str,
    filename: &str,
    relative_path: &Option<String>,
    total_chunks: usize,
) -> Result<(u64, f64), String> {
    use tokio::task::spawn_blocking;

    let final_path = final_path.to_string();
    let temp_dir = temp_dir.to_string();
    let filename = filename.to_string();
    let rel_clone = relative_path.clone();

    spawn_blocking(move || -> Result<(u64, f64), String> {
        let start_time = Instant::now();

        // 先写入临时最终文件
        let tmp_final = format!("{}.tmp.{}", final_path, Uuid::new_v4());
        let mut tmp_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_final)
            .map_err(|e| format!("创建临时文件失败: {}", e))?;

        let mut total_merged_size: u64 = 0;

        for i in 0..total_chunks {
            let temp_filename = if let Some(rel_path) = &rel_clone {
                let safe_path = rel_path.replace('/', "_").replace('\\', "_");
                format!("{}_{}.part{}", safe_path, filename, i)
            } else {
                format!("{}.part{}", filename, i)
            };

            let chunk_filepath = format!("{}/{}", temp_dir, temp_filename);

            if !Path::new(&chunk_filepath).exists() {
                let _ = std::fs::remove_file(&tmp_final);
                return Err(format!("分块 {} 不存在", i));
            }

            let mut chunk_file = std::fs::File::open(&chunk_filepath)
                .map_err(|e| format!("打开分块文件失败: {}", e))?;
            let chunk_size = chunk_file.metadata()
                .map_err(|e| format!("获取分块元数据失败: {}", e))?.len();
            
            std::io::copy(&mut chunk_file, &mut tmp_file)
                .map_err(|e| format!("合并分块失败: {}", e))?;

            total_merged_size += chunk_size;

            // 删除临时分片文件
            if let Err(e) = std::fs::remove_file(&chunk_filepath) {
                log::warn!("删除临时分片文件失败 {}: {}", chunk_filepath, e);
            }
        }

        tmp_file.sync_all()
            .map_err(|e| format!("同步文件失败: {}", e))?;

        // 原子重命名
        std::fs::rename(&tmp_final, &final_path)
            .map_err(|e| format!("重命名文件失败: {}", e))?;

        let elapsed = start_time.elapsed().as_secs_f64();
        log::info!("合并完成，耗时: {:.2}s", elapsed);
        Ok((total_merged_size, elapsed))
    }).await.map_err(|e| format!("合并任务失败: {}", e))?
}

pub async fn get_upload_progress(module: &str, filename: &str) -> Option<UploadProgress> {
    let progress_key = format!("{}_{}", module, filename);
    get_upload_manager().get_progress(&progress_key).await
}

// 清理过期的上传进度记录，返回清理的数量
pub async fn cleanup_expired_progress(max_age: Duration) -> usize {
    get_upload_manager().cleanup_expired(max_age).await
}

pub async fn check_file_exists(info: ResumeUploadRequest) -> Result<FileExistsResult, String> {
    let module_path = format!("./uploads/{}", info.module);
    let filepath = format!("{}/{}", module_path, info.filename);

    log::info!("检查文件是否存在: {}", filepath);

    if Path::new(&filepath).exists() {
        let metadata = std::fs::metadata(&filepath)
            .map_err(|e| format!("获取文件元数据失败: {}", e))?;

        if metadata.len() == info.total_size {
            log::info!("文件已存在，可秒传: {}", filepath);
            return Ok(FileExistsResult {
                exists: true,
                size: Some(metadata.len()),
                can_instant_upload: true,
                uploaded_chunks: Vec::new(),
                can_resume: false,
            });
        }
    }

    // 检查部分上传的分片
    let temp_dir = format!("./temp/{}", info.module);
    let mut uploaded_chunks = Vec::new();

    let part_re = regex::Regex::new(r"\.part(\d+)$").unwrap();
    if let Ok(entries) = std::fs::read_dir(&temp_dir) {
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
    let can_resume = !uploaded_chunks.is_empty();

    Ok(FileExistsResult {
        exists: false,
        size: None,
        can_instant_upload: false,
        uploaded_chunks,
        can_resume,
    })
}

#[derive(Debug, serde::Serialize)]
pub struct FileExistsResult {
    pub exists: bool,
    pub size: Option<u64>,
    pub can_instant_upload: bool,
    pub uploaded_chunks: Vec<usize>,
    pub can_resume: bool,
}