use actix_web::web;
use chrono::Utc;
use crate::state::{AppState, SERVER_START_TIME};
use crate::utils::lock_utils;

pub async fn get_health_info(state: web::Data<AppState>) -> serde_json::Value {
    let resources = monitor_system_resources().await;
    let app_stats = state.get_stats();
    
    serde_json::json!({
        "status": "healthy",
        "timestamp": Utc::now().to_rfc3339(),
        "resources": resources,
        "app_stats": app_stats,
    })
}

pub async fn get_system_stats(state: web::Data<AppState>) -> Result<serde_json::Value, String> {
    let stats = tokio::task::spawn_blocking(|| -> Result<serde_json::Value, String> {
        let uploads_dir = "./uploads";
        let temp_dir = "./temp";
        let mut total_modules = 0usize;
        let mut total_files = 0usize;
        let mut total_size = 0u64;
        let mut temp_files_count = 0usize;
        let mut temp_files_size = 0u64;

        // 统计上传文件
        if let Ok(entries) = std::fs::read_dir(uploads_dir) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name != "." && name != ".." {
                            total_modules += 1;
                            let _ = crate::utils::file_utils::count_files_recursive(
                                &entry.path(), &mut total_files, &mut total_size
                            );
                        }
                    }
                }
            }
        }

        // 统计临时文件
        if let Ok(entries) = std::fs::read_dir(temp_dir) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        if let Ok(files) = std::fs::read_dir(entry.path()) {
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
        });

        Ok(stats)
    }).await.map_err(|e| format!("阻塞任务失败: {}", e))??;

    // 合并应用状态统计
    let mut stats_value = stats;
    if let Some(obj) = stats_value.as_object_mut() {
        obj.extend(state.get_stats().as_object().unwrap().clone());
    }

    Ok(stats_value)
}

// 系统资源监控
pub async fn monitor_system_resources() -> serde_json::Value {
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

    let file_locks_count = lock_utils::get_file_lock_count().await;

    serde_json::json!({
        "timestamp": Utc::now().to_rfc3339(),
        "file_locks_count": file_locks_count,
        "uptime_seconds": Utc::now().timestamp() as u64 - SERVER_START_TIME.load(std::sync::atomic::Ordering::Relaxed),
        "memory_usage": memory_info,
    })
}