---
description: 触发语义索引更新（后台，非阻塞）
---

调用 MCP server `semantic-search` 的 `start_index` tool。

参数：
- `layer`: "all"（同时建立文件级与符号级索引）
- `project`: 当前工作区仓库根的绝对路径

将 tool 返回的 JSON 以人类可读方式汇报给用户：
- 索引状态（running / already_running / error）
- 本次索引的 layers
- 下一步提示：建议用 /index_progress 轮询进度
