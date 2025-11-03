use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub chunk_size: usize,
    pub max_concurrent_chunks: usize,
    pub max_file_size: u64,
    pub temp_file_cleanup_interval: Duration,
    pub global_max_concurrent: usize,
    pub max_memory_locks: usize,
    pub lock_cleanup_interval: Duration,
    pub merge_max_concurrent: usize,
    pub address: String,
    pub port: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            chunk_size: 5 * 1024 * 1024, // 5MB
            max_concurrent_chunks: 3,
            max_file_size: 10 * 1024 * 1024 * 1024, // 10GB
            temp_file_cleanup_interval: Duration::from_secs(3600),
            global_max_concurrent: 64,
            merge_max_concurrent: 4,
            max_memory_locks: 10000,
            lock_cleanup_interval: Duration::from_secs(1800),
            address: "127.0.0.1".to_string(),
            port: "2233".to_string(),
        }
    }
}

impl ServerConfig {
    pub fn new() -> Self {
        let args: Vec<String> = std::env::args().collect();
        // args[0] is executable path; optional args: address, port
        let address = args.get(1).cloned().unwrap_or_else(|| "127.0.0.1".to_string());
        let port = args.get(2).cloned().unwrap_or_else(|| "2233".to_string());

        Self { address, port, ..Default::default() }
    }

    pub async fn init_directories(&self) -> std::io::Result<()> {
        tokio::fs::create_dir_all("./uploads").await?;
        tokio::fs::create_dir_all("./uploads/default").await?;
        tokio::fs::create_dir_all("./temp").await?;
        tokio::fs::create_dir_all("./frontend").await.ok(); // 前端目录可选
        
        Ok(())
    }

    pub fn log_config(&self) {
        log::info!("配置信息:");
        log::info!("  - 分片大小: {}MB", self.chunk_size / 1024 / 1024);
        log::info!("  - 最大文件大小: {}GB", self.max_file_size / 1024 / 1024 / 1024);
        log::info!("  - 最大并发分片数: {}", self.max_concurrent_chunks);
        log::info!("  - 全局并发限制: {}", self.global_max_concurrent);
        log::info!("  - 合并并发限制: {}", self.merge_max_concurrent);
        log::info!("上传目录: ./uploads/");
        log::info!("临时目录: ./temp/");
    }
}