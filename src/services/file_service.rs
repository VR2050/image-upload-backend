use std::path::{Path, PathBuf};
use std::fs;
use tokio::fs as tokio_fs;
use crate::models::{FileInfo, ModuleInfo};
use crate::utils::file_utils;

pub async fn create_module_directory(module_name: &str) -> Result<(), String> {
    let module_path = format!("./uploads/{}", module_name);
    
    tokio_fs::create_dir_all(&module_path).await
        .map_err(|e| format!("创建模块目录失败: {}", e))?;

    let temp_dir = format!("./temp/{}", module_name);
    let _ = tokio_fs::create_dir_all(&temp_dir).await;

    Ok(())
}

pub async fn create_submodule_directory(module_name: &str, submodule_name: &str) -> Result<(), String> {
    // 创建 uploads/{module_name}/{submodule_name}
    let sub_path = format!("./uploads/{}/{}", module_name, submodule_name);
    tokio_fs::create_dir_all(&sub_path).await
        .map_err(|e| format!("创建子模块目录失败: {}", e))?;

    // 同步创建 temp 子目录
    let temp_sub = format!("./temp/{}/{}", module_name, submodule_name);
    let _ = tokio_fs::create_dir_all(&temp_sub).await;

    Ok(())
}

pub async fn get_all_modules_info() -> Result<Vec<ModuleInfo>, String> {
    let uploads_dir = "./uploads";
    
    let modules_info = tokio::task::spawn_blocking(move || -> Result<Vec<ModuleInfo>, String> {
        let mut modules_info = Vec::new();
        
        let entries = fs::read_dir(uploads_dir)
            .map_err(|e| format!("读取上传目录失败: {}", e))?;
            
        for entry in entries {
            let entry = entry.map_err(|e| format!("读取目录项失败: {}", e))?;
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name != "." && name != ".." {
                        let module_info = file_utils::get_module_info(&entry)
                            .map_err(|e| format!("获取模块信息失败: {}", e))?;
                        modules_info.push(module_info);
                    }
                }
            }
        }
        Ok(modules_info)
    }).await.map_err(|e| format!("阻塞任务失败: {}", e))??;

    Ok(modules_info)
}

pub async fn get_submodules(module: &str) -> Result<Vec<String>, String> {
    let module_path = format!("./uploads/{}", module);

    let submodules = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
        let mut subs = Vec::new();
        let entries = std::fs::read_dir(&module_path)
            .map_err(|e| format!("读取模块目录失败: {}", e))?;

        for entry in entries {
            let entry = entry.map_err(|e| format!("读取目录项失败: {}", e))?;
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    subs.push(name);
                }
            }
        }

        Ok(subs)
    }).await.map_err(|e| format!("阻塞任务失败: {}", e))??;

    Ok(submodules)
}

pub async fn get_module_files(module: &str) -> Result<Vec<FileInfo>, String> {
    let module_path = format!("./uploads/{}", module);
    
    if !Path::new(&module_path).exists() {
        return Err(format!("模块 '{}' 不存在", module));
    }

    // Move owned data into the blocking closure to avoid borrowing non-'static references
    let module_path_owned = module_path.clone();
    let module_owned = module.to_string();
    let files = tokio::task::spawn_blocking(move || -> Result<Vec<FileInfo>, String> {
        let mut files: Vec<FileInfo> = Vec::new();
        let base = PathBuf::from(module_path_owned);
        file_utils::collect_files_recursive(&base, "", &mut files, &module_owned)
            .map_err(|e| format!("收集文件失败: {}", e))?;
        
        files.sort_by(|a, b| b.upload_time.cmp(&a.upload_time));
        Ok(files)
    }).await.map_err(|e| format!("阻塞任务失败: {}", e))??;

    Ok(files)
}

pub async fn build_file_path(
    module: &str,
    original_filename: &str,
    relative_path: &Option<String>,
) -> Result<String, String> {
    let module_path = format!("./uploads/{}", module);
    
    // 确保模块目录存在
    tokio_fs::create_dir_all(&module_path).await
        .map_err(|e| format!("创建模块目录失败: {}", e))?;

    // 构建初始文件路径
    let initial_filepath = if let Some(rel_path) = relative_path {
        let full_path = Path::new(&module_path).join(rel_path).join(original_filename);
        if let Some(parent) = full_path.parent() {
            tokio_fs::create_dir_all(parent).await
                .map_err(|e| format!("创建子目录失败: {}", e))?;
        }
        full_path.to_string_lossy().to_string()
    } else {
        format!("{}/{}", module_path, original_filename)
    };

    // 生成唯一文件名
    let final_filepath = file_utils::generate_unique_filename(original_filename, &initial_filepath);
    
    Ok(final_filepath)
}

pub async fn delete_file(module: &str, filename: &str) -> Result<(), String> {
    let file_path = format!("./uploads/{}/{}", module, filename);
    
    tokio_fs::remove_file(&file_path).await
        .map_err(|e| format!("删除文件失败: {}", e))?;
        
    Ok(())
}

pub async fn delete_folder(module: &str, folder_path: &str) -> Result<(), String> {
    let full_path = format!("./uploads/{}/{}", module, folder_path);
    
    tokio_fs::remove_dir_all(&full_path).await
        .map_err(|e| format!("删除文件夹失败: {}", e))?;
        
    Ok(())
}

pub async fn delete_module(module: &str) -> Result<(), String> {
    let module_path = format!("./uploads/{}", module);
    let temp_dir = format!("./temp/{}", module);

    // 删除模块目录
    tokio_fs::remove_dir_all(&module_path).await
        .map_err(|e| format!("删除模块目录失败: {}", e))?;

    // 尝试删除临时目录（可选）
    let _ = tokio_fs::remove_dir_all(&temp_dir).await;
        
    Ok(())
}