# Agent 接入方案分析

本文档对比将语义搜索能力接入外部 agent 的三种方案，分析各自的优劣与适用场景。

---

## 方案一：Skill + CLI（无状态进程）

每次 agent 调用语义搜索时，通过 shell 执行 `semantic-search` binary，执行完毕进程退出。

### 冷启动开销

每次调用都要完整经历以下初始化链：

```
dlopen(onnxruntime.dylib)       ~100–500 ms
OnnxSession::new(model.onnx)    ~1–3 s     ← 主要瓶颈
Tokenizer::from_file(...)        ~100 ms
rusqlite::Connection::open(...)  ~10–50 ms
LanceDB::connect(...)            ~50–200 ms
────────────────────────────────────────────
总计冷启动                       ~1.5–4 s / 次
```

对于一个需要连续多次 search 的对话，每次提问都付出这笔固定成本。

### 核心缺陷

**无法获取索引状态**

索引进度和取消令牌存活在进程内存中，进程退出后完全消失。后台运行索引时，search 进程与 index 进程之间没有任何状态共享通道——无法得到细粒度进度，无法触发取消，唯一能做的是轮询 SQLite 的完成时间戳。

**内存随并发线性放大**

```
N 个 agent 并发：内存 = N × (模型 + Runtime + LanceDB 缓存) ≈ N × 300–800 MB
单个 Server：   内存 = 固定 300–800 MB，不随并发增长
```

**无法实现真正的非阻塞索引**

后台索引需要外部 PID 文件管理、进度只能磁盘轮询近似、取消只能发 SIGTERM 无法优雅等待当前 batch、多次触发需手动判重。

**并发访问风险**

index 进程持有 SQLite 写事务时，search 进程读取会遇到 `SQLITE_BUSY`；LanceDB 多进程并发打开存在元数据缓存不一致问题。

---

## 方案二：Skill + HTTP Server（有状态进程）

常驻 HTTP server，启动时完成一次性初始化，后续所有请求复用已加载的资源。Skill 告知 agent 通过 HTTP 接口调用能力。

### 热路径

```
agent 发起 search（Server 已启动）
  → HTTP POST /search
  → 嵌入 query（warm Session，~20 ms）
  → 向量检索（~50 ms）
  → 返回结果
总耗时 ~70–150 ms（节省 ~2–4 s 冷启动）
```

### 优势

| 能力 | CLI | HTTP Server |
|------|-----|-------------|
| 索引进度实时查询 | 不可（跨进程） | 可（内存直接访问） |
| 优雅取消索引 | 不可（只能 SIGTERM） | 可（CancelToken） |
| 索引中并发 search | 有风险（SQLite 锁） | 安全（单连接） |
| 多 agent 并发 | N 倍内存 | 固定内存 |
| 多项目支持 | 每次重建 | 惰性缓存 |

### 上下文影响

**HTTP Server 不会向 agent 上下文注入任何内容。** Agent 通过 skill 文件了解接口的存在和调用方式，工具描述写在 skill 的自然语言里，不占用结构化的工具 schema 上下文。

### 局限

- 需要常驻进程，比 CLI 部署略复杂
- 需要管理端口、生命周期（启动/停止）
- Skill 里的接口描述是自然语言，参数校验和错误处理需要 agent 自行处理，不如 MCP 的 schema 约束严格
- 仅支持本机或局域网调用（无内置认证）

---

## 方案三：MCP（有状态进程 + 协议标准化）

MCP（Model Context Protocol）是专为 agent 工具调用设计的标准协议。Agent 通过 stdio 与 MCP server 通信，server 常驻进程，具备与 HTTP Server 相同的有状态优势。

### 工作机制

```
agent 启动
  → 连接 MCP server（stdio）
  → 调用 tools/list，拉取所有工具 schema
  → 工具 schema 注入 agent 上下文（每轮对话）

agent 发起 search
  → MCP tool call: search(query, layer, limit, ...)
  → server 内执行（warm Session）
  → 返回结构化结果
```

### 优势

**协议标准化**：工具名、参数类型、错误码均有 schema 约束，agent 不需要自行解析自然语言描述的接口，调用更可靠。

**工具自动发现**：Agent 连接后自动获得所有可用工具，无需在 skill 里手动描述每个接口。

**原生兼容主流 agent**：OpenCode、Claude Code、Cursor、Windsurf 等均原生支持 MCP，无需额外适配。

**共享状态**：与 HTTP Server 相同——单进程、单连接、warm 模型、内存进度状态，不存在 CLI 的并发问题。

### 局限

**工具 schema 强制注入上下文**

这是 MCP 与 HTTP Server 最关键的差异。MCP 协议在 agent 连接时通过 `tools/list` 返回所有工具定义，agent 框架将其注入每轮对话的上下文。**无法只启动 server 而不暴露工具到上下文**——这是协议机制决定的，不是实现问题。

每个工具 schema 约占 100–300 token，5 个工具合计 ~500–1500 token，且每轮对话都消耗。

---

## 三方案对比

| 维度 | 方案一：CLI | 方案二：HTTP Server | 方案三：MCP |
|------|-----------|-------------------|------------|
| 首次 search 延迟 | ~2–4 s | ~100 ms | ~100 ms |
| 后续 search 延迟 | ~2–4 s | ~70 ms | ~70 ms |
| 索引状态查询 | 不支持 | 支持 | 支持 |
| 并发安全 | 需额外处理 | 天然安全 | 天然安全 |
| 内存（N 并发） | N × 模型大小 | 固定 | 固定 |
| 工具 schema 注入上下文 | 否 | 否 | **是（强制）** |
| 参数校验 | 无 | 无（自然语言描述） | 有（schema 约束） |
| agent 兼容性 | 任何能执行 shell | 任何能发 HTTP | 仅 MCP 兼容 agent |
| 部署复杂度 | 最低 | 中 | 中 |

