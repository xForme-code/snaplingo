# SnapLingo

划词翻译 / 截图翻译 / 文字提取工具。

**当前只发布 macOS 版本**（universal 二进制，Apple Silicon 与 Intel 通用；
Intel 那一半只在 CI 上编译验证过，没有实机跑过）。Windows / Linux 的代码路径
已就位并能编译，但截图框选尚未接通、也没有在真机验证过，暂不提供构建产物。

常驻菜单栏（系统托盘），不占 Dock。

---

## 界面

划词后弹出的图标条，贴着光标出现：

<img src="assets/screenshots/bubble-light.png" width="252" alt="划词图标条">

翻译面板 · 截图提取面板（浅色 / 深色）：

| 翻译 | 截图提取 |
|---|---|
| <img src="assets/screenshots/translate-light.png" width="380" alt="翻译面板"> | <img src="assets/screenshots/extract-light.png" width="380" alt="提取面板"> |
| <img src="assets/screenshots/translate-dark.png" width="380" alt="翻译面板深色"> | <img src="assets/screenshots/extract-dark.png" width="380" alt="提取面板深色"> |

收集夹——多段暂存、批量翻译、单条导出 Markdown：

<img src="assets/screenshots/collector-light.png" width="760" alt="收集夹">

设置：

<img src="assets/screenshots/settings-light.png" width="700" alt="设置">

外观支持**跟随系统 / 浅色 / 深色**三种，在设置里切换，所有窗口即时生效。

---

## 安装

macOS：下载 [Releases](../../releases) 里的 `.dmg`，拖进「应用程序」。

> **首次打开会被系统拦下。** 本项目目前用自签名证书，没有做 Apple 公证，
> 所以 Gatekeeper 会提示「无法验证开发者」。绕过方法：
> 双击 → 点「完成」→ 打开**系统设置 → 隐私与安全性** → 下滑找到 SnapLingo → 点「**仍要打开**」。
>
> 注意 macOS 15 起，老教程说的「右键 → 打开」已经失效，只能走系统设置。
>
> 首次使用还需在**隐私与安全性 → 辅助功能**里勾选 SnapLingo（划词取词的前提），
> 截图翻译另需**屏幕录制**权限。

装好之后会**自动检查更新**：启动 20 秒后静默查一次，有新版本才弹窗询问，
装不装由你决定。也可以从托盘菜单「检查更新…」手动触发。

各引擎的 API Key 存在 **macOS 钥匙串**里，不写进 `config.json`——配置文件是明文的，
贴 issue、丢进同步盘、被备份工具抄走都很常见，密钥不该跟着一起走。老版本留在
`config.json` 里的明文密钥会在首次启动时自动搬进钥匙串并把文件里的抹掉。

---

## 功能

| 功能 | 触发方式 |
|---|---|
| 划词翻译 | 鼠标拖选任意文字 → 弹出图标条 → 点「翻译」 |
| 快捷键翻译 | 选中文字后按 `⌥⇧T`（Win/Linux: `Alt+Shift+T`） |
| 截图翻译 | `⌥⇧A` 框选屏幕区域 → OCR 识别 → **翻译** |
| 截图提取 | `⌥⇧E` 框选屏幕区域 → OCR 识别 → **原样抠出文字供复制**，不翻译 |
| 多段收集 | `⌥⇧C` 逐条收集 → `⌥⇧D` 打开收集夹 → 批量翻译 / 合并复制 / 单条导出 Markdown 文件 |

快捷键可在设置里自定义（点击后直接按组合键录制）。支持开机自启。

### 离线翻译

**断网、代理失效、云端限流时仍然可用**，两层兜底自动接管，无需手动切换：

| 层 | 引擎 | 体积 | 说明 |
|---|---|---|---|
| 1 | macOS 系统翻译框架 | **0**（系统管理语言包） | macOS 15+。端上推理，质量接近云端 |
| 2 | OPUS-MT（CTranslate2 int8） | 按语言方向 60~190 MB，**按需下载** | 跨平台。长句接近 Google，短句偏生硬 |

引擎链是「**联网优先、断网回落**」：云端质量更好，能用就用；4 秒内连不上就自动切本地，
并进入 60 秒冷却期，避免断网时每次划词都白等。

### 支持的翻译引擎

| 引擎 | 需要 Key | 国内直连 |
|---|---|---|
| 系统翻译（离线） | 否 | 不需要联网 |
| 离线模型 OPUS-MT | 否 | 不需要联网 |
| Google | 否 | ✗ 需代理 |
| 有道翻译 | 是 | ✓ |
| 百度翻译 | 是 | ✓ |
| OpenAI 兼容接口 | 是 | 取决于服务商 |
| DeepL | 是 | ✗ 需代理 |
| Claude | 是 | ✗ 需代理 |
| LibreTranslate | 否（自建） | ✓ 自建即可 |

「OpenAI 兼容接口」只需改接口地址，即可接入 DeepSeek / Kimi / 智谱 / 通义 / OpenRouter，
以及 Ollama、LM Studio 等本地服务。

---

## 关键设计决策

### 为什么是 Tauri 而不是 Electron

这是个常驻后台的小工具，内存占用是硬指标。

| | Electron | Tauri（本项目） |
|---|---|---|
| 安装包 | ~120 MB | **14 MB**（debug 构建的 DMG） |
| 常驻内存 | ~200 MB | **~78 MB**（debug；release 更低） |

同类开源项目 [pot-desktop](https://github.com/pot-app/pot-desktop) 同样选择 Tauri，实测常驻约 80MB，与本项目一致。

### 取词：模拟复制 + 哨兵法

跨平台没有统一的「读取选区」API，所以走通用方案：

```
备份剪贴板 → 写入哨兵串 → 模拟 Ctrl/Cmd+C → 轮询到剪贴板变化 → 还原剪贴板
```

用哨兵串而不是「比较前后文本是否相同」，是因为用户完全可能复制一段和剪贴板里一模一样的文字，那样就会误判为「复制失败」。整个过程对用户剪贴板**无副作用**。

按键模拟用 [`enigo`](https://crates.io/crates/enigo)（三端原生），不依赖 xdotool 之类的外部命令。

### OCR：一律用系统自带引擎

免费、离线、零模型下载、用完即退不常驻内存。

| 平台 | 引擎 | 说明 |
|---|---|---|
| macOS | Apple Vision framework | 30 种语言，中文准确率极高，helper 二进制仅 104KB |
| Windows | `Windows.Media.Ocr` | 系统自带，通过 PowerShell 调用 |
| Linux | Tesseract | 系统无内置，需自行安装 |

macOS 实测（中英混排测试图，识别耗时约 0.5s）：

```
深空笔记 · 划词翻译工具 SnapLingo
Select text anywhere, translate instantly.
邮箱 support@snaplingo.dev｜电话 13800138000｜2026年8月15日
金额 ¥1,299.00 / $199 · IP 192.168.1.100
```

文字内容全部正确。

### 翻译引擎

默认 **Google 免费端点**，无需任何配置。其余引擎不填 Key 就不启用，不增加使用负担：

| 引擎 | 是否需要 Key | 备注 |
|---|---|---|
| Google | 否 | 非官方接口，高频可能限流，网络受限地区可能不通 |
| LibreTranslate | 否 | 建议 Docker 自建，数据不出内网 |
| DeepL | 是 | 免费版每月 50 万字符 |
| Claude | 是 | 长句 / 专业术语质量最好，按量付费 |

> 实测提醒：微软 Edge 免费翻译端点、有道网页端点、LibreTranslate 各公共镜像**当前均已不可用**。
> 想要不依赖 Google 的退路，请自建 LibreTranslate：
> ```bash
> docker run -ti --rm -p 5000:5000 libretranslate/libretranslate
> ```

### 标识符拆词

翻译前把 `user_name` / `getUserInfo` / `get-user-info` 拆成自然语言，否则翻译引擎会把整个标识符当成不认识的词原样吐回。这个细节参考自 [STranslate](https://github.com/STranslate/STranslate)。

只在整段看起来像单个标识符时才处理，不会破坏正常句子。

---

## 权限设置

### macOS

两个权限，都在 **系统设置 → 隐私与安全性**：

| 权限 | 用途 | 何时需要 |
|---|---|---|
| 辅助功能 | 模拟复制取词 + 监听鼠标划选 | 划词翻译的前提，**必须** |
| 屏幕录制 | 截取屏幕区域做 OCR | 只有用截图翻译才需要 |

授权后**必须重启本程序**才生效（macOS 的权限缓存机制所致）。

### Windows / Linux（尚未发布，以下为规划）

> 这两个平台**目前没有构建产物**，下面的内容是代码里已经写好的设计，
> 但**没有在真机上验证过**。等有可用版本时会更新这一节。

**Windows**：不需要额外授权。OCR 走系统自带的 `Windows.Media.Ocr`，
中文识别依赖系统语言包（设置 → 时间和语言 → 语言和区域 → 添加中文语言包）。

**Linux**

依赖：

```bash
# Ubuntu / Debian
sudo apt install libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
                 librsvg2-dev patchelf libxdo-dev tesseract-ocr tesseract-ocr-chi-sim
```

⚠️ **Wayland 限制**：Ubuntu 22.04+ 的 GNOME 默认使用 Wayland，其安全模型**不允许程序监听全局鼠标事件**，因此「划词自动触发」无法工作，全局快捷键也可能失效。这是协议层面的限制，不是本程序的 bug（[pot-desktop 也有同样问题](https://github.com/pot-app/pot-desktop)）。

解决办法任选其一：
- 登录界面选择 **“Ubuntu on Xorg”** 会话
- 安装 `ydotool` 并启动 `ydotoold` 服务

程序检测到 Wayland 会主动提示。

---

## 开发

```bash
brew install cmake   # 必需：ct2rs 会从源码编译 CTranslate2（离线翻译引擎）
npm install
npm run build:helpers   # 编译 macOS 的 OCR / 系统翻译 sidecar
npm run icons           # 生成应用 / 托盘图标
npm run dev             # 开发模式
npm run install:macos   # 构建 + 固定证书签名 + 装到 ~/Applications
```

**cmake 是硬性前提**，缺了直接构建失败。首次编译 CTranslate2 约 1 分钟，之后走缓存。

`sidecar` 是本机编译产物（带 target triple 后缀），不进版本库，拉下代码后需要自己生成。

跨平台打包需要在对应系统上编译：macOS 上打不出 Windows 安装包。

发布用的 `scripts/release-macos.sh` 默认出 universal 二进制（一个包同时给
Apple Silicon 和 Intel）。只想要本机架构的话：

```bash
TARGET_TRIPLE=aarch64-apple-darwin bash scripts/release-macos.sh
```

### 测试

```bash
cd src-tauri
cargo test --lib                        # 单元测试
cargo test --lib -- --ignored --nocapture   # 含真实网络请求的集成测试
```

### 目录结构

```
src/                   前端（纯 HTML/CSS/JS，无打包工具链）
  api.js               与 Rust 通信的唯一入口
  bubble.*             划词后的小图标条
  result.*             翻译 / 提取结果面板
  collector.*          收集夹
  settings.*           设置（含快捷键录制）
src-tauri/src/
  config.rs            配置读写
  selection.rs         取词（哨兵法）+ 标识符拆词
  hooks.rs             全局鼠标钩子（rdev）
  permissions.rs       权限自检与引导
  capture.rs           截图
  ocr.rs               各平台 OCR 分发
  secrets.rs           API Key 存取（macOS 走 Keychain）
  collector.rs         收集夹存储
  localmodel.rs        离线模型下载与管理（断点续传）
  translate/           翻译引擎（system / opus / google / youdao / baidu / openai / deepl / claude / libre）
  windows.rs           窗口管理
  commands.rs          暴露给前端的命令
  lib.rs               托盘 / 快捷键 / 划词流水线
helpers/macos-ocr.swift   macOS Vision OCR helper
```

---

## 当前状态

**已验证可用**
- 四个翻译引擎的调用与解析（Google 已跑通真实请求）
- macOS Vision OCR（实测中英混排全对）
- 标识符拆词、OCR 文本清洗（单元测试覆盖）
- 应用启动、托盘常驻、内存占用

**待你在真机上验证**（需要授权后才能测）
- 划词自动触发、快捷键取词、截图框选

**尚未接线**
- Windows / Linux 的截图框选遮罩（macOS 直接复用了系统自带的框选 UI）

---

## 参考

方案调研参考了以下开源项目，仅参考思路未使用其代码：

- [pot-desktop](https://github.com/pot-app/pot-desktop)（GPL-3.0）— Tauri 选型、三端 OCR 引擎方案、Linux 依赖清单
- [STranslate](https://github.com/STranslate/STranslate) — 标识符分隔符预处理
- [Easydict](https://github.com/tisfeng/Easydict) — 自动语言识别策略
