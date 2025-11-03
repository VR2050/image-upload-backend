use std::time::Duration;
use std::fs;
use crate::utils::lock_utils;

pub async fn start_background_cleanup() {
    let mut cleanup_interval = tokio::time::interval(Duration::from_secs(1800)); // 30分钟
    
    loop {
        cleanup_interval.tick().await;
        
        log::info!("执行后台清理任务...");
        
        // 清理过期的文件锁
        let locks_cleaned = lock_utils::cleanup_file_locks().await;
    // 清理过期的上传进度记录（6小时）
    let progress_cleaned = crate::services::upload_service::cleanup_expired_progress(std::time::Duration::from_secs(6 * 3600)).await;
        
        // 清理临时文件
        let (files_cleaned, size_freed) = cleanup_temp_files_internal().await
            .unwrap_or((0, 0));
        
    log::info!("清理完成 - 文件锁: {}, 已清理上传进度: {}, 临时文件: {} (释放 {} bytes)", 
          locks_cleaned, progress_cleaned, files_cleaned, size_freed);
    }
}

pub async fn cleanup_temp_files() -> Result<(usize, u64), String> {
    cleanup_temp_files_internal().await
}

async fn cleanup_temp_files_internal() -> Result<(usize, u64), String> {
    tokio::task::spawn_blocking(|| -> Result<(usize, u64), String> {
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
    }).await.map_err(|e| format!("清理任务失败: {}", e))?
}

pub async fn graceful_shutdown() {
    log::info!("接收到关闭信号，开始优雅关闭...");
    
    // 执行清理操作
    log::info!("清理文件锁...");
    let locks_cleaned = lock_utils::cleanup_file_locks().await;
    
    log::info!("清理临时文件...");
    let _ = cleanup_temp_files_internal().await;
    
    log::info!("优雅关闭完成 - 清理文件锁: {}", locks_cleaned);
}