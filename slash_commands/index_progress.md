---
description: 查询当前语义索引的进度与状态
---

调用 MCP server `semantic-search` 的 `index_progress` tool。

参数：
- `project`: 当前工作区仓库根的绝对路径

将结果以人类可读方式汇报给用户：
- status（running / done / cancelled / error / idle）
- 进度：已处理 / 总计（文件数、符号数）
- last_error（若有错误，建议重新执行 /index）

根据状态给出建议：
- running → 继续轮询，或等待完成后再 search
- done → 索引已完成，可直接进行语义搜索
- error → 建议重新执行 /index
- cancelled → 索引已停止，可重新执行 /index
