use actix_web::{web, HttpResponse, Error};
use actix_multipart::Multipart;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use crate::{
    models::{ApiResponse, ChunkUploadRequest, ResumeUploadRequest}, 
    state::{AppState, ACTIVE_UPLOADS},
    utils::validation_utils
};
use crate::services::upload_service;

pub async fn upload_file(
    state: web::Data<AppState>,
    payload: Multipart,
    params: web::Query<HashMap<String, String>>,
) -> Result<HttpResponse, Error> {
    state.record_request();
    
    let _permit = state.global_semaphore.acquire().await
        .map_err(|e| {
            log::error!("获取全局并发许可失败: {}", e);
            actix_web::error::ErrorServiceUnavailable("服务器繁忙，请稍后重试")
        })?;

    ACTIVE_UPLOADS.fetch_add(1, Ordering::Relaxed);
    
    let result = upload_service::handle_file_upload(state.clone(), payload, params).await;
    
    ACTIVE_UPLOADS.fetch_sub(1, Ordering::Relaxed);
    
    result
}

pub async fn upload_chunk(
    state: web::Data<AppState>,
    payload: Multipart,
    params: web::Query<HashMap<String, String>>,
) -> Result<HttpResponse, Error> {
    state.record_request();
    
    let _permit = state.global_semaphore.acquire().await
        .map_err(|e| {
            log::error!("获取全局并发许可失败: {}", e);
            actix_web::error::ErrorServiceUnavailable("服务器繁忙，请稍后重试")
        })?;

    ACTIVE_UPLOADS.fetch_add(1, Ordering::Relaxed);
    
    let result = upload_service::handle_chunk_upload(state.clone(), payload, params).await;
    
    ACTIVE_UPLOADS.fetch_sub(1, Ordering::Relaxed);
    
    result
}

pub async fn merge_chunks(
    state: web::Data<AppState>,
    info: web::Json<ChunkUploadRequest>,
) -> HttpResponse {
    state.record_request();
    // 限制并发合并，优先使用专用的 MERGE_SEMAPHORE，若未初始化则退回到全局信号量
    if let Some(sem) = crate::utils::lock_utils::get_merge_semaphore() {
        let _permit = sem.acquire().await
            .map_err(|e| {
                log::error!("获取合并并发许可失败: {}", e);
                actix_web::error::ErrorServiceUnavailable("服务器繁忙，请稍后重试")
            });
    } else {
        let _permit = state.global_semaphore.acquire().await
            .map_err(|e| {
                log::error!("获取全局并发许可失败 (merge fallback): {}", e);
                actix_web::error::ErrorServiceUnavailable("服务器繁忙，请稍后重试")
            });
    }
    ACTIVE_UPLOADS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    
    if !validation_utils::is_valid_filename(&info.filename) {
        state.record_error();
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "文件名包含非法字符".to_string(),
            data: None,
        });
    }

    match upload_service::merge_chunk_files(state.clone(), info.into_inner()).await {
        Ok(file_info) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: "文件合并成功".to_string(),
            data: Some(file_info),
        }),
        Err(e) => {
            log::error!("合并文件失败: {}", e);
            state.record_error();
            ACTIVE_UPLOADS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: e,
                data: None,
            })
        }
    }
}

pub async fn get_upload_progress(
    state: web::Data<AppState>,
    path: web::Path<(String, String)>,
) -> HttpResponse {
    state.record_request();
    
    let (module, filename) = path.into_inner();
    
    match upload_service::get_upload_progress(&module, &filename).await {
        Some(progress) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: "获取上传进度成功".to_string(),
            data: Some(progress),
        }),
        None => HttpResponse::NotFound().json(ApiResponse::<crate::models::UploadProgress> {
            success: false,
            message: "未找到上传进度".to_string(),
            data: None,
        })
    }
}

pub async fn check_file_exists(
    state: web::Data<AppState>,
    info: web::Json<ResumeUploadRequest>,
) -> HttpResponse {
    state.record_request();
    
    match upload_service::check_file_exists(info.into_inner()).await {
        Ok(result) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: if result.exists { "文件已存在" } else { "文件不存在" }.to_string(),
            data: Some(result),
        }),
        Err(e) => {
            log::error!("检查文件存在失败: {}", e);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: e,
                data: None,
            })
        }
    }
}