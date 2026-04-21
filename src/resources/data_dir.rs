use std::path::{Path, PathBuf};

use anyhow::anyhow;

#[derive(Debug, Clone)]
pub struct DefaultDataPaths {
    pub root_dir: PathBuf,
    pub index_db_path: PathBuf,
    pub vector_db_path: PathBuf,
}

pub fn default_paths_under(data_dir: &Path) -> DefaultDataPaths {
    let root_dir = data_dir.join("semantic_search");
    DefaultDataPaths {
        index_db_path: root_dir.join("index.db"),
        vector_db_path: root_dir.join("vectordb"),
        root_dir,
    }
}

/// Platform default `${DATA_DIR}`:
/// - macOS: `~/Library/Application Support`
/// - Windows: `%APPDATA%`
///
/// Linux is intentionally not considered for now.
pub fn platform_data_dir() -> anyhow::Result<PathBuf> {
    if cfg!(target_os = "linux") {
        return Err(anyhow!("linux data dir is not supported yet"));
    }

    dirs::data_dir().ok_or_else(|| anyhow!("failed to resolve platform data_dir"))
}

pub fn platform_default_data_paths() -> anyhow::Result<DefaultDataPaths> {
    let data_dir = platform_data_dir()?;
    Ok(default_paths_under(&data_dir))
}

/// 平台级日志路径：`${DATA_DIR}/semantic_search/running.log`。
pub fn platform_log_path() -> anyhow::Result<PathBuf> {
    let data_dir = platform_data_dir()?;
    Ok(data_dir.join("semantic_search").join("running.log"))
}
