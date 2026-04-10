---
description: 语义搜索 skill —— 指导 agent 何时调用语义索引/搜索、如何映射 slash command 到 MCP tool
---

# Semantic Search Skill

你可以访问 MCP server `semantic-search`，它提供对当前代码仓库的语义索引与检索能力。

---

## 何时调用 `search`

当用户的问题满足以下任一条件时，**优先调用 `search`**，而非直接读取文件或全仓扫描：

- **定位类**："X 在哪里实现/定义？""某接口/函数/struct/类在哪个文件里？"
- **关系类**："谁调用了 X？""X 会触发哪些逻辑？""这段逻辑和哪些模块相关？"
- **意图类（不知道精确关键词）**："权限校验在哪里做的？""错误处理逻辑怎么运作？"
- **跨文件/跨模块**：需要从多个位置快速收敛候选范围

**不适合用语义搜索：**

- 查找字符串字面量的出现位置 → 优先用 ripgrep / grep
- 纯概念解释、不依赖代码细节的问题

---

## `search` 参数指南

默认参数（无特殊场景不需要调整）：

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `layer` | `symbol` | 定位定义/调用关系首选；问文档/注释用 `content`；需要广泛候选集用 `all` |
| `limit` | `10` | 精准定位时可降至 5；结果较少时可提高到 20 |
| `threshold` | `0.5` | 不建议低于 0.4（噪音多）或高于 0.8（漏结果） |
| `paths` | 不传 | 用户明确限定目录/模块时再加 |
| `project` | 不传 | 由环境变量兜底；多仓库场景传入当前仓库根的绝对路径 |

**`layer` 快速选择：**

| 场景 | `layer` |
|------|---------|
| 定位函数/类/接口/符号定义 | `symbol` |
| 找哪个文件最相关 | `file` |
| 读文档/注释/README/协议文本 | `content` |
| 需要综合候选集 | `all` |

---

## Slash Command → MCP Tool 映射

当用户输入以下命令时，调用对应的 MCP tool：

| 用户命令 | 调用 MCP tool | 关键参数 |
|----------|--------------|---------|
| `/index` | `start_index` | `layer=all` |
| `/index_progress` | `index_progress` | — |
| `/stop_index` | `stop_index` | — |
| `/index_last_update_time` | `index_last_update_time` | — |

所有 tool 均支持可选参数 `project`（仓库根的绝对路径）。未传时由环境变量 `SEMANTIC_SEARCH_PROJECT` 兜底。

---

## 索引与搜索协同策略

**日常使用（索引已存在）：**
- 直接调用 `search`，无需先等待 `/index` 完成。
- `search` 内部会在上次索引完成超过 10 分钟时自动触发后台刷新（非阻塞），并继续返回当前索引的搜索结果。

**首次使用或大规模变更后：**
1. 调用 `/index`（触发 `start_index`），立即返回 running 状态
2. 使用 `/index_progress` 轮询，直到 `status` 变为 `done`
3. 调用 `search` 获取完整结果

**索引进行中执行 `search`：**
- 允许，但结果只包含已写入的部分数据。
- 必须在回答中告知用户：**"当前索引尚未完成，结果可能不完整。"**

**`/index_last_update_time` 的使用场景：**
- 判断索引是否过期（`stale: true` 表示已超过 10 分钟）
- `index_finished_time` 为 null 时表示当前工程尚未完成过索引，建议先执行 `/index`

---

## 行为准则（最小规则集）

```
当用户在问"代码在哪里/怎么实现/谁调用谁/相关模块有哪些"，优先调用 search。
search 在索引超过 10 分钟时会自动后台刷新；若索引正在进行中，结果可能不完整，需在回答中说明。
当用户显式输入 /index、/index_progress、/stop_index、/index_last_update_time 时，分别调用对应 MCP tool。
```
