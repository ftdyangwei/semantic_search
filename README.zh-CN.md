# vnext_semantic_search

[English](README.md) | **简体中文**

面向代码与文本的 **Rust 语义检索库**：对文档 **分块**，用 **ONNX Runtime** 做向量 **嵌入**，**SQLite** 存项目与文件级索引元数据，**LanceDB** 存向量并支持近邻检索。支持按 **索引层**（如文件 / 符号 / 内容）组织数据，基于哈希与修改时间的 **增量索引**，以及异步索引流水线。

## 功能概览

- **编排**：`SemanticSearchManager` 统一组装存储、嵌入器与 `IndexManager`。
- **存储**：SQLite 管理项目与 `index_status`；LanceDB 存储 chunk 与向量，支持余弦距离与路径过滤。
- **嵌入**：`OnnxEmbedder` 批量推理，可配置图优化级别（对部分 FP16 模型可降级以避免图优化错误）。
- **分块**：通过 `ChunkerRegistry` 注册分块器（如整文件、tree-sitter TypeScript 符号级）。
- **指标**：索引阶段的耗时与资源画像（`metrics` 模块）。
- **CLI / MCP 集成**：支持通过 `semantic-search index` / `semantic-search search` 以及 `semantic-search-mcp` 暴露给 Cursor、OpenCode、ClaudeCode、OpenClaw 等产品，并映射为 `/index`、`/search` 或 MCP tools。

## 环境要求

- 已安装 **Rust** 工具链（本项目 `edition = "2024"`，请使用较新的 stable/nightly）。
- 运行时需要能加载 **ONNX Runtime** 动态库（路径与 `ort` 配置一致，详见嵌入相关代码与测试）。

## 构建与测试

```bash
cargo build --release
cargo test
```

## 打包与发布（仅 `resources/`，三平台）

如果你当前只需要发布运行时资源（模型/词表/onnxruntime 动态库等），可以把仓库根目录的 `resources/` 按平台拆分后上传到 GitHub Release。

本项目运行时会优先从“可执行文件同级的 `resources/`”加载资源；你也可以通过环境变量 **`SEMANTIC_SEARCH_RESOURCES_DIR`** 指定资源目录（优先级最高）。

### 本地打包 `resources/`（三平台）

```bash
bash scripts/package-resources-3platform.sh
ls -lh dist/semantic-search-resources-*
```

产物：
- `dist/semantic-search-resources-darwin-aarch64.tar.gz`
- `dist/semantic-search-resources-darwin-x86_64.tar.gz`
- `dist/semantic-search-resources-windows-x86_64.zip`

### 上传到 GitHub Release

前置：已安装并登录 GitHub CLI（`gh auth login`，或设置 `GH_TOKEN`）。

```bash
bash scripts/upload-resources-local.sh mcp-v0.1.0
```

代码格式化（不依赖 [cargo-make](https://github.com/sagiegurari/cargo-make)）：

```bash
cargo fmt --all
```

可选：安装 `cargo install cargo-make` 后使用 `cargo make fmt-fix`、`cargo make clippy`（见仓库根目录 `Makefile.toml`）。

## 目录结构

| 路径 | 说明 |
|------|------|
| `src/manager.rs` | 对外入口 `SemanticSearchManager` |
| `src/storage/` | `StorageManager`，SQLite `rdb/`，LanceDB `vector_db/` |
| `src/embedding/` | ONNX 嵌入与配置 |
| `src/index/` | 索引管理、Worker、文件检查 |
| `src/document_chunker/` | 分块器与 TS 符号解析 |
| `src/metrics/` | 指标数据、性能分析、计时 |
| `docs/DESIGN.md` | **架构与设计说明（中文）** |

## 设计文档

详细架构、数据流与模块职责见 **[`docs/DESIGN.md`](docs/DESIGN.md)**。

## 产品集成

如果你希望在 Cursor、OpenCode、ClaudeCode、OpenClaw 等产品中直接使用：

- `/index` 进行索引
- `/search <query>` 进行语义搜索
- 在回答代码问题时优先调用 `search`

请参考 **[`docs/INTEGRATIONS.md`](docs/INTEGRATIONS.md)**，其中包含：

- slash command 到 CLI 的映射规范
- MCP server 的接入方式
- 环境变量配置方式
- 各产品的命令模板示例
- “先 index 后 search”与“边 index 边 search”的使用约定

## 许可证

对外发布时请在仓库根目录添加 `LICENSE` 并在此处更新说明。
