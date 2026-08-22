#!/usr/bin/env bash
# 构建 → 签名 → 生成更新清单 → 发布到 GitHub Release
#
# 用法: bash scripts/release-macos.sh [版本号]
#   版本号省略时用 tauri.conf.json 里的值。
#   默认出 universal 二进制（Apple Silicon + Intel 一个包通吃）；
#   想只出本机架构的话 TARGET_TRIPLE=aarch64-apple-darwin bash scripts/release-macos.sh
#
# 两套签名不要混淆：
#   · 代码签名（SnapLingo Dev Signing）→ 让 macOS 允许运行
#   · 更新签名（~/.snaplingo-keys/updater.key）→ 让旧版本相信这个更新包是你发的
# 两者都必须有，缺一个用户就装不上或收不到更新。
set -euo pipefail

cd "$(dirname "$0")/.."

KEY="$HOME/.snaplingo-keys/updater.key"
CERT="SnapLingo Dev Signing"
# 仓库 2026-08-22 从 snaplingo 改名成 SnapLingo。GitHub 对旧路径做 301 跳转，
# 所以 v0.5.0 及更早的用户（更新地址是编译进去的旧 URL）还能正常收到更新。
# **绝不能再建一个叫 snaplingo 的仓库**——那会顶掉跳转，老用户从此断更。
REPO="xForme-code/SnapLingo"

[[ -f "$KEY" ]] || { echo "[error] 找不到更新签名私钥: $KEY"; exit 1; }

VERSION="${1:-$(python3 -c 'import json;print(json.load(open("src-tauri/tauri.conf.json"))["version"])')}"
TAG="v$VERSION"
export TARGET_TRIPLE="${TARGET_TRIPLE:-universal-apple-darwin}"
echo "[release] 版本 $TAG，目标 $TARGET_TRIPLE"

if [[ "$TARGET_TRIPLE" == *universal* || "$TARGET_TRIPLE" == x86_64* ]]; then
  rustup target add x86_64-apple-darwin >/dev/null
  # ct2rs 0.10 的 build.rs 按**宿主**架构决定 CMAKE_OSX_ARCHITECTURES，
  # 在 Apple Silicon 上交叉编 x86_64 会传成 arm64，和 --target=x86_64 打架，
  # ruy 的 AVX 代码路径报 unsupported option '-mavx'。用工具链文件强行掰回来。
  # 变量名带目标后缀（cmake-rs 认下划线形式），只对 x86_64 那一半生效——
  # universal 的 arm64 那一半本来就是对的，套上去反而会编错架构。
  export CMAKE_TOOLCHAIN_FILE_x86_64_apple_darwin="$PWD/scripts/cmake/x86_64-apple-darwin.cmake"
fi

# C/C++ 依赖（CTranslate2、sentencepiece）默认按**编译机器**的系统版本设
# -mmacosx-version-min。Rust 那边是 11.0，两边对不上会一路警告；更要命的是
# Intel Mac 根本没有 macOS 26，x86_64 那半边真按 26 打上去就没人能启动。
export MACOSX_DEPLOYMENT_TARGET=11.0

# ---------------------------------------------------------------- 构建
echo "[build] 编译 sidecar"
bash scripts/build-ocr-helper.sh
bash scripts/build-translate-helper.sh

echo "[build] 构建并签名（同时产出 DMG 和更新包）"
export TAURI_SIGNING_PRIVATE_KEY="$KEY"   # v2 认这个名字，值可以是路径或密钥内容
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
export APPLE_SIGNING_IDENTITY="$CERT"
npx -y @tauri-apps/cli@^2 build --target "$TARGET_TRIPLE" --bundles dmg,app

BUNDLE="src-tauri/target/$TARGET_TRIPLE/release/bundle"
# Tauri 的 DMG 命名里带架构（SnapLingo_0.5.0_aarch64.dmg / _x64 / _universal），
# 与其猜后缀不如直接找——猜错了脚本会在最后一步才炸。
# 统一改名成 SnapLingo-<版本>.dmg：用 mv 不用 cp，否则磁盘上留两份一模一样的
# 安装包，看的人会以为构建出了两个不同的东西。
DMG_RAW="$(ls "$BUNDLE/dmg/SnapLingo_${VERSION}_"*.dmg 2>/dev/null | head -1 || true)"
DMG="$BUNDLE/dmg/SnapLingo-${VERSION}.dmg"
TARBALL="$BUNDLE/macos/SnapLingo.app.tar.gz"
SIGFILE="$TARBALL.sig"

[[ -n "$DMG_RAW" ]] || { echo "[error] 没找到 DMG: $BUNDLE/dmg/SnapLingo_${VERSION}_*.dmg"; exit 1; }
for f in "$TARBALL" "$SIGFILE"; do
  [[ -f "$f" ]] || { echo "[error] 缺少产物: $f"; exit 1; }
done

# 产物架构如实报出来：universal 少了一半 slice 的话，Intel 用户装上去
# 是「打不开」，而这里不看根本发现不了。
ARCHS="$(lipo -archs "$BUNDLE/macos/SnapLingo.app/Contents/MacOS/SnapLingo")"
echo "[build] 主程序架构: $ARCHS"

mv "$DMG_RAW" "$DMG"

# ---------------------------------------------------------------- 更新清单
# 文件名必须和上传到 Release 的一致，否则旧版本下载会 404。
# 这里和 DMG 用同一套命名，Release 页面看起来才整齐。
ASSET="SnapLingo-${VERSION}.app.tar.gz"
mv "$TARBALL" "$BUNDLE/macos/$ASSET"

echo "[manifest] 生成 latest.json"
python3 - "$VERSION" "$SIGFILE" "$REPO" "$TAG" "$ASSET" "$ARCHS" > "$BUNDLE/latest.json" <<'PY'
import json, sys, datetime
version, sigfile, repo, tag, asset, archs = sys.argv[1:7]
signature = open(sigfile).read().strip()
url = f"https://github.com/{repo}/releases/download/{tag}/{asset}"

# platforms 的键必须和产物里真有的架构对上。多写一个键，那个架构的用户会
# 下载到跑不了的包；少写一个，那批用户就永远收不到更新。所以照 lipo 报出来的
# slice 生成，不写死。darwin-universal 是 Tauri 对 universal 包的额外识别键。
mapping = {"arm64": "darwin-aarch64", "x86_64": "darwin-x86_64"}
slices = archs.split()
platforms = {mapping[a]: {"signature": signature, "url": url} for a in slices if a in mapping}
if len(slices) > 1:
    platforms["darwin-universal"] = {"signature": signature, "url": url}

print(json.dumps({
    "version": version,
    "notes": f"SnapLingo {version}",
    "pub_date": datetime.datetime.now(datetime.timezone.utc).isoformat().replace("+00:00", "Z"),
    "platforms": platforms,
}, ensure_ascii=False, indent=2))
PY

# ---------------------------------------------------------------- 发布
echo "[publish] 上传到 GitHub Release $TAG"
# 发布说明：优先用 NOTES_FILE 指定的文件，没给就生成一份最简的。
# 不能缺省读 /dev/stdin —— 非交互环境下 gh 会一直挂着等输入。
if [[ -n "${NOTES_FILE:-}" && -f "${NOTES_FILE}" ]]; then
  NOTES_ARG=(--notes-file "$NOTES_FILE")
else
  NOTES_ARG=(--notes "SnapLingo $TAG")
fi

if gh release view "$TAG" >/dev/null 2>&1; then
  echo "[publish] Release 已存在，覆盖上传资产"
  gh release upload "$TAG" "$DMG" "$BUNDLE/macos/$ASSET" "$BUNDLE/latest.json" --clobber
else
  gh release create "$TAG" \
    "$DMG" "$BUNDLE/macos/$ASSET" "$BUNDLE/latest.json" \
    --title "SnapLingo $TAG" "${NOTES_ARG[@]}"
fi

echo ""
echo "[done] https://github.com/$REPO/releases/tag/$TAG"
echo "旧版本会从 releases/latest/download/latest.json 读到这次更新。"
