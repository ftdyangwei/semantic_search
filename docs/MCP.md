# Semantic Search MCP Server

`semantic-search-mcp` 是一个原生 Rust 实现的 [MCP](https://modelcontextprotocol.io/) server，将代码语义索引与检索能力以标准化工具接口暴露给外部 agent。

---

## 概述

### 能力

- **文件级索引**（`file`）：对仓库内每个代码文件整体进行向量嵌入，适合"某功能在哪个文件里"的定位查询。
- **符号级索引**（`symbol`）：借助 tree-sitter 对 TypeScript / JavaScript / ArkTS 等语言提取函数、类、变量等符号，适合"某符号如何实现/被谁调用"的精准查询。
- **内容级索引**（`content`）：对文件内容进行细粒度分块嵌入（实验性）。
- **增量索引**：基于文件哈希与 mtime，只对变更文件重新嵌入，避免全量重建。
- **多项目隔离**：一个 MCP server 实例可同时服务多个仓库，各自维护独立的存储与索引状态。

### 内置资源

server 二进制内嵌了以下资源，无需用户额外配置即可开箱使用：

| 资源 | 说明 |
|------|------|
| ONNX Runtime | 各平台动态库（`darwin-aarch64` / `darwin-x86_64` / `windows-x86_64`） |
| Veso 嵌入模型 | `model.onnx` + `tokenizer.json`，维度 768 |



## 工具参考

### `index`

**描述**：在后台启动语义索引任务（非阻塞）。在对新仓库执行 `search` 前，必须先调用此工具完成索引。

**参数：**

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `layer` | `string` | 否 | `"all"` | 索引层级：`file` \| `symbol` \| `content` \| `all` |
| `project` | `string` | 是 |  | 仓库根目录的绝对路径 |

**`layer` 说明：**

| 值 | 含义 |
|----|------|
| `file` | 仅建立文件级索引 |
| `symbol` | 仅建立符号级索引 |
| `content` | 仅建立内容级索引（实验性） |
| `all` | 依次建立 `file` + `symbol` 索引 |

**请求示例：**

```json
{
  "layer": "all",
  "project": "/workspace/my-project"
}
```

**响应示例：**

```json
{
  "project": "/workspace/my-project",
  "layers": ["file", "symbol"],
  "status": "running",
  "usage_hint": "Use index_progress to check status"
}
```

---

### `index_progress`

**描述**：查询当前索引任务的进度与状态。

**参数：**

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `project` | `string` | 是 |  | 仓库根目录的绝对路径 |

**请求示例：**

```json
{
  "project": "/workspace/my-project"
}
```

**响应示例：**

```json
{
  "project": "/workspace/my-project",
  "status": "running",
  "progress": {
    "total_file_count": 320,
    "handled_file_count": 128,
    "total_symbol_count": 4800,
    "handled_symbol_count": 1920
  },
  "last_error": null,
  "usage_hint": "Indexing in progress, search may return partial results"
}
```

**`status` 取值：**

| 值 | 含义 |
|----|------|
| `running` | 索引正在进行 |
| `done` | 索引已完成，可执行 `search` |
| `cancelled` | 索引被手动取消 |
| `error` | 索引过程出现错误，见 `last_error` |
| `idle` | 当前没有索引任务 |

---

### `stop_index`

**描述**：取消/停止当前正在运行的索引任务。

**参数：**

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `project` | `string` | 是 |  | 仓库根目录的绝对路径 |

**请求示例：**

```json
{
  "project": "/workspace/my-project"
}
```

**响应示例：**

```json
{
  "project": "/workspace/my-project",
  "status": "cancelled",
  "usage_hint": "Indexing has been stopped"
}
```

---

### `index_last_update_time`

**描述**：查询指定项目最近一次索引完成的时间，以及当前是否已超过过期阈值（10 分钟）。可用于决策是否需要重新触发索引。

**参数：**

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `project` | `string` | 是 |  | 仓库根目录的绝对路径 |

**请求示例：**

```json
{
  "project": "/workspace/my-project"
}
```

**响应示例：**

```json
{
  "project": "/workspace/my-project",
  "index_finished_time": 1712640000,
  "now": 1712640300,
  "stale_threshold_seconds": 600,
  "stale": false,
  "usage_hint": "Index is fresh, search is ready"
}
```

**字段说明：**

| 字段 | 类型 | 说明 |
|------|------|------|
| `index_finished_time` | `integer` \| `null` | 最近一次索引完成的 Unix 时间戳（秒）；从未完成过索引时为 `null` |
| `now` | `integer` | 当前 Unix 时间戳（秒） |
| `stale_threshold_seconds` | `integer` | 过期阈值，固定为 `600`（10 分钟） |
| `stale` | `boolean` | `true` 表示索引已过期，建议调用 `start_index` 重建；`false` 表示索引仍在有效期内 |
| `usage_hint` | `string` | 操作建议 |

---

### `search`

**描述**：对已索引的仓库执行语义搜索。如果距离上次索引完成超过 10 分钟，会自动触发后台重新索引（非阻塞）。

**参数：**

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `query` | `string` | **是** |  | 语义查询字符串 |
| `project` | `string` | 是 |  | 仓库根目录的绝对路径 |
| `layer` | `string` | 否 | `"symbol"` | 搜索层级：`file` \| `symbol` \| `content` \| `all` |
| `limit` | `integer` | 否 | `10` | 最大返回条数 |
| `threshold` | `float` | 否 | `0.5` | 最低相似度阈值（0.0 ~ 1.0） |
| `paths` | `string[]` | 否 | — | 按相对路径过滤，仅在这些文件中搜索 |

**请求示例：**

```json
{
  "query": "audio player state management",
  "project": "/workspace/my-project",
  "layer": "symbol",
  "limit": 10,
  "threshold": 0.5,
  "paths": ["src/player", "src/audio"]
}
```

**响应示例：**

```json
{
  "project": "/workspace/my-project",
  "query": "audio player state management",
  "layer": "symbol",
  "limit": 10,
  "threshold": 0.5,
  "results": [
    {
      "score": 0.87,
      "chunk_info": {
        "file_path": "src/player/PlayerController.ts",
        "language": "typescript",
        "layer": "symbol",
        "range": {
          "start": { "line": 42, "character": 0 },
          "end": { "line": 89, "character": 1 }
        }
      }
    }
  ],
  "usage_hint": ""
}
```

---

## 环境变量

### 模型与运行时（可选，覆盖内置资源）

| 变量 | 说明 |
|------|------|
| `SEMANTIC_SEARCH_MODEL` | 嵌入模型 `.onnx` 文件路径 |
| `SEMANTIC_SEARCH_TOKENIZER` | Tokenizer JSON 文件路径 |
| `SEMANTIC_SEARCH_MODEL_TYPE` | 模型类型 |

### 性能调优（可选）

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `SEMANTIC_SEARCH_BATCH_SIZE` | `32` | 推理批大小 |
| `SEMANTIC_SEARCH_THREADS` | `1` | 嵌入线程数 |
| `SEMANTIC_SEARCH_INTRA_THREADS` | `1` | ONNX 算子内线程数 |

### 日志与存储（可选）

| 变量 | 说明 |
|------|------|
| `SEMANTIC_SEARCH_LOG_PATH` | 日志文件路径（覆盖默认） |
| `SEMANTIC_SEARCH_LOG_LEVEL` | 日志级别：`trace` \| `debug` \| `info`（默认）\| `warn` \| `error` |
| `SEMANTIC_SEARCH_DATA_DIR` | 数据存储路径 |

---

## 数据存储

### 存储布局

默认情况下，数据按仓库路径隔离存储，无需手动配置：

```
${DATA_DIR}/semantic_search/
├── {project_name}_{path_hash_16hex}/
│   ├── index.db          # SQLite 元数据（文件状态、增量信息）
│   └── vectordb/         # LanceDB 向量数据
└── running.log           # 运行日志（所有项目共用）
```

**`DATA_DIR` 平台路径：**

| 平台 | 路径 | 示例 |
|------|------|------|
| macOS | `$HOME/Library/Application Support` | `/Users/alice/Library/Application Support` |
| Windows | `%APPDATA%` | `C:\Users\Alice\AppData\Roaming` |

### 多项目支持

一个 MCP server 实例通过 `ProjectRegistry` 管理多个仓库的生命周期。每个仓库的 `SemanticSearchManager`（包含嵌入器、索引管理器、存储层）在首次访问时按需创建，彼此完全隔离。

在 tool 请求中显式传入 `project` 字段即可切换目标仓库，无需重启 server。

---

## 接入 MCP 客户端

以 Claude Code 为例，在 MCP 配置中添加：

```json
{
  "mcpServers": {
    "semantic-search": {
      "command": "semantic-search-mcp",
    }
  }
}
```

---

## 给 Agent 的使用建议

### 何时调用 `start_index`

- 首次进入仓库，索引不存在时
- 仓库代码发生大规模变更后
- `search` 返回的结果明显滞后时

### 何时调用 `search`

- 用户询问"某功能在哪里实现""某变量在哪里定义""某逻辑如何运作"
- 需要定位调用关系、文件归属、符号位置时
- 优先在 `search` 命中结果的基础上再精读文件，避免全仓盲扫

### Agent 推荐提示词

```text
如果用户在问代码位置、实现方式、调用关系、相关文件，优先调用 search 工具。
如果当前仓库尚未索引，先调用 start_index，再轮询 index_progress 等待完成，然后调用 search。
如果索引正在进行中，仍可调用 search，但需告知用户结果可能不完整。
```

