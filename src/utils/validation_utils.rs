// 验证模块名称
pub fn is_valid_module_name(name: &str) -> bool {
    !name.contains("..") && !name.contains("/") && !name.contains("\\")
}

// 允许包含 '/' 的模块路径（用于指定子模块路径），但不允许路径穿越或绝对路径
pub fn is_valid_module_path(path: &str) -> bool {
    if path.is_empty() { return false; }
    if path.contains("..") { return false; }
    if path.starts_with('/') || path.contains('\\') { return false; }
    // 禁止以 '.' 或空节段为模块名
    for seg in path.split('/') {
        if seg.trim().is_empty() { return false; }
        if seg == "." { return false; }
    }
    true
}

// 验证文件名
pub fn is_valid_filename(filename: &str) -> bool {
    !filename.contains("..") && !filename.contains("//")
}

// 验证路径
pub fn is_valid_path(path: &str) -> bool {
    !path.contains("..") && !path.contains("//")
}

// 验证文件大小
pub fn is_valid_file_size(size: u64, max_size: u64) -> bool {
    size <= max_size
}

// 验证分块参数
pub fn is_valid_chunk_params(chunk_number: usize, total_chunks: usize) -> bool {
    chunk_number < total_chunks
}