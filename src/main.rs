use actix_files::Files;
use actix_multipart::Multipart;
use actix_web::{middleware::Logger, web, App, Error, HttpResponse, HttpServer};
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use uuid::Uuid;

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
struct ImageInfo {
    filename: String,
    url: String,
    module: String,
    upload_time: String,
    size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ModuleInfo {
    name: String,
    image_count: usize,
    created_time: String,
}

// 检查文件扩展名是否为有效的图片格式
fn is_valid_image_extension(ext: &str) -> bool {
    matches!(
        ext.to_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "ico"
    )
}

// 创建模块目录
async fn create_module(module: web::Json<Module>) -> HttpResponse {
    let module_name = module.name.trim();

    if module_name.is_empty() {
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "模块名称不能为空".to_string(),
            data: None,
        });
    }

    // 检查模块名称是否合法
    if module_name.contains("..") || module_name.contains("/") || module_name.contains("\\") {
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "模块名称包含非法字符".to_string(),
            data: None,
        });
    }

    let module_path = format!("./uploads/{}", module_name);

    match fs::create_dir_all(&module_path) {
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
            HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: format!("创建模块失败: {}", e),
                data: None,
            })
        }
    }
}

// 获取所有模块的详细信息
async fn get_modules() -> HttpResponse {
    let uploads_dir = "./uploads";

    match fs::read_dir(uploads_dir) {
        Ok(entries) => {
            let mut modules_info = Vec::new();

            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name != "." && name != ".." {
                            let module_path = entry.path();

                            // 获取图片数量
                            let image_count = match fs::read_dir(&module_path) {
                                Ok(images) => images
                                    .flatten()
                                    .filter(|e| {
                                        if let Ok(ft) = e.file_type() {
                                            if ft.is_file() {
                                                if let Some(ext) = e.path().extension() {
                                                    return is_valid_image_extension(
                                                        ext.to_string_lossy().as_ref(),
                                                    );
                                                }
                                            }
                                        }
                                        false
                                    })
                                    .count(),
                                Err(_) => 0,
                            };

                            // 获取创建时间
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
                                image_count,
                                created_time,
                            });
                        }
                    }
                }
            }

            HttpResponse::Ok().json(ApiResponse {
                success: true,
                message: "获取模块列表成功".to_string(),
                data: Some(modules_info),
            })
        }
        Err(e) => {
            log::error!("读取模块目录失败: {}", e);
            HttpResponse::InternalServerError().json(ApiResponse::<Vec<ModuleInfo>> {
                success: false,
                message: format!("读取模块目录失败: {}", e),
                data: None,
            })
        }
    }
}

// 上传图片
async fn upload_image(
    mut payload: Multipart,
    web::Query(params): web::Query<HashMap<String, String>>,
) -> Result<HttpResponse, Error> {
    let module = params
        .get("module")
        .unwrap_or(&"default".to_string())
        .clone();
    let module_path = format!("./uploads/{}", module);

    // 确保模块目录存在
    if let Err(e) = fs::create_dir_all(&module_path) {
        return Ok(HttpResponse::InternalServerError().json(ApiResponse::<()> {
            success: false,
            message: format!("创建模块目录失败: {}", e),
            data: None,
        }));
    }

    let mut uploaded_files = Vec::new();
    let current_time = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    while let Some(mut field) = payload.try_next().await? {
        let content_disposition = field.content_disposition();
        let original_filename = content_disposition
            .as_ref()
            .and_then(|cd| cd.get_filename())
            .unwrap_or("unknown")
            .to_string();

        // 获取文件扩展名
        let file_extension = std::path::Path::new(&original_filename)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("jpg")
            .to_lowercase();

        // 检查文件类型
        if !is_valid_image_extension(&file_extension) {
            log::warn!("跳过不支持的文件类型: {}", original_filename);
            continue; // 跳过非图片文件
        }

        // 生成唯一文件名
        let unique_filename = format!("{}.{}", Uuid::new_v4(), file_extension);
        let filepath = format!("{}/{}", module_path, unique_filename);

        // 创建文件并写入内容 - 使用 move 关键字
        let filepath_clone = filepath.clone();
        let mut file = match web::block(move || std::fs::File::create(&filepath_clone)).await {
            Ok(Ok(file)) => file,
            Ok(Err(e)) => {
                log::error!("创建文件失败: {}", e);
                continue;
            }
            Err(e) => {
                log::error!("阻塞操作失败: {}", e);
                continue;
            }
        };

        let mut total_size = 0;
        // 写入文件内容并计算大小
        while let Some(chunk) = field.try_next().await? {
            let chunk_size = chunk.len();
            total_size += chunk_size as u64;

            if let Err(e) = file.write_all(&chunk) {
                log::error!("写入文件失败: {}", e);
                continue;
            }
        }

        // 记录上传的文件信息
        let image_info = ImageInfo {
            filename: unique_filename.clone(),
            url: format!("/uploads/{}/{}", module, unique_filename),
            module: module.clone(),
            upload_time: current_time.clone(),
            size: total_size,
        };

        uploaded_files.push(image_info);
        log::info!("文件上传成功: {} (大小: {} bytes)", filepath, total_size);
    }

    if uploaded_files.is_empty() {
        Ok(HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "没有有效的图片文件上传".to_string(),
            data: None,
        }))
    } else {
        Ok(HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("成功上传 {} 张图片", uploaded_files.len()),
            data: Some(uploaded_files),
        }))
    }
}

// 获取模块中的图片列表
async fn get_module_images(path: web::Path<String>) -> HttpResponse {
    let module = path.into_inner();
    let module_path = format!("./uploads/{}", module);
    let mut images = Vec::new();

    match fs::read_dir(module_path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_file() {
                        let path = entry.path();
                        if let Some(extension) = path.extension() {
                            let ext = extension.to_string_lossy().to_lowercase();
                            if is_valid_image_extension(&ext) {
                                if let Some(file_name) = path.file_name() {
                                    let filename = file_name.to_string_lossy().to_string();

                                    // 获取文件元数据
                                    let metadata = match entry.metadata() {
                                        Ok(meta) => meta,
                                        Err(_) => continue,
                                    };
                                    let size = metadata.len();
                                    let created = metadata
                                        .created()
                                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                                    let upload_time: DateTime<Utc> = created.into();

                                    let image_info = ImageInfo {
                                        filename: filename.clone(),
                                        url: format!("/uploads/{}/{}", module, filename),
                                        module: module.clone(),
                                        upload_time: upload_time
                                            .format("%Y-%m-%d %H:%M:%S")
                                            .to_string(),
                                        size,
                                    };
                                    images.push(image_info);
                                }
                            }
                        }
                    }
                }
            }

            // 按上传时间倒序排列
            images.sort_by(|a, b| b.upload_time.cmp(&a.upload_time));

            HttpResponse::Ok().json(ApiResponse {
                success: true,
                message: format!("获取模块 '{}' 的图片列表成功", module),
                data: Some(images),
            })
        }
        Err(e) => {
            log::error!("读取模块图片失败: {}", e);
            HttpResponse::NotFound().json(ApiResponse::<Vec<ImageInfo>> {
                success: false,
                message: format!("模块不存在或读取失败: {}", e),
                data: None,
            })
        }
    }
}

// 删除图片
async fn delete_image(path: web::Path<(String, String)>) -> HttpResponse {
    let (module, filename) = path.into_inner();
    let file_path = format!("./uploads/{}/{}", module, filename);

    // 安全检查：防止路径遍历攻击
    if filename.contains("..") || filename.contains("/") || filename.contains("\\") {
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "文件名包含非法字符".to_string(),
            data: None,
        });
    }

    match fs::remove_file(&file_path) {
        Ok(_) => {
            log::info!("图片删除成功: {}", file_path);
            HttpResponse::Ok().json(ApiResponse::<()> {
                success: true,
                message: "图片删除成功".to_string(),
                data: None,
            })
        }
        Err(e) => {
            log::error!("删除图片失败: {}", e);
            HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: format!("删除图片失败: {}", e),
                data: None,
            })
        }
    }
}

// 删除模块
async fn delete_module(path: web::Path<String>) -> HttpResponse {
    let module = path.into_inner();
    let module_path = format!("./uploads/{}", module);

    // 安全检查
    if module.contains("..") || module.contains("/") || module.contains("\\") {
        return HttpResponse::BadRequest().json(ApiResponse::<()> {
            success: false,
            message: "模块名称包含非法字符".to_string(),
            data: None,
        });
    }

    match fs::remove_dir_all(&module_path) {
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
            HttpResponse::InternalServerError().json(ApiResponse::<()> {
                success: false,
                message: format!("删除模块失败: {}", e),
                data: None,
            })
        }
    }
}

// 健康检查
async fn health_check() -> HttpResponse {
    HttpResponse::Ok().json(ApiResponse::<()> {
        success: true,
        message: "服务运行正常".to_string(),
        data: None,
    })
}

// 获取系统统计信息
async fn get_stats() -> HttpResponse {
    let uploads_dir = "./uploads";
    let mut total_modules = 0;
    let mut total_images = 0;
    let mut total_size = 0;

    if let Ok(entries) = fs::read_dir(uploads_dir) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name != "." && name != ".." {
                        total_modules += 1;

                        // 统计模块内的图片
                        if let Ok(images) = fs::read_dir(entry.path()) {
                            for image in images.flatten() {
                                if let Ok(metadata) = image.metadata() {
                                    if metadata.is_file() {
                                        if let Some(ext) = image.path().extension() {
                                            if is_valid_image_extension(
                                                ext.to_string_lossy().as_ref(),
                                            ) {
                                                total_images += 1;
                                                total_size += metadata.len();
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

    let stats = serde_json::json!({
        "total_modules": total_modules,
        "total_images": total_images,
        "total_size": total_size,
        "total_size_mb": (total_size as f64 / 1024.0 / 1024.0).round() as u64
    });

    HttpResponse::Ok().json(ApiResponse {
        success: true,
        message: "获取统计信息成功".to_string(),
        data: Some(stats),
    })
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // 初始化日志
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    // 创建上传目录
    fs::create_dir_all("./uploads").unwrap_or_else(|e| {
        log::warn!("创建上传目录失败: {}, 但继续启动", e);
    });

    fs::create_dir_all("./uploads/default").unwrap_or_else(|e| {
        log::warn!("创建默认模块目录失败: {}", e);
    });

    // 创建前端目录
    fs::create_dir_all("./frontend").unwrap_or_else(|e| {
        log::warn!("创建前端目录失败: {}", e);
    });

    log::info!("启动图片上传管理系统...");
    log::info!("服务器将运行在 http://127.0.0.1:8080");
    log::info!("上传目录: ./uploads/");

    HttpServer::new(|| {
        App::new()
            .wrap(Logger::default())
            // API 路由
            .service(
                web::scope("/api")
                    .route("/health", web::get().to(health_check))
                    .route("/stats", web::get().to(get_stats))
                    .route("/modules", web::get().to(get_modules))
                    .route("/modules", web::post().to(create_module))
                    .route("/modules/{module}", web::delete().to(delete_module))
                    .route("/upload", web::post().to(upload_image))
                    .route("/images/{module}", web::get().to(get_module_images))
                    .route("/image/{module}/{filename}", web::delete().to(delete_image)),
            )
            // 静态文件服务 - 提供上传的图片访问
            .service(
                Files::new("/uploads", "./uploads")
                    .show_files_listing()
                    .use_last_modified(true),
            )
            // 前端静态文件服务
            .service(
                Files::new("/", "./frontend")
                    .index_file("index.html")
                    .prefer_utf8(true),
            )
    })
    .bind("10.11.2.32:2233")?
    .run()
    .await
}
