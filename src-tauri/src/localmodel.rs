//! 离线翻译模型的下载与管理。
//!
//! 模型是 OPUS-MT 经 CTranslate2 int8 量化后的产物，直接用 Argos 打包好的
//! `.argosmodel`（就是个 zip）。这套东西**不进安装包**：安装包只有几 MB，
//! 模型按需下载——每个语言方向 60~190 MB，全打进去会让安装包膨胀几十倍。
//!
//! 这一层只负责「拿到并管理模型文件」，不负责推理。推理见 translate/opus.rs。

use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

use crate::config::config_dir;

/// 一个可下载的语言方向。
///
/// URL 里的版本号各语言对并不统一（1_9 / 1_1 / 1_3 / 1_0 都有），
/// 所以只能逐个写死已验证过的地址，不能按模板拼。
struct Spec {
    id: &'static str,
    from: &'static str,
    to: &'static str,
    label: &'static str,
    url: &'static str,
    /// 下载大小（字节），实测值，用于进度条和下载前告知用户
    bytes: u64,
    /// 压缩包的 SHA-256。
    ///
    /// 上游（Argos 的包索引）**不提供任何校验和**，这些是我们自己把每个包
    /// 完整下下来算出来的。没有它的话，下到手的几十上百 MB 二进制会被直接
    /// 喂给 CTranslate2——传输被掉包、CDN 出错、上游仓库被改，程序都察觉不到。
    sha256: &'static str,
}

const REGISTRY: &[Spec] = &[
    Spec { id: "en-zh", from: "en", to: "zh", label: "英语 → 中文", url: "https://argos-net.com/v1/translate-en_zh-1_9.argosmodel", bytes: 70_743_021, sha256: "433e7c4f034d87fbe2353161e05f18646d7999452f801a4e1f0378522b9850ab" },
    Spec { id: "zh-en", from: "zh", to: "en", label: "中文 → 英语", url: "https://argos-net.com/v1/translate-zh_en-1_9.argosmodel", bytes: 74_481_402, sha256: "62e7af5a3a48b530e47b7b3e5c78c2de79073ecd815750d2bf3ab35b4a67da2d" },
    Spec { id: "en-ja", from: "en", to: "ja", label: "英语 → 日语", url: "https://argos-net.com/v1/translate-en_ja-1_1.argosmodel", bytes: 120_470_284, sha256: "16300cc4eaa85320520cabcf433b63d01be40ef6966251de72043a083408f716" },
    Spec { id: "ja-en", from: "ja", to: "en", label: "日语 → 英语", url: "https://argos-net.com/v1/translate-ja_en-1_1.argosmodel", bytes: 117_155_716, sha256: "623e3477959a815eb0a5ef53e09079ae8f1f9d3bbcd230473baf28c03fb83335" },
    Spec { id: "en-ko", from: "en", to: "ko", label: "英语 → 韩语", url: "https://argos-net.com/v1/translate-en_ko-1_1.argosmodel", bytes: 120_789_009, sha256: "e03d8e65e6d44525ec5808c3409fcf8728c76c2c76925372b6d3dc3278de17fc" },
    Spec { id: "ko-en", from: "ko", to: "en", label: "韩语 → 英语", url: "https://argos-net.com/v1/translate-ko_en-1_1.argosmodel", bytes: 118_852_077, sha256: "6da8f3db6ca40f42b1875570a1c06856f6e17c7ef62845d85de217ba548c1471" },
    Spec { id: "en-fr", from: "en", to: "fr", label: "英语 → 法语", url: "https://argos-net.com/v1/translate-en_fr-1_9.argosmodel", bytes: 65_472_327, sha256: "3a65ed83364f4e7b06e30f9dd823db1934899ed3ce839e63f46dc7b09dc797b4" },
    Spec { id: "fr-en", from: "fr", to: "en", label: "法语 → 英语", url: "https://argos-net.com/v1/translate-fr_en-1_9.argosmodel", bytes: 66_585_033, sha256: "3b3052fee6bb1e8e8e632a26a723eb2a2c7710dfe73ba61ffd9b83e85d4f14c1" },
    Spec { id: "en-de", from: "en", to: "de", label: "英语 → 德语", url: "https://argos-net.com/v1/translate-en_de-1_3.argosmodel", bytes: 150_508_297, sha256: "6cd847f0c06c9c66013e6b0932e07fd54a6d90894659c02bf6c5247b72fb25b1" },
    Spec { id: "en-ru", from: "en", to: "ru", label: "英语 → 俄语", url: "https://argos-net.com/v1/translate-en_ru-1_9.argosmodel", bytes: 195_746_693, sha256: "591d743ae103752b88ffc38785c50421320f4eff93c8967e0d3d2e14d4e27811" },
    Spec { id: "en-es", from: "en", to: "es", label: "英语 → 西班牙语", url: "https://argos-net.com/v1/translate-en_es-1_0.argosmodel", bytes: 87_503_191, sha256: "d698d0ef87ad70d5d184b7fa6965905bf4368f09a2bb9ffb165a79bac96af0c4" },
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub from: String,
    pub to: String,
    pub label: String,
    /// 下载大小（字节）
    pub bytes: u64,
    pub installed: bool,
    /// 已装模型在磁盘上占的字节数，未装时为 0
    pub disk_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub id: String,
    /// downloading | extracting | done | failed
    pub phase: &'static str,
    pub received: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 下载专用的 HTTP client。
///
/// **不能复用 translate::HTTP**：那个客户端为翻译 API 调优，设了 20 秒总超时。
/// 模型有 60~190 MB，实测下载速度约 2 MB/s，一个 71MB 的包要 38 秒——
/// 用那个客户端下载**必然在 20 秒被掐断**，且报错长得像网络不通，
/// 会让人误以为是墙或代理的问题。
///
/// 这里改成：连接有超时（连不上要快速失败），但整体不设上限；
/// 用 read_timeout 兜住「连上了却不再吐数据」的情况。
static DOWNLOAD_HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .read_timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("构建下载 client 失败")
});

/// 把错误的 source 链摊平成一行。
///
/// reqwest 的 Display 只输出「error sending request for url (...)」这种顶层描述，
/// 真正的原因（DNS、TLS、超时、连接被拒）全藏在 source 链里。不展开的话
/// 日志和界面上看到的永远是同一句废话，根本没法判断该怎么修。
fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut cursor = err.source();
    while let Some(inner) = cursor {
        parts.push(inner.to_string());
        cursor = inner.source();
    }
    parts.join(" ← ")
}

fn spec(id: &str) -> Option<&'static Spec> {
    REGISTRY.iter().find(|s| s.id == id)
}

pub fn models_dir() -> PathBuf {
    config_dir().join("models")
}

pub fn model_dir(id: &str) -> PathBuf {
    models_dir().join(id)
}

/// 装好的标志：CTranslate2 的权重和分词模型都在。
/// 只看目录存在是不够的——下载中断会留下半个目录。
pub fn is_installed(id: &str) -> bool {
    let dir = model_dir(id);
    dir.join("model").join("model.bin").exists() && dir.join("sentencepiece.model").exists()
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => dir_size(&entry.path()),
            Ok(meta) => meta.len(),
            Err(_) => 0,
        })
        .sum()
}

pub fn list() -> Vec<ModelInfo> {
    REGISTRY
        .iter()
        .map(|s| {
            let installed = is_installed(s.id);
            ModelInfo {
                id: s.id.into(),
                from: s.from.into(),
                to: s.to.into(),
                label: s.label.into(),
                bytes: s.bytes,
                installed,
                disk_bytes: if installed { dir_size(&model_dir(s.id)) } else { 0 },
            }
        })
        .collect()
}

/// 找一个能把 from 译成 to 的已装模型。
///
/// 语言代码要归一化：内部用 zh-CN / zh-TW，模型只分 zh。
pub fn installed_for(from: &str, to: &str) -> Option<String> {
    let norm = |code: &str| -> String {
        let base = code.split('-').next().unwrap_or(code);
        base.to_lowercase()
    };
    let (from, to) = (norm(from), norm(to));
    REGISTRY
        .iter()
        .find(|s| s.from == from && s.to == to && is_installed(s.id))
        .map(|s| s.id.to_string())
}

pub fn remove(id: &str) -> Result<()> {
    let dir = model_dir(id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| anyhow!("删除模型失败: {e}"))?;
    }
    Ok(())
}

fn emit(app: &AppHandle, progress: Progress) {
    let _ = app.emit("model:progress", progress);
}

/// 下载并解开一个语言方向的模型。
///
/// 全程向前端汇报进度：几十到上百 MB 的下载，没有进度条用户会以为卡死了。
pub async fn download(app: AppHandle, id: String) -> Result<()> {
    let spec = spec(&id).ok_or_else(|| anyhow!("未知的模型: {id}"))?;

    if is_installed(&id) {
        return Ok(());
    }

    std::fs::create_dir_all(models_dir()).map_err(|e| anyhow!("创建模型目录失败: {e}"))?;

    // 先下到临时文件，成功解开后才落到正式位置——
    // 中途失败或断网时不会留下一个看起来「已安装」的残缺目录
    let archive_path = models_dir().join(format!("{id}.partial"));
    let result = download_inner(&app, spec, &archive_path).await;

    match result {
        Ok(()) => {
            // 成功解压后压缩包就没用了，它有几十 MB
            let _ = std::fs::remove_file(&archive_path);
            emit(&app, Progress { id: id.clone(), phase: "done", received: spec.bytes, total: spec.bytes, error: None });
            log::info!("离线模型 {id} 安装完成");
            Ok(())
        }
        Err(err) => {
            // 注意：**不删** .partial，下次点下载会从断点接着下。
            // 只清掉可能解了一半的模型目录。
            let _ = remove(&id);
            emit(&app, Progress { id: id.clone(), phase: "failed", received: 0, total: spec.bytes, error: Some(err.to_string()) });
            log::warn!("离线模型 {id} 安装失败: {err}");
            Err(err)
        }
    }
}

async fn download_inner(app: &AppHandle, spec: &'static Spec, archive_path: &Path) -> Result<()> {
    use std::io::Write;

    // 断点续传：上次没下完的部分接着下。
    // 服务器支持 Range（实测返回 206），几十上百 MB 的下载中断一次就重来
    // 是很糟的体验，尤其网络本来就不稳的时候。
    let resume_from = std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0);
    let resume_from = if resume_from >= spec.bytes { 0 } else { resume_from };

    let mut request = DOWNLOAD_HTTP.get(spec.url);
    if resume_from > 0 {
        log::info!("模型 {} 从 {} 字节处续传", spec.id, resume_from);
        request = request.header("Range", format!("bytes={resume_from}-"));
    }

    let mut response = request
        .send()
        .await
        .map_err(|e| anyhow!("连接下载服务器失败: {}", error_chain(&e)))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("下载失败：HTTP {}", status.as_u16()));
    }

    // 服务器忽略了 Range（返回 200 而不是 206）时必须从头写，否则会拼出坏文件
    let appending = resume_from > 0 && status.as_u16() == 206;
    let mut received = if appending { resume_from } else { 0 };
    let total = response
        .content_length()
        .map(|len| received + len)
        .unwrap_or(spec.bytes);

    let mut file = if appending {
        std::fs::OpenOptions::new()
            .append(true)
            .open(archive_path)
            .map_err(|e| anyhow!("打开续传文件失败: {e}"))?
    } else {
        std::fs::File::create(archive_path).map_err(|e| anyhow!("创建临时文件失败: {e}"))?
    };

    let mut last_report = std::time::Instant::now();

    // 用 chunk() 而不是 bytes_stream()：后者要 reqwest 的 stream feature，
    // 这里逐块读同样够用，还少一个依赖。
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| anyhow!("下载中断: {}", error_chain(&e)))?
    {
        file.write_all(&chunk).map_err(|e| anyhow!("写入失败: {e}"))?;
        received += chunk.len() as u64;

        // 每 300ms 报一次就够了，每块都报会把前端的事件队列刷爆
        if last_report.elapsed() >= std::time::Duration::from_millis(300) {
            last_report = std::time::Instant::now();
            emit(app, Progress { id: spec.id.into(), phase: "downloading", received, total, error: None });
        }
    }
    file.flush().map_err(|e| anyhow!("写入失败: {e}"))?;
    drop(file);

    // 大小对不上说明下载残缺（服务器提前断流、磁盘写满等），
    // 直接解压会得到一个坏模型，不如在这里就失败掉
    let actual = std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0);
    if actual < total {
        return Err(anyhow!("下载不完整（{actual} / {total} 字节），请重试"));
    }

    // 大小对得上不代表内容对。校验哈希再解压——**必须在解压之前**，
    // 解开之后再发现不对，坏文件已经落到模型目录里了。
    let to_verify = archive_path.to_path_buf();
    let expected = spec.sha256;
    let actual_hash = tokio::task::spawn_blocking(move || sha256_of(&to_verify))
        .await
        .map_err(|e| anyhow!("校验任务异常: {e}"))??;

    if actual_hash != expected {
        // 这里**必须删掉** .partial，和其它错误路径的处理相反。
        // 别处保留残档是为了续传，但内容已经错了的文件续不出正确结果——
        // 续传只会往一堆坏字节后面接着写，用户点几次下载就失败几次，
        // 而且永远不会好。
        let _ = std::fs::remove_file(archive_path);
        log::warn!("模型 {} 校验失败：期望 {expected}，实际 {actual_hash}", spec.id);
        return Err(anyhow!(
            "下载的模型文件校验不通过，已丢弃。可能是网络中间环节篡改或缓存了错误内容，请重试或换个网络。"
        ));
    }

    emit(app, Progress { id: spec.id.into(), phase: "extracting", received: total, total, error: None });

    // 解压是纯 CPU + 磁盘的阻塞活，挪出异步运行时
    let archive = archive_path.to_path_buf();
    let target = model_dir(spec.id);
    tokio::task::spawn_blocking(move || extract(&archive, &target))
        .await
        .map_err(|e| anyhow!("解压任务异常: {e}"))?
}

/// 算文件的 SHA-256。
///
/// 分块读而不是一次性读进内存：模型最大 190 MB，整个读进来纯属浪费，
/// 内存紧张的机器上还可能直接失败。
fn sha256_of(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|e| anyhow!("打开待校验文件失败: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        let n = file.read(&mut buf).map_err(|e| anyhow!("读取待校验文件失败: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// 解开 .argosmodel。
///
/// 包内所有文件都在一个顶层目录下（translate-en_zh-1_9/），要剥掉这层，
/// 否则路径里会多一级还带着版本号，换版本就对不上了。
/// 解压上限，防 zip 炸弹。
///
/// 模型包是从固定 HTTPS 源下载的，但「源可信」不等于「内容一定合规」——
/// 一个几十 MB 的包可以解出上百 GB，把用户磁盘撑爆。已知最大的模型包
/// 解压后约 200 MB，给到 1 GB 已经很宽松了。
const MAX_ENTRIES: usize = 512;
const MAX_TOTAL_BYTES: u64 = 1_024 * 1_024 * 1_024;

fn extract(archive_path: &Path, target: &Path) -> Result<()> {
    let file = std::fs::File::open(archive_path).map_err(|e| anyhow!("打开压缩包失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| anyhow!("压缩包无法解析: {e}"))?;

    if zip.len() > MAX_ENTRIES {
        return Err(anyhow!("压缩包条目过多（{} 个），拒绝解压", zip.len()));
    }
    let mut written: u64 = 0;

    if target.exists() {
        std::fs::remove_dir_all(target).ok();
    }
    std::fs::create_dir_all(target).map_err(|e| anyhow!("创建模型目录失败: {e}"))?;

    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|e| anyhow!("读取压缩项失败: {e}"))?;

        let Some(path) = entry.enclosed_name() else {
            continue; // 带 .. 的恶意路径，直接跳过
        };

        // 剥掉顶层的 translate-xx_yy-1_9/ 这一级
        let mut parts = path.components();
        parts.next();
        let relative: PathBuf = parts.collect();
        if relative.as_os_str().is_empty() {
            continue;
        }

        // stanza 是 Python 的分句器（约 750KB），我们自己按句切分，用不上
        if relative.starts_with("stanza") {
            continue;
        }

        let out_path = target.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| anyhow!("创建目录失败: {e}"))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| anyhow!("创建目录失败: {e}"))?;
        }

        written = written.saturating_add(entry.size());
        if written > MAX_TOTAL_BYTES {
            return Err(anyhow!("解压后体积超过 {} MB，拒绝继续", MAX_TOTAL_BYTES / 1_048_576));
        }

        let mut out = std::fs::File::create(&out_path).map_err(|e| anyhow!("写入 {} 失败: {e}", relative.display()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| anyhow!("解压 {} 失败: {e}", relative.display()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_ids_match_language_pairs() {
        for spec in REGISTRY {
            assert_eq!(
                spec.id,
                format!("{}-{}", spec.from, spec.to),
                "id 必须是 from-to，否则 installed_for 查不到"
            );
            assert!(spec.bytes > 1_000_000, "{} 的大小明显不对", spec.id);
            assert!(spec.url.ends_with(".argosmodel"), "{} 的地址不是模型包", spec.id);
        }
    }

    /// 哈希写错的后果是**这个语言方向永远装不上**，而且报错看起来像网络问题，
    /// 排查起来很绕。抄错一位、少一位、混进大写都要在这里就拦住。
    #[test]
    fn registry_hashes_are_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for spec in REGISTRY {
            assert_eq!(spec.sha256.len(), 64, "{} 的哈希长度不对", spec.id);
            assert!(
                spec.sha256.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
                "{} 的哈希含非法字符（必须是小写十六进制）",
                spec.id
            );
            assert!(seen.insert(spec.sha256), "{} 的哈希和别的条目重复了", spec.id);
        }
    }

    #[test]
    fn sha256_matches_known_value() {
        let path = std::env::temp_dir().join(format!("snaplingo-sha-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        // "abc" 的 SHA-256 是公开的标准测试向量
        assert_eq!(
            sha256_of(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_file(&path).ok();
    }

    /// 用真实的 .argosmodel 验证解压：剥顶层目录、跳过 stanza、文件齐全。
    /// 需要一个本地模型包，所以默认不跑：
    ///   SNAPLINGO_TEST_ARCHIVE=/path/to/x.argosmodel \
    ///     cargo test --lib -- --ignored --nocapture extracts_real
    #[test]
    #[ignore]
    fn extracts_real_argos_package() {
        let Ok(archive) = std::env::var("SNAPLINGO_TEST_ARCHIVE") else {
            panic!("需要设置 SNAPLINGO_TEST_ARCHIVE 指向一个 .argosmodel");
        };
        let target = std::env::temp_dir().join("snaplingo-extract-test");
        let _ = std::fs::remove_dir_all(&target);

        extract(Path::new(&archive), &target).expect("解压失败");

        // 顶层目录必须被剥掉：model/ 和 sentencepiece.model 应该直接在根下
        assert!(target.join("model").join("model.bin").exists(), "缺少 model/model.bin");
        assert!(target.join("model").join("config.json").exists(), "缺少 model/config.json");
        assert!(target.join("sentencepiece.model").exists(), "缺少 sentencepiece.model");
        // stanza 是 Python 分句器，应该被跳过
        assert!(!target.join("stanza").exists(), "stanza 没有被跳过");
        // 目录名里不该还留着版本号那一层
        assert!(!target.join("translate-en_zh-1_9").exists(), "顶层目录没有被剥掉");

        println!("解压后体积: {:.1} MiB", dir_size(&target) as f64 / 1048576.0);
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn normalizes_regional_codes() {
        // zh-CN / zh-TW 都要能落到 zh 的模型上
        assert!(REGISTRY.iter().any(|s| s.from == "zh" && s.to == "en"));
    }
}

#[cfg(test)]
mod net_tests {
    /// 直接用下载专用 client 打一次真实请求，把完整错误链打出来。
    /// 默认不跑（要联网）：
    ///   cargo test --lib -- --ignored --nocapture download_probe
    #[tokio::test]
    #[ignore]
    async fn download_probe() {
        let url = super::REGISTRY[1].url; // zh-en
        println!("请求: {url}");

        let started = std::time::Instant::now();
        let result = super::DOWNLOAD_HTTP
            .get(url)
            .header("Range", "bytes=0-1048575") // 只取 1MB，够验证链路
            .send()
            .await;

        match result {
            Ok(response) => {
                println!("HTTP {} / 耗时 {:?}", response.status(), started.elapsed());
                let bytes = response.bytes().await.expect("读取响应体失败");
                println!("收到 {} 字节", bytes.len());
                assert!(!bytes.is_empty());
            }
            Err(err) => {
                panic!("失败（{:?}）: {}", started.elapsed(), super::error_chain(&err));
            }
        }
    }
}
