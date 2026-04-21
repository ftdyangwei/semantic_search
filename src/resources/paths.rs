use std::path::{Path, PathBuf};

use anyhow::anyhow;

use crate::embedding::utils::EmbeddingModelType;

/// Base directory containing embedded resources.
///
/// If `SEMANTIC_SEARCH_RESOURCES_DIR` is set, it wins. Otherwise we use
/// a best-effort search order:
/// - `<exe_dir>/resources/` (portable packaging)
/// - `<crate_root>/resources/` (dev builds; baked at compile time)
fn resources_base_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("SEMANTIC_SEARCH_RESOURCES_DIR") {
        PathBuf::from(dir)
    } else {
        // Portable layout: place `resources/` next to the executable.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                let candidate = exe_dir.join("resources");
                if candidate.is_dir() {
                    return candidate;
                }
            }
        }

        // Dev layout: this is an absolute path baked at compile time.
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources")
    }
}

fn legacy_assets_dir() -> PathBuf {
    // Keep backward-compatibility with the previous local-only directory layout:
    // `<repo_root>/../semantic_search_resources/`
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    repo_root
        .parent()
        .map(|p| p.join("semantic_search_resources"))
        .unwrap_or_else(|| repo_root.join("semantic_search_resources"))
}

fn candidate_file_exists(path: &Path) -> bool {
    path.is_file()
}

fn default_model_dir(model_type: &EmbeddingModelType) -> PathBuf {
    resources_base_dir()
        .join("embedding")
        .join(model_type.to_string().to_lowercase())
}

fn default_runtime_platform_dir() -> &'static str {
    // Keep keys in sync with `resources/README.md`.
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "darwin-aarch64"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        "darwin-x86_64"
    } else if cfg!(target_os = "windows") && cfg!(target_arch = "x86_64") {
        "windows-x86_64"
    } else {
        "unsupported"
    }
}

fn runtime_filename() -> &'static str {
    if cfg!(target_os = "macos") {
        "onnxruntime.dylib"
    } else if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "linux") {
        "onnxruntime.so"
    } else {
        "onnxruntime"
    }
}

fn find_existing_from_candidates(candidates: &[PathBuf], what: &str) -> anyhow::Result<PathBuf> {
    for c in candidates {
        if candidate_file_exists(c) {
            return Ok(c.clone());
        }
    }
    Err(anyhow!(
        "missing {}; tried: {}",
        what,
        candidates
            .iter()
            .map(|c| c.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Return the default ONNX Runtime dynamic library path for the current platform.
pub fn default_onnxruntime_path() -> anyhow::Result<PathBuf> {
    let platform = default_runtime_platform_dir();
    if platform == "unsupported" {
        return Err(anyhow!(
            "unsupported platform for built-in onnxruntime resources"
        ));
    }

    let candidates = vec![
        resources_base_dir()
            .join("onnxruntime")
            .join(platform)
            .join(runtime_filename()),
        // Legacy layout (used by tests & local dev):
        legacy_assets_dir().join("models").join(runtime_filename()),
    ];

    find_existing_from_candidates(&candidates, "onnxruntime runtime library")
}

fn default_model_candidates(model_type: &EmbeddingModelType) -> Vec<PathBuf> {
    vec![
        default_model_dir(model_type).join("model.onnx"),
        default_model_dir(model_type).join("model_fp16.onnx"),
        // Legacy layout:
        legacy_assets_dir().join("models").join("model_fp16.onnx"),
        legacy_assets_dir().join("models").join("model.onnx"),
    ]
}

fn default_tokenizer_candidates(model_type: &EmbeddingModelType) -> Vec<PathBuf> {
    let _ = model_type;
    vec![
        default_model_dir(model_type).join("tokenizer.json"),
        // Legacy layout:
        legacy_assets_dir().join("models").join("tokenizer.json"),
    ]
}

/// Return the default embedding model path for the given model type.
pub fn default_embedding_model_path(model_type: EmbeddingModelType) -> anyhow::Result<PathBuf> {
    let candidates = default_model_candidates(&model_type);
    find_existing_from_candidates(&candidates, "embedding model (model.onnx)")
}

/// Return the default tokenizer path for the given model type.
pub fn default_tokenizer_path(model_type: EmbeddingModelType) -> anyhow::Result<PathBuf> {
    let candidates = default_tokenizer_candidates(&model_type);
    find_existing_from_candidates(&candidates, "tokenizer.json")
}
