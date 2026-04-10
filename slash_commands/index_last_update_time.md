---
description: 查看上次语义索引完成时间，判断索引是否需要刷新
---

调用 MCP server `semantic-search` 的 `index_last_update_time` tool。

参数：
- `project`: 当前工作区仓库根的绝对路径

输出要求：
- 若 `index_finished_time` 为 null：提示"当前工程尚未完成过索引"，建议执行 /index
- 若不为 null，展示：
  - 原始 epoch 时间戳（秒）
  - 人类可读时间（本地时间或 UTC）
  - 距今多久
  - 是否已过期（直接使用 MCP 返回的 `stale` 字段）；若已过期（`stale: true`），建议执行 /index 刷新索引
