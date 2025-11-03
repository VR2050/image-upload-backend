use std::path::{Path, PathBuf};
use std::fs;
use chrono::{DateTime, Utc};
use crate::models::{FileInfo, ModuleInfo};

// 检查文件扩展名是否为有效的文件格式
pub fn is_valid_file_extension(ext: &str) -> bool {
    let ext_lower = ext.to_lowercase();
    matches!(
        ext_lower.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "ico" |
        "zip" | "rar" | "7z" | "tar" | "gz" |
        "pdf" | "doc" | "docx" | "txt" | "md" | "json" | "xml" | "csv" | "xls" | "xlsx" | "ppt" | "pptx" |
        "mp4" | "avi" | "mov" | "wmv" | "flv" | "mkv" |
        "mp3" | "wav" | "ogg" | "flac"
    )
}

// 获取文件类型分类
pub fn get_file_type(ext: &str) -> String {
    let ext_lower = ext.to_lowercase();
    match ext_lower.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "ico" => "image".to_string(),
        "zip" | "rar" | "7z" | "tar" | "gz" => "archive".to_string(),
        "pdf" | "doc" | "docx" | "txt" | "md" | "json" | "xml" | "csv" | "xls" | "xlsx" | "ppt" | "pptx" => "document".to_string(),
        "mp4" | "avi" | "mov" | "wmv" | "flv" | "mkv" => "video".to_string(),
        "mp3" | "wav" | "ogg" | "flac" => "audio".to_string(),
        _ => "other".to_string(),
    }
}

// 递归统计文件数量和大小
pub fn count_files_recursive(
    path: &Path,
    file_count: &mut usize,
    total_size: &mut u64,
) -> std::io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;

        if file_type.is_file() {
            *file_count += 1;
            if let Ok(metadata) = entry.metadata() {
                *total_size += metadata.len();
            }
        } else if file_type.is_dir() {
            count_files_recursive(&entry.path(), file_count, total_size)?;
        }
    }
    Ok(())
}

// 递归收集文件信息
pub fn collect_files_recursive(
    base_path: &Path,
    current_path: &str,
    files: &mut Vec<FileInfo>,
    module: &str,
) -> std::io::Result<()> {
    let full_path = if current_path.is_empty() {
        base_path.to_path_buf()
    } else {
        base_path.join(current_path)
    };

    for entry in fs::read_dir(full_path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;

        if file_type.is_file() {
            let path = entry.path();
            if let Some(file_name) = path.file_name() {
                let filename = file_name.to_string_lossy().to_string();

                let metadata = entry.metadata()?;
                let size = metadata.len();
                let created = metadata
                    .created()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                let upload_time: DateTime<Utc> = created.into();

                let file_extension = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();

                let relative_path = if current_path.is_empty() {
                    None
                } else {
                    Some(current_path.to_string())
                };

                let url = if let Some(rel_path) = &relative_path {
                    format!("/uploads/{}/{}/{}", module, rel_path, filename)
                } else {
                    format!("/uploads/{}/{}", module, filename)
                };

                let file_info = FileInfo {
                    filename: filename.clone(),
                    url,
                    module: module.to_string(),
                    upload_time: upload_time.format("%Y-%m-%d %H:%M:%S").to_string(),
                    size,
                    file_type: get_file_type(&file_extension),
                    relative_path,
                    file_hash: None,
                };
                files.push(file_info);
            }
        } else if file_type.is_dir() {
            let dir_name = entry.file_name().to_string_lossy().to_string();
            let new_path = if current_path.is_empty() {
                dir_name
            } else {
                format!("{}/{}", current_path, dir_name)
            };
            collect_files_recursive(base_path, &new_path, files, module)?;
        }
    }
    Ok(())
}

// 生成唯一的文件名
pub fn generate_unique_filename(original_filename: &str, filepath: &str) -> String {
    let path = Path::new(filepath);
    if !path.exists() {
        return filepath.to_string();
    }

    let file_extension = Path::new(original_filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let stem = Path::new(original_filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file");

    let parent = path.parent().unwrap_or(Path::new("."));
    
    let mut counter = 1;
    loop {
        let new_name = if file_extension.is_empty() {
            format!("{}_{}", stem, counter)
        } else {
            format!("{}_{}.{}", stem, counter, file_extension)
        };
        
        let new_path = parent.join(&new_name);
        
        if !new_path.exists() {
            return new_path.to_string_lossy().to_string();
        }
        counter += 1;
    }
}

// 获取模块信息
pub fn get_module_info(entry: &fs::DirEntry) -> std::io::Result<ModuleInfo> {
    let name = entry.file_name().to_string_lossy().to_string();
    let module_path = entry.path();
    let mut file_count = 0;
    let mut total_size = 0;

    let _ = count_files_recursive(&module_path, &mut file_count, &mut total_size);

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

    Ok(ModuleInfo {
        name,
        file_count,
        created_time,
        total_size,
    })
}