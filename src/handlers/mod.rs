pub mod module_handlers;
pub mod file_handlers;
pub mod upload_handlers;
pub mod system_handlers;
pub mod submodule_handlers;

use actix_web::web;

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api")
            .route("/health", web::get().to(system_handlers::health_check))
            .route("/stats", web::get().to(system_handlers::get_stats))
            .route("/modules", web::get().to(module_handlers::get_modules))
            .route("/modules", web::post().to(module_handlers::create_module))
            .route("/modules/{module}", web::delete().to(module_handlers::delete_module))
            .route("/modules/{module}/submodules", web::post().to(submodule_handlers::create_submodule))
            .route("/modules/{module}/submodules", web::get().to(submodule_handlers::get_submodules))
            .route("/upload", web::post().to(upload_handlers::upload_file))
            .route("/upload/chunk", web::post().to(upload_handlers::upload_chunk))
            .route("/upload/merge", web::post().to(upload_handlers::merge_chunks))
            .route("/upload/progress/{module}/{filename}", web::get().to(upload_handlers::get_upload_progress))
            .route("/upload/check", web::post().to(upload_handlers::check_file_exists))
            .route("/cleanup", web::post().to(system_handlers::cleanup_temp_files))
            .route("/files/{module:.*}", web::get().to(file_handlers::get_module_files))
            .route("/file/{module:.*}/{filename}", web::delete().to(file_handlers::delete_file))
            .route(
                "/folder/{module}/{folder_path:.*}",
                web::delete().to(file_handlers::delete_folder),
            ),
    )
    .service(
        actix_files::Files::new("/uploads", "./uploads")
            .show_files_listing()
            .use_last_modified(true),
    )
    .service(
        actix_files::Files::new("/", "./frontend")
            .index_file("index.html")
            .prefer_utf8(true),
    );
}