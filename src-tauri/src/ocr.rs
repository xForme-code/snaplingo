use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::path::Path;
use tauri::AppHandle;

use crate::config;

#[derive(Debug, Deserialize)]
struct SidecarOutput {
    ok: bool,
    text: String,
    #[serde(default)]
    error: Option<String>,
}

/// 各平台都用系统自带的 OCR 引擎：免费、离线、零模型下载、用完即退不常驻。
///   macOS   → Vision.framework（sidecar，实测中文准确率极高）
///   Windows → Windows.Media.Ocr（系统自带，走 PowerShell 调用）
///   Linux   → tesseract（发行版仓库里有，需用户自行安装）
pub async fn recognize(app: &AppHandle, image_path: &Path) -> Result<String> {
    let text = recognize_platform(app, image_path).await?;
    Ok(normalize(&text))
}

#[cfg(target_os = "macos")]
async fn recognize_platform(app: &AppHandle, image_path: &Path) -> Result<String> {
    use tauri_plugin_shell::ShellExt;

    let languages = config::get().ocr_languages.join(",");
    let output = app
        .shell()
        .sidecar("snaplingo-ocr")
        .map_err(|e| anyhow!("找不到 OCR 组件: {e}"))?
        .args([image_path.to_string_lossy().to_string(), languages])
        .output()
        .await
        .map_err(|e| anyhow!("OCR 组件执行失败: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: SidecarOutput = serde_json::from_str(stdout.trim()).map_err(|e| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow!("OCR 输出无法解析: {e}\n{stderr}")
    })?;

    if !parsed.ok {
        return Err(anyhow!(
            "OCR 失败: {}",
            parsed.error.unwrap_or_else(|| "未知错误".into())
        ));
    }
    Ok(parsed.text)
}

/// 语言标签只允许字母、数字和连字符（BCP-47 的合法字符集）。
///
/// 这个值来自设置里可自由编辑的文本框，而 Windows 的 OCR 走 PowerShell
/// **脚本字符串拼接**——不校验的话，一个引号就能跳出字符串上下文执行任意命令。
/// 路径那边做了转义，语言这边原来漏了。
pub(crate) fn is_valid_language_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 35 // BCP-47 实际长度远小于此
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[cfg(target_os = "windows")]
async fn recognize_platform(_app: &AppHandle, image_path: &Path) -> Result<String> {
    use tokio::process::Command;

    // Windows.Media.Ocr 是系统自带的 WinRT API，通过 PowerShell 调用，
    // 不需要用户额外安装任何东西。识别语言取决于系统已装的语言包。
    let language = config::get()
        .ocr_languages
        .first()
        .cloned()
        .unwrap_or_else(|| "zh-Hans".into());

    // 宁可退回默认值也不能把可疑内容拼进脚本
    let language = if is_valid_language_tag(&language) {
        language
    } else {
        log::warn!("OCR 语言标签 {language:?} 不合法，退回 zh-Hans");
        "zh-Hans".to_string()
    };

    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
[Windows.Media.Ocr.OcrEngine,Windows.Foundation,ContentType=WindowsRuntime] | Out-Null
[Windows.Graphics.Imaging.BitmapDecoder,Windows.Foundation,ContentType=WindowsRuntime] | Out-Null
[Windows.Storage.StorageFile,Windows.Foundation,ContentType=WindowsRuntime] | Out-Null

Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {{
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and
    $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
}})[0]

function Await($op, $type) {{
    $task = $asTask.MakeGenericMethod($type).Invoke($null, @($op))
    $task.Wait(-1) | Out-Null
    $task.Result
}}

$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync('{path}')) ([Windows.Storage.StorageFile])
$stream = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])

$lang = [Windows.Globalization.Language]::new('{language}')
$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromLanguage($lang)
if ($null -eq $engine) {{ $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages() }}
if ($null -eq $engine) {{ throw 'no OCR engine available for the requested language' }}

$result = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])
$result.Lines | ForEach-Object {{ $_.Text }}
"#,
        path = image_path.to_string_lossy().replace('\'', "''"),
        language = language,
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .await
        .map_err(|e| anyhow!("调用 PowerShell 失败: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "Windows OCR 失败：{}\n提示：需要在「设置 → 时间和语言 → 语言」中安装对应语言的 OCR 组件。",
            stderr.trim()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(target_os = "linux")]
async fn recognize_platform(_app: &AppHandle, image_path: &Path) -> Result<String> {
    use tokio::process::Command;

    // Linux 没有系统级 OCR，用发行版仓库里的 tesseract
    let langs = config::get()
        .ocr_languages
        .iter()
        .map(|l| match l.as_str() {
            "zh-Hans" => "chi_sim",
            "zh-Hant" => "chi_tra",
            "en-US" => "eng",
            "ja-JP" => "jpn",
            "ko-KR" => "kor",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("+");

    let output = Command::new("tesseract")
        .args([&image_path.to_string_lossy(), "stdout", "-l", &langs])
        .output()
        .await
        .map_err(|_| {
            anyhow!(
                "未找到 tesseract。请先安装：\n  \
                 Ubuntu/Debian: sudo apt install tesseract-ocr tesseract-ocr-chi-sim\n  \
                 Fedora: sudo dnf install tesseract tesseract-langpack-chi_sim\n  \
                 Arch: sudo pacman -S tesseract tesseract-data-chi_sim"
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("tesseract 识别失败：{}", stderr.trim()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// OCR 输出的常见毛病：CJK 字符之间被插空格、行尾留空白、空行过多。
/// 这里只做保守清理，不改动实际文字内容。
pub(crate) fn normalize(text: &str) -> String {
    let cleaned: Vec<String> = text
        .lines()
        .map(|line| {
            let mut out = String::with_capacity(line.len());
            let chars: Vec<char> = line.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let ch = chars[i];
                if ch == ' ' || ch == '\t' {
                    // 只在「前后都是 CJK」时才吃掉空格
                    let prev = out.chars().last();
                    let next = chars[i + 1..].iter().find(|c| **c != ' ' && **c != '\t');
                    if matches!((prev, next), (Some(p), Some(n)) if is_cjk(p) && is_cjk(*n)) {
                        i += 1;
                        continue;
                    }
                }
                out.push(ch);
                i += 1;
            }
            out.trim_end().to_string()
        })
        .collect();

    // 连续空行压成一个
    let mut result: Vec<String> = Vec::with_capacity(cleaned.len());
    let mut blank_run = 0;
    for line in cleaned {
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        result.push(line);
    }

    result.join("\n").trim().to_string()
}

fn is_cjk(c: char) -> bool {
    let code = c as u32;
    (0x3040..=0x30FF).contains(&code)
        || (0x3400..=0x4DBF).contains(&code)
        || (0x4E00..=0x9FFF).contains(&code)
        || (0xF900..=0xFAFF).contains(&code)
        || (0xAC00..=0xD7AF).contains(&code)
        || (0x3000..=0x303F).contains(&code) // CJK 标点
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn removes_spaces_between_cjk_only() {
        assert_eq!(normalize("深 空 笔 记"), "深空笔记");
        // 中英之间的空格必须保留
        assert_eq!(normalize("深空笔记 SnapLingo"), "深空笔记 SnapLingo");
        // 纯英文完全不动
        assert_eq!(normalize("hello world"), "hello world");
    }

    #[test]
    fn collapses_blank_runs() {
        assert_eq!(normalize("a\n\n\n\nb"), "a\n\nb");
    }
}

#[cfg(test)]
mod language_tests {
    use super::is_valid_language_tag;

    #[test]
    fn accepts_real_bcp47_tags() {
        for tag in ["zh-Hans", "en-US", "ja", "zh-Hant-TW", "pt-BR"] {
            assert!(is_valid_language_tag(tag), "{tag} 应该被接受");
        }
    }

    #[test]
    fn rejects_powershell_injection_attempts() {
        // Windows 的 OCR 把这个值拼进脚本字符串，引号能跳出上下文执行命令
        for tag in [
            "zh'); Remove-Item C:\\ -Recurse; ('",
            "en_US; calc.exe",
            "zh Hans",     // 空格
            "zh$(whoami)", // 子表达式
            "",
        ] {
            assert!(!is_valid_language_tag(tag), "{tag:?} 应该被拒绝");
        }
    }
}
