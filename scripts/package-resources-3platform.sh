#!/usr/bin/env bash
set -euo pipefail

# 只打包 `resources/`（按平台拆分 onnxruntime），生成可上传到 GitHub Release 的归档文件。
#
# 产物（默认输出到 dist/）：
# - dist/semantic-search-resources-darwin-aarch64.tar.gz
# - dist/semantic-search-resources-darwin-x86_64.tar.gz
# - dist/semantic-search-resources-windows-x86_64.zip
#
# 说明：
# - 该脚本不会编译任何二进制，只处理 `resources/`。
# - 会把 `resources/onnxruntime/` 里“非目标平台”的目录移除，仅保留目标平台那一份，减小体积。
#
# 依赖：
# - tar / zip

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing command: $1"
    exit 2
  }
}

need_cmd tar
need_cmd zip

if [[ ! -d "resources" ]]; then
  echo "missing resources/ directory at repo root"
  exit 2
fi

OUT_DIR="${ROOT_DIR}/dist"
mkdir -p "${OUT_DIR}"

# 打包策略：
# - macOS 自带的 bsdtar 不支持 GNU tar 的 `--transform`，因此这里统一采用轻量 staging 方式。
# - staging 时不会拷贝 `resources/onnxruntime/` 的其它平台目录，只保留目标平台那一份。

stage_and_archive() {
  local platform="$1"   # darwin|windows
  local arch="$2"       # aarch64|x86_64
  local ext="$3"        # tar.gz|zip
  local ort_dir="${platform}-${arch}"

  if [[ ! -d "resources/onnxruntime/${ort_dir}" ]]; then
    echo "missing onnxruntime dir: resources/onnxruntime/${ort_dir}"
    exit 2
  fi

  local stage_dir
  stage_dir="$(mktemp -d)"
  trap 'rm -rf "${stage_dir}"' RETURN

  local pkg_name="semantic-search-resources-${platform}-${arch}"
  local pkg_root="${stage_dir}/${pkg_name}"

  mkdir -p "${pkg_root}"

  # 只拷贝 resources/ 下除 onnxruntime 外的内容，再补上目标平台的 onnxruntime/<platform-arch>
  mkdir -p "${pkg_root}/resources"
  for p in resources/*; do
    if [[ "$(basename "$p")" == "onnxruntime" ]]; then
      continue
    fi
    cp -R "$p" "${pkg_root}/resources/"
  done
  mkdir -p "${pkg_root}/resources/onnxruntime"
  cp -R "resources/onnxruntime/${ort_dir}" "${pkg_root}/resources/onnxruntime/${ort_dir}"

  local out_path="${OUT_DIR}/${pkg_name}.${ext}"
  rm -f "${out_path}"

  echo "[resources] creating ${out_path}"
  if [[ "${ext}" == "zip" ]]; then
    (cd "${stage_dir}" && zip -qr "${out_path}" "${pkg_name}")
  else
    (cd "${stage_dir}" && tar --exclude='*.DS_Store' -czf "${out_path}" "${pkg_name}")
  fi
  echo "[resources] wrote ${out_path}"
}

stage_and_archive "darwin" "aarch64" "tar.gz"
stage_and_archive "darwin" "x86_64" "tar.gz"
stage_and_archive "windows" "x86_64" "zip"

echo "[resources] done:"
ls -lh dist/semantic-search-resources-* || true

