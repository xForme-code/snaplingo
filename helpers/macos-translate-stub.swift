// macOS 15 以下的占位组件。
//
// 为什么需要它：tauri.conf.json 的 externalBin 无条件要求这个 sidecar 存在，
// 而真正的实现依赖 Translation 框架（macOS 15+）。低版本上不生成的话，
// **构建脚本会直接失败**——别人在 macOS 14 上 clone 仓库就编不了。
//
// 所以这里给一个总是回报「不可用」的替身：构建能过，运行时如实说明情况，
// Rust 侧据此回落到 OPUS-MT 或云端引擎。
import Foundation

print(#"{"ok":false,"status":"unavailable","text":"","error":"系统翻译需要 macOS 15 或更新版本"}"#)
exit(1)
