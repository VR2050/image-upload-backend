use actix_web::{web, HttpResponse};
use crate::{models::ApiResponse, state::AppState};
use crate::services::{system_service, cleanup_service};

pub async fn health_check(state: web::Data<AppState>) -> HttpResponse {
    state.record_request();
    
    let health_info = system_service::get_health_info(state.clone()).await;
    
    HttpResponse::Ok().json(ApiResponse {
        success: true,
        message: "服务运行正常".to_string(),
        data: Some(health_info),
    })
}

pub async fn get_stats(state: web::Data<AppState>) -> HttpResponse {
    state.record_request();
    
    match system_service::get_system_stats(state.clone()).await {
        Ok(stats) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: "获取统计信息成功".to_string(),
            data: Some(stats),
        }),
        Err(e) => {
            log::error!("获取统计信息失败: {}", e);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: e,
                data: None,
            })
        }
    }
}

pub async fn cleanup_temp_files(state: web::Data<AppState>) -> HttpResponse {
    state.record_request();
    
    match cleanup_service::cleanup_temp_files().await {
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