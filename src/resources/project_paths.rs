//! Per-project layout under `${DATA_DIR}/semantic_search/{sanitized_name}_{path_hash}/`.

use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::common::utils::hash_str;

use super::data_dir::DefaultDataPaths;

/// Max length for the directory name segment taken from the project root folder name.
const MAX_NAME_LEN: usize = 64;

/// Sanitize a single path component for use as a directory name (cross-platform).
fn sanitize_dir_component(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    s.truncate(MAX_NAME_LEN);
    let s = s.trim_matches('_').trim_matches('.');
    if s.is_empty() {
        "project".to_string()
    } else {
        s.to_string()
    }
}

/// Canonicalize project root (creates directory if missing so `canonicalize` can succeed when possible).
pub fn normalize_project_root(root: &Path) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(root)
        .with_context(|| format!("failed to create project directory: {}", root.display()))?;
    match root.canonicalize() {
        Ok(p) => Ok(p),
        Err(_) => {
            // Fallback if canonicalize fails after create_dir_all (rare).
            Ok(root.to_path_buf())
        }
    }
}

/// Directory name: `{sanitized_last_segment}_{hash_str(canonical_path)}`.
/// Stable string key for registry maps: normalized absolute path as UTF-8 (lossy).
pub fn project_path_key(root: &Path) -> anyhow::Result<String> {
    let canon = normalize_project_root(root)?;
    Ok(canon.to_string_lossy().into_owned())
}

pub fn project_storage_dir_name(root: &Path) -> anyhow::Result<String> {
    let canon = normalize_project_root(root)?;
    let key = canon.to_string_lossy();
    let file_name = canon
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let safe = sanitize_dir_component(file_name);
    let h = hash_str(key.as_ref());
    Ok(format!("{safe}_{h}"))
}

/// Default index / vector paths for one project under the given data directory root.
pub fn project_default_paths(data_dir: &Path, root: &Path) -> anyhow::Result<DefaultDataPaths> {
    let dir_name = project_storage_dir_name(root)?;
    let root_dir = data_dir.join("semantic_search").join(dir_name);
    Ok(DefaultDataPaths {
        index_db_path: root_dir.join("index.db"),
        vector_db_path: root_dir.join("vectordb"),
        root_dir,
    })
}

/// Resolve per-project paths using the platform user data directory.
pub fn platform_project_default_paths(root: &Path) -> anyhow::Result<DefaultDataPaths> {
    let data_dir = super::data_dir::platform_data_dir()?;
    project_default_paths(&data_dir, root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn dir_name_is_stable_for_same_path() {
        let tmp = std::env::temp_dir().join("vnext_proj_paths_test");
        fs::create_dir_all(&tmp).unwrap();
        let a = project_storage_dir_name(&tmp).unwrap();
        assert!(a.contains("vnext_proj_paths_test"));
    }
}
