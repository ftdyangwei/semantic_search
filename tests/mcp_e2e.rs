use rmcp::{
    model::{CallToolRequestParam, JsonObject},
    serve_client,
    service::QuitReason,
    transport::TokioChildProcess,
};
use std::{fs, path::PathBuf};
use vnext_semantic_search::{
    embedding::utils::EmbeddingModelType, resources::paths as resource_paths,
};

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "{}-{}-{}",
        prefix,
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    dir
}

fn json_object(pairs: &[(&str, serde_json::Value)]) -> JsonObject {
    let mut obj = JsonObject::new();
    for (k, v) in pairs {
        obj.insert((*k).to_string(), v.clone());
    }
    obj
}

fn extract_first_text_json(result: &rmcp::model::CallToolResult) -> Option<&str> {
    result.content.first().and_then(|c| match &c.raw {
        rmcp::model::RawContent::Text(t) => Some(t.text.as_str()),
        _ => None,
    })
}

#[tokio::test]
async fn mcp_stdio_e2e_index_and_search() -> anyhow::Result<()> {
    // If model/runtime assets are not present, skip this E2E test gracefully.
    if resource_paths::default_onnxruntime_path().is_err()
        || resource_paths::default_embedding_model_path(EmbeddingModelType::Veso).is_err()
        || resource_paths::default_tokenizer_path(EmbeddingModelType::Veso).is_err()
    {
        eprintln!("skipping mcp e2e: missing embedded/legacy onnxruntime/model/tokenizer assets");
        return Ok(());
    }

    let project_dir = unique_temp_dir("semantic-search-mcp-project");
    fs::create_dir_all(&project_dir)?;
    fs::write(
        project_dir.join("main.rs"),
        "fn main() { println!(\"hello\"); }\n",
    )?;

    let bin = env!("CARGO_BIN_EXE_semantic-search-mcp");

    let cmd = tokio::process::Command::new(bin);

    let transport = TokioChildProcess::new(cmd)?;

    // `()` is a no-op client handler, sufficient for tool calls.
    let mut client = serve_client((), transport).await?;

    let tools = client.list_all_tools().await?;
    let tool_names = tools.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>();
    // MCP 侧已用 `start_index` 替代阻塞式 `index` 工具名。
    assert!(
        tool_names.contains(&"start_index"),
        "missing tool: start_index"
    );
    assert!(tool_names.contains(&"search"), "missing tool: search");

    let start_index_result = client
        .call_tool(
            CallToolRequestParam {
                name: "start_index".into(),
                arguments: Some(json_object(&[
                    ("layer", serde_json::json!("all")),
                    (
                        "project",
                        serde_json::json!(project_dir.to_string_lossy().to_string()),
                    ),
                ])),
            },
        )
        .await?;

    let start_index_json = extract_first_text_json(&start_index_result)
        .ok_or_else(|| anyhow::anyhow!("start_index result missing text content"))?;
    let start_index_value: serde_json::Value = serde_json::from_str(start_index_json)?;
    assert!(
        start_index_value.get("layers").is_some(),
        "start_index json: {start_index_json}"
    );

    let search_result = client
        .call_tool(
            CallToolRequestParam {
                name: "search".into(),
                arguments: Some(json_object(&[
                    ("query", serde_json::json!("hello")),
                    ("layer", serde_json::json!("file")),
                    ("limit", serde_json::json!(3)),
                    ("threshold", serde_json::json!(0.0)),
                    (
                        "project",
                        serde_json::json!(project_dir.to_string_lossy().to_string()),
                    ),
                ])),
            },
        )
        .await?;

    let search_json = extract_first_text_json(&search_result)
        .ok_or_else(|| anyhow::anyhow!("search result missing text content"))?;
    let search_value: serde_json::Value = serde_json::from_str(search_json)?;
    assert!(
        search_value.get("query").is_some(),
        "search json: {search_json}"
    );

    // 第二个工程目录：通过 tool 参数 `project` 路由到独立 registry 项（数据目录按工程隔离）
    let project_b = unique_temp_dir("semantic-search-mcp-project-b");
    fs::create_dir_all(&project_b)?;
    fs::write(project_b.join("lib.rs"), "fn other_crate_fn() {}\n")?;

    let search_b = client
        .call_tool(
            CallToolRequestParam {
                name: "search".into(),
                arguments: Some(json_object(&[
                    ("query", serde_json::json!("other_crate_fn")),
                    (
                        "project",
                        serde_json::json!(project_b.to_string_lossy().to_string()),
                    ),
                    ("layer", serde_json::json!("file")),
                    ("limit", serde_json::json!(3)),
                    ("threshold", serde_json::json!(0.0)),
                ])),
            },
        )
        .await?;
    let search_b_json = extract_first_text_json(&search_b)
        .ok_or_else(|| anyhow::anyhow!("search(project_b) missing text"))?;
    let search_b_value: serde_json::Value = serde_json::from_str(search_b_json)?;
    assert!(
        search_b_value.get("project").is_some(),
        "search with project: {search_b_json}"
    );

    // Graceful shutdown.
    // rmcp 0.12.x 的 `RunningService` 没有 `close()`；这里使用 cancel() 停止后台任务并回收子进程。
    let _reason: QuitReason = client.cancel().await?;

    Ok(())
}
