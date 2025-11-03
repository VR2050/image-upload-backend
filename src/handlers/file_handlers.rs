use actix_web::{web, HttpResponse};
use crate::{models::ApiResponse, state::AppState};
use crate::services::file_service;
use crate::utils::validation_utils;

pub async fn get_module_files(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    state.record_request();
    
    let module = path.into_inner();
    
    log::info!("获取模块文件列表: {}", module);

    match file_service::get_module_files(&module).await {
        Ok(files) => {
            log::info!("找到 {} 个文件", files.len());
            HttpResponse::Ok().json(ApiResponse {
                success: true,
                message: format!("获取模块 '{}' 的文件列表成功", module),
                data: Some(files),
            })
        }
        Err(e) => {
            log::error!("获取模块文件失败: {}", e);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<Vec<crate::models::FileInfo>> {
                success: false,
                message: e,
                data: None,
            })
        }
    }
}

pub async fn delete_file(
    state: web::Data<AppState>,
    path: web::Path<(String, String)>,
) -> HttpResponse {
    state.record_request();
    
    let (module, filename) = path.into_inner();

    if !validation_utils::is_valid_filename(&filename) {
        state.record_error();
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "文件名包含非法字符".to_string(),
            data: None,
        });
    }

    match file_service::delete_file(&module, &filename).await {
        Ok(_) => {
            log::info!("文件删除成功: {}/{}", module, filename);
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

pub async fn delete_folder(
    state: web::Data<AppState>,
    path: web::Path<(String, String)>,
) -> HttpResponse {
    state.record_request();
    
    let (module, folder_path) = path.into_inner();

    if !validation_utils::is_valid_path(&folder_path) {
        state.record_error();
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "文件夹路径包含非法字符".to_string(),
            data: None,
        });
    }

    match file_service::delete_folder(&module, &folder_path).await {
        Ok(_) => {
            log::info!("文件夹删除成功: {}/{}", module, folder_path);
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