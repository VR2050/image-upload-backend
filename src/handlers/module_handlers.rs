use actix_web::{web, HttpResponse};
use crate::{models::{Module, ApiResponse, ModuleInfo}, state::AppState};
use crate::services::file_service;
use crate::utils::validation_utils;

pub async fn create_module(
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

    if !validation_utils::is_valid_module_name(module_name) {
        state.record_error();
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "模块名称包含非法字符".to_string(),
            data: None,
        });
    }

    match file_service::create_module_directory(module_name).await {
        Ok(_) => {
            log::info!("模块 '{}' 创建成功", module_name);
            HttpResponse::Ok().json(ApiResponse {
                success: true,
                message: format!("模块 '{}' 创建成功", module_name),
                data: Some(module_name.to_string()),
            })
        }
        Err(e) => {
            log::error!("创建模块失败: {}", e);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: format!("创建模块失败: {}", e),
                data: None,
            })
        }
    }
}

pub async fn get_modules(state: web::Data<AppState>) -> HttpResponse {
    state.record_request();
    
    match file_service::get_all_modules_info().await {
        Ok(modules_info) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: "获取模块列表成功".to_string(),
            data: Some(modules_info),
        }),
        Err(e) => {
            log::error!("获取模块列表失败: {}", e);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<Vec<ModuleInfo>> {
                success: false,
                message: format!("获取模块列表失败: {}", e),
                data: None,
            })
        }
    }
}

pub async fn delete_module(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    state.record_request();
    
    let module = path.into_inner();

    if !validation_utils::is_valid_module_name(&module) {
        state.record_error();
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "模块名称包含非法字符".to_string(),
            data: None,
        });
    }

    match file_service::delete_module(&module).await {
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