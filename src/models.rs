use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Module {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub message: String,
    pub data: Option<T>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileInfo {
    pub filename: String,
    pub url: String,
    pub module: String,
    pub upload_time: String,
    pub size: u64,
    pub file_type: String,
    pub relative_path: Option<String>,
    pub file_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub name: String,
    pub file_count: usize,
    pub created_time: String,
    pub total_size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChunkUploadRequest {
    pub chunk_number: usize,
    pub total_chunks: usize,
    pub filename: String,
    pub module: String,
    pub chunk_size: usize,
    pub relative_path: Option<String>,
    pub file_hash: Option<String>,
    pub chunk_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChunkUploadResponse {
    pub success: bool,
    pub message: String,
    pub chunk_number: usize,
    pub total_chunks: usize,
    pub filename: String,
    pub next_chunk: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FolderUploadRequest {
    pub module: String,
    pub folder_name: String,
    pub files: Vec<FolderFileInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FolderFileInfo {
    pub filename: String,
    pub relative_path: String,
    pub size: u64,
    pub file_type: String,
    pub file_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UploadProgress {
    pub filename: String,
    pub module: String,
    pub uploaded_chunks: usize,
    pub total_chunks: usize,
    pub total_size: u64,
    pub uploaded_size: u64,
    pub speed: f64,
    pub estimated_time: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResumeUploadRequest {
    pub filename: String,
    pub module: String,
    pub file_hash: String,
    pub total_size: u64,
}