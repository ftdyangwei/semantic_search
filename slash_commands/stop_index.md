---
description: 停止/取消当前语义索引任务
---

调用 MCP server `semantic-search` 的 `stop_index` tool。

参数：
- `project`: 当前工作区仓库根的绝对路径

将 tool 返回的状态汇报给用户（通常为 cancelled 或 not_running），并提示可通过 /index 重新触发索引。
