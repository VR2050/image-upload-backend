mod config;
mod models;

mod state;
mod handlers;
mod utils;
pub mod services;
use actix_web::{middleware::Logger, web, App, HttpServer};
use std::io::Result;

#[actix_web::main]
async fn main() -> Result<()> {
    // 初始化日志
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    // 初始化配置
    let config = config::ServerConfig::new();
    config.init_directories().await?;

    // 初始化全局并发控制
    utils::lock_utils::init_global_semaphore(config.global_max_concurrent);
    // 初始化合并并发控制
    utils::lock_utils::init_merge_semaphore(config.merge_max_concurrent);

    // 创建应用状态
    let app_state = state::AppState::new(config.global_max_concurrent);

    // 启动后台清理任务
    tokio::spawn(services::cleanup_service::start_background_cleanup());

    log::info!("启动优化的文件上传管理系统...");
    config.log_config();
    println!("服务器运行在：http://{}:{}", config.address, config.port);

    let server = HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(app_state.clone()))
            .wrap(Logger::default())
            .app_data(web::PayloadConfig::new(config.max_file_size as usize))
            .configure(handlers::configure_routes)
    })
    .bind(format!("{}:{}", config.address, config.port))?
    .run();

    // 设置优雅关闭
    let server_handle = server.handle();
    let shutdown_signal = async {
        tokio::signal::ctrl_c().await.expect("Failed to install CTRL+C handler");
        log::info!("接收到关闭信号");
    };

    tokio::select! {
        _ = server => {
            log::info!("服务器正常退出");
        }
        _ = shutdown_signal => {
            log::info!("开始优雅关闭流程");
            server_handle.stop(true).await;
            services::cleanup_service::graceful_shutdown().await;
            log::info!("优雅关闭完成");
        }
    }

    Ok(())
}