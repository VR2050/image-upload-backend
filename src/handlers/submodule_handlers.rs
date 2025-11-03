use actix_web::{web, HttpResponse};
use crate::{models::{ApiResponse}, state::AppState};
use crate::services::file_service;
use crate::utils::validation_utils;

#[derive(serde::Deserialize)]
pub struct CreateSubmoduleRequest {
    pub name: String,
}

pub async fn create_submodule(
    state: web::Data<AppState>,
    path: web::Path<String>,
    info: web::Json<CreateSubmoduleRequest>,
) -> HttpResponse {
    state.record_request();

    let module = path.into_inner();
    let sub_name = info.name.trim();

    if sub_name.is_empty() {
        state.record_error();
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "子模块名称不能为空".to_string(),
            data: None,
        });
    }

    if !validation_utils::is_valid_module_name(&module) || !validation_utils::is_valid_module_name(sub_name) {
        state.record_error();
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "模块或子模块名称包含非法字符".to_string(),
            data: None,
        });
    }

    match file_service::create_submodule_directory(&module, sub_name).await {
        Ok(_) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("子模块 '{}' 在模块 '{}' 下创建成功", sub_name, module),
            data: Some(sub_name.to_string()),
        }),
        Err(e) => {
            log::error!("创建子模块失败: {}", e);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: format!("创建子模块失败: {}", e),
                data: None,
            })
        }
    }
}

pub async fn get_submodules(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    state.record_request();

    let module = path.into_inner();

    if !validation_utils::is_valid_module_name(&module) {
        state.record_error();
        return HttpResponse::BadRequest().json(ApiResponse::<Vec<String>> {
            success: false,
            message: "模块名称包含非法字符".to_string(),
            data: None,
        });
    }

    match file_service::get_submodules(&module).await {
        Ok(subs) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: "获取子模块成功".to_string(),
            data: Some(subs),
        }),
        Err(e) => {
            log::error!("获取子模块失败: {}", e);
            state.record_error();
            HttpResponse::InternalServerError().json(ApiResponse::<Vec<String>> {
                success: false,
                message: format!("获取子模块失败: {}", e),
                data: None,
            })
        }
    }
}
