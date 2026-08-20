#!/usr/bin/env bash
# 编 Swift sidecar 的公共部分，供 build-ocr-helper.sh / build-translate-helper.sh 引用。
#
# 两件事必须一起对，错一个用户那边就是「功能毫无反应」：
#   · 文件名后缀 —— Tauri 靠 `<名字>-<target triple>` 找 sidecar，对不上就打包失败；
#   · 二进制架构 —— 以前这里写死 arm64，可后缀取的是 rustc 的 host triple。
#     在 Intel Mac 上就会产出「名字叫 x86_64、内容是 arm64」的东西，
#     打包能过，一运行 exec format error。
#
# 目标由 TARGET_TRIPLE 指定，缺省是 rustc 的 host。universal-apple-darwin
# 会分别编两个架构再 lipo 合并（swiftc 一次只能编一个架构）。

sidecar_triple() {
  echo "${TARGET_TRIPLE:-$(rustc -vV | awk '/^host:/ {print $2}')}"
}

# 用法: build_swift_sidecar <名字> <源文件> <最低系统版本> [额外的 swiftc 参数...]
build_swift_sidecar() {
  local name="$1" src="$2" minos="$3"
  shift 3

  local triple arches
  triple="$(sidecar_triple)"
  case "$triple" in
    aarch64-apple-darwin)   arches=(arm64) ;;
    x86_64-apple-darwin)    arches=(x86_64) ;;
    universal-apple-darwin) arches=(arm64 x86_64) ;;
    *) echo "[sidecar] 不认识的目标 $triple" >&2; return 1 ;;
  esac

  mkdir -p src-tauri/binaries
  local out="src-tauri/binaries/${name}-${triple}"

  # 必须显式指定部署目标！swiftc 默认用**编译机器**的系统版本，
  # 在 macOS 26 上编出来的产物 minos 就是 26.0，装到别人 macOS 14/15 的
  # 机器上会直接拒绝启动（dyld: app requires macOS 26.0 or later）。
  if [[ ${#arches[@]} -eq 1 ]]; then
    swiftc -O -target "${arches[0]}-apple-macos${minos}" "$@" "$src" -o "$out"
  else
    local parts=() arch
    for arch in "${arches[@]}"; do
      swiftc -O -target "${arch}-apple-macos${minos}" "$@" "$src" -o "${out}.${arch}"
      parts+=("${out}.${arch}")
    done
    lipo -create "${parts[@]}" -output "$out"
    rm -f "${parts[@]}"
  fi

  chmod +x "$out"
  echo "$out"
}
