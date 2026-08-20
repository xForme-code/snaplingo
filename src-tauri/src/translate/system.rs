//! 系统内置翻译引擎（macOS 15+ 的 Translation 框架）。
//!
//! 端上推理：不联网、不要 API Key、不受网络环境影响、也没有调用成本。
//! 这是本项目在 macOS 上的默认引擎，其余引擎都属于「想要更好质量时的可选项」。
//!
//! 实现走 sidecar 子进程（helpers/macos-translate.swift），原因和 OCR 一样：
//! 苹果的 TranslationSession 只能通过 SwiftUI 拿到，没法直接从 Rust FFI 调。

use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::Translation;

#[derive(Debug, Deserialize)]
struct HelperOutput {
    ok: bool,
    #[serde(default)]
    text: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// 语言包状态。对应 Apple 的 LanguageAvailability.Status。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// 语言包已下载，可以直接翻译
    Installed,
    /// 系统支持这个语言对，但语言包还没下载
    NeedsDownload,
    /// 系统根本不支持这个语言对
    Unsupported,
    /// 没有系统翻译能力（macOS 15 以下，或非 macOS）
    Unavailable,
}

/// 我们内部用 zh-CN / zh-TW 这类代码，Apple 要的是 zh-Hans / zh-Hant
fn to_apple_code(code: &str) -> &str {
    match code {
        "zh-CN" | "zh" => "zh-Hans",
        "zh-TW" | "zh-HK" => "zh-Hant",
        other => other,
    }
}

/// 定出查询语言包状态时用的源语言。
///
/// 查「语言包装没装」必须指名具体的语言对，不能用 auto。用户明确选了源语言
/// 就照用（最准）；选的是自动检测时才退回到猜测——按目标语言反推：
/// 译成中文的多半是英文原文，译成其它语言的多半是中文原文。
fn resolve_source(source: &str, target_apple: &str) -> String {
    if source != "auto" && !source.is_empty() {
        return to_apple_code(source).to_string();
    }
    if target_apple.starts_with("zh") {
        "en".to_string()
    } else {
        "zh-Hans".to_string()
    }
}

#[cfg(target_os = "macos")]
fn run_helper(app: &tauri::AppHandle, args: Vec<String>, stdin: Option<String>) -> Result<HelperOutput> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use tauri::Manager;

    // sidecar 在 bundle 里和主程序同目录；开发时（cargo run）不存在，
    // 这种情况下退回到 binaries/ 目录里的构建产物。
    let exe = app
        .path()
        .resolve("snaplingo-translate", tauri::path::BaseDirectory::Resource)
        .ok()
        .filter(|p| p.exists())
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join("snaplingo-translate")))
                .filter(|p| p.exists())
        })
        .ok_or_else(|| anyhow!("找不到系统翻译组件 snaplingo-translate"))?;

    let mut child = Command::new(&exe)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("启动系统翻译组件失败: {e}"))?;

    if let Some(text) = stdin {
        if let Some(mut pipe) = child.stdin.take() {
            let _ = pipe.write_all(text.as_bytes());
            // 必须显式关闭，helper 读到 EOF 才会往下走
            drop(pipe);
        }
    } else {
        drop(child.stdin.take());
    }

    let output = child
        .wait_with_output()
        .map_err(|e| anyhow!("系统翻译组件执行失败: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim())
        .map_err(|e| anyhow!("系统翻译组件输出无法解析: {e}（原始输出: {}）", stdout.trim()))
}

/// 查询某个目标语言的语言包状态
#[cfg(target_os = "macos")]
pub fn availability(app: &tauri::AppHandle, source: &str, target: &str) -> Availability {
    let target_apple = to_apple_code(target);
    let source_apple = resolve_source(source, target_apple);

    let parsed = match run_helper(
        app,
        vec!["--check".into(), source_apple, target_apple.into()],
        None,
    ) {
        Ok(parsed) => parsed,
        Err(err) => {
            log::warn!("查询系统翻译语言包状态失败: {err}");
            return Availability::Unavailable;
        }
    };

    match parsed.status.as_deref() {
        Some("installed") => Availability::Installed,
        Some("supported") => Availability::NeedsDownload,
        Some("unsupported") => Availability::Unsupported,
        _ => Availability::Unavailable,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn availability(_app: &tauri::AppHandle, _source: &str, _target: &str) -> Availability {
    Availability::Unavailable
}

/// 弹出系统的语言包下载界面。需要用户点确认，所以会阻塞较久。
#[cfg(target_os = "macos")]
pub fn prepare(app: &tauri::AppHandle, source: &str, target: &str) -> Result<()> {
    let target_apple = to_apple_code(target);
    let source_apple = resolve_source(source, target_apple);

    let parsed = run_helper(
        app,
        vec!["--prepare".into(), source_apple, target_apple.into()],
        None,
    )?;

    if !parsed.ok {
        return Err(anyhow!(
            "下载语言包失败：{}",
            parsed.error.unwrap_or_else(|| "未知错误".into())
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn prepare(_app: &tauri::AppHandle, _source: &str, _target: &str) -> Result<()> {
    Err(anyhow!("当前系统没有内置翻译能力"))
}

/// 翻译。语言包没就绪时返回带 needs-download 标记的错误，交给上层决定怎么提示。
#[cfg(target_os = "macos")]
pub fn translate_blocking(
    app: &tauri::AppHandle,
    text: &str,
    source: &str,
    target: &str,
) -> Result<Translation> {
    let target_apple = to_apple_code(target);
    // 传 auto 时 helper 会把 source 设成 nil，由系统自己识别
    let source_arg = if source == "auto" || source.is_empty() {
        "auto".to_string()
    } else {
        to_apple_code(source).to_string()
    };

    let parsed = run_helper(
        app,
        vec![source_arg, target_apple.into()],
        Some(text.to_string()),
    )?;

    if !parsed.ok {
        if parsed.status.as_deref() == Some("needs-download") {
            return Err(anyhow!("{NEEDS_DOWNLOAD}"));
        }
        return Err(anyhow!(
            "系统翻译失败：{}",
            parsed.error.unwrap_or_else(|| "未知错误".into())
        ));
    }

    Ok(Translation {
        text: parsed.text,
        provider: "系统翻译".into(),
        target: target.to_string(),
        // 系统自动识别了源语言但不回报，这里不猜
        detected_source: None,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn translate_blocking(
    _app: &tauri::AppHandle,
    _text: &str,
    _source: &str,
    _target: &str,
) -> Result<Translation> {
    Err(anyhow!("当前系统没有内置翻译能力"))
}

/// sidecar 是否真的存在且可用。
///
/// 只判断 `cfg!(target_os = "macos")` 是不够的：翻译 helper 需要 macOS 15+，
/// 构建脚本在更低版本上会跳过生成它。那种情况下系统翻译在列表里显示为可用，
/// 用户选了却直接报错，而且明确选择离线引擎时不会回落云端。
pub fn sidecar_available(app: &tauri::AppHandle) -> bool {
    #[cfg(target_os = "macos")]
    {
        use tauri::Manager;

        // 光看文件在不在不够：macOS 15 以下打包的是「如实回报不可用」的占位组件，
        // 文件存在但用不了。必须同时看系统版本，否则用户会选到一个必然失败的引擎。
        if macos_major() < 15 {
            return false;
        }
        let by_resource = app
            .path()
            .resolve("snaplingo-translate", tauri::path::BaseDirectory::Resource)
            .ok()
            .is_some_and(|p| p.exists());
        let by_exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("snaplingo-translate")))
            .is_some_and(|p| p.exists());
        by_resource || by_exe
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        false
    }
}

/// 当前 macOS 主版本号。查一次就够，结果不会变。
#[cfg(target_os = "macos")]
fn macos_major() -> u32 {
    use std::sync::OnceLock;
    static VERSION: OnceLock<u32> = OnceLock::new();

    *VERSION.get_or_init(|| {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|text| text.trim().split('.').next()?.parse().ok())
            // 读不到就当成够新，让后续调用去暴露真实问题，
            // 而不是在这里一刀切地判定不可用
            .unwrap_or(u32::MAX)
    })
}

/// 前端靠这个标记识别「需要下载语言包」，从而显示下载引导而不是普通报错
pub const NEEDS_DOWNLOAD: &str = "NEEDS_LANGUAGE_PACK";

/// 语言包状态缓存。
///
/// 查一次要起一个 sidecar 子进程（约 1 秒），不能每次翻译都查。但也不能永久
/// 缓存——用户可能刚在系统设置里装好语言包，得让它在可接受的时间内被发现。
static AVAILABILITY_CACHE: Lazy<Mutex<HashMap<String, (Availability, Instant)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

const CACHE_TTL: Duration = Duration::from_secs(300);

/// 带缓存的语言包状态查询。
///
/// 存在的意义：回落链在试系统翻译之前必须先问一句「语言包装了吗」。
/// 不问就直接调用的话，系统会弹出它自己的「下载语言以翻译」对话框——
/// 用户明明已经下好了我们的离线模型，却被一个无关的系统弹框打断。
pub fn availability_cached(app: &tauri::AppHandle, source: &str, target: &str) -> Availability {
    let key = format!("{source}->{target}");

    if let Ok(cache) = AVAILABILITY_CACHE.lock() {
        if let Some((status, at)) = cache.get(&key) {
            if at.elapsed() < CACHE_TTL {
                return *status;
            }
        }
    }

    let status = availability(app, source, target);
    if let Ok(mut cache) = AVAILABILITY_CACHE.lock() {
        cache.insert(key, (status, Instant::now()));
    }
    status
}

/// sidecar 是阻塞式的，挪到线程池里去跑，别占住异步运行时
pub async fn translate(
    app: tauri::AppHandle,
    text: String,
    source: String,
    target: String,
) -> Result<Translation> {
    tokio::task::spawn_blocking(move || translate_blocking(&app, &text, &source, &target))
        .await
        .map_err(|e| anyhow!("系统翻译任务异常: {e}"))?
}
