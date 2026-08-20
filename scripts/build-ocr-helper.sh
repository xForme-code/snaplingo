#!/usr/bin/env bash
# 编译 macOS OCR sidecar。架构/命名/最低系统版本的讲究见 lib-sidecar.sh。
set -euo pipefail

cd "$(dirname "$0")/.."
source scripts/lib-sidecar.sh

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "[ocr-helper] 非 macOS，跳过（Windows 用系统 OCR API，Linux 用 ONNX 模型）"
  exit 0
fi

# Vision 框架 macOS 10.15 就有，这里对齐 App 声明的最低版本 11.0。
OUT="$(build_swift_sidecar snaplingo-ocr helpers/macos-ocr.swift 11.0)"

echo "[ocr-helper] 已生成 $OUT ($(du -h "$OUT" | cut -f1), $(lipo -archs "$OUT"))"
