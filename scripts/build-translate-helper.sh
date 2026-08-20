#!/usr/bin/env bash
# 编译 macOS 翻译 sidecar。架构/命名/最低系统版本的讲究见 lib-sidecar.sh。
set -euo pipefail

cd "$(dirname "$0")/.."
source scripts/lib-sidecar.sh

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "[translate-helper] 非 macOS，跳过（其它平台走 OPUS-MT 或云端引擎）"
  exit 0
fi

# Translation 框架要 macOS 15+ 的 SDK。低版本上编不出真实现，但**不能跳过**——
# tauri.conf.json 的 externalBin 无条件要求这个文件存在，不生成的话构建直接失败，
# 别人在 macOS 14 上 clone 仓库就编不了。所以低版本编一个如实回报「不可用」的替身。
MAJOR="$(sw_vers -productVersion | cut -d. -f1)"
if [[ "$MAJOR" -lt 15 ]]; then
  echo "[translate-helper] 当前 macOS $MAJOR 低于 15，编译占位组件（运行时回报不可用）"
  OUT="$(build_swift_sidecar snaplingo-translate helpers/macos-translate-stub.swift 11.0)"
  echo "[translate-helper] 已生成占位 $OUT"
  exit 0
fi

# -parse-as-library 配合 @main：否则 Swift 会把顶层代码当脚本处理，和 @main 冲突
OUT="$(build_swift_sidecar snaplingo-translate helpers/macos-translate.swift 15.0 -parse-as-library)"

echo "[translate-helper] 已生成 $OUT ($(du -h "$OUT" | cut -f1), $(lipo -archs "$OUT"))"
