#!/usr/bin/env bash
set -euo pipefail

# 用本地打包好的 “resources 三平台归档” 创建/更新 GitHub Release 并上传 assets。
#
# 前置：
# - 已安装并登录 GitHub CLI：`gh auth login`（或设置 GH_TOKEN）
# - 已生成 dist/semantic-search-resources-*.tar.gz|*.zip
#
# 用法：
#   bash scripts/upload-resources-local.sh mcp-v0.1.0
#
# 它会：
# - 创建 release（如果不存在）
# - 上传 dist/semantic-search-resources-* 的所有文件（覆盖同名 asset）

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

TAG="${1:-}"
if [[ -z "${TAG}" ]]; then
  echo "missing tag, e.g. mcp-v0.1.0"
  exit 2
fi

shopt -s nullglob
ASSETS=(dist/semantic-search-resources-*.tar.gz dist/semantic-search-resources-*.zip)
if [[ ${#ASSETS[@]} -eq 0 ]]; then
  echo "no assets found under dist/ (semantic-search-resources-*.tar.gz|*.zip)"
  exit 2
fi

if gh release view "${TAG}" >/dev/null 2>&1; then
  echo "[release] ${TAG} exists"
else
  echo "[release] creating ${TAG}"
  gh release create "${TAG}" --title "${TAG}" --notes "Local resources artifacts."
fi

echo "[release] uploading assets:"
printf ' - %s\n' "${ASSETS[@]}"

gh release upload "${TAG}" "${ASSETS[@]}" --clobber

echo "[release] done"

