use serde::Serialize;
use serde_json::Value;
use tauri::AppHandle;

use crate::{capture, collector, config, localmodel, ocr, selection, translate, windows};

/// 统一把 anyhow 错误转成前端能直接显示的字符串
fn to_message(err: impl std::fmt::Display) -> String {
    err.to_string()
}

// ------------------------------------------------------------------ 配置

#[tauri::command]
pub fn get_config() -> config::Config {
    config::get()
}

#[tauri::command]
pub fn set_config(app: AppHandle, patch: Value) -> Result<config::Config, String> {
    // 前端只传改动的字段，这里合并进现有配置
    let mut current = serde_json::to_value(config::get()).map_err(to_message)?;
    if let (Some(base), Some(incoming)) = (current.as_object_mut(), patch.as_object()) {
        for (key, value) in incoming {
            base.insert(key.clone(), value.clone());
        }
    }

    let next: config::Config = serde_json::from_value(current).map_err(to_message)?;
    let saved = config::save(next).map_err(to_message)?;

    crate::apply_config_side_effects(&app, &saved);

    // 广播给所有已打开的窗口。不广播的话，改了主题要把面板关掉重开才生效。
    {
        use tauri::Emitter;
        let _ = app.emit("config:changed", &saved);
    }
    Ok(saved)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    providers: Vec<translate::ProviderInfo>,
    languages: Vec<Language>,
    /// 源语言列表。和 languages 是同一批语言，但 "auto" 的含义不同
    source_languages: Vec<Language>,
    config_path: String,
    platform: &'static str,
    ocr_engine: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Language {
    code: String,
    label: String,
}

#[tauri::command]
pub fn get_meta(app: AppHandle) -> Meta {
    Meta {
        providers: translate::list_providers(&app),
        languages: config::target_languages()
            .into_iter()
            .map(|(code, label)| Language {
                code: code.into(),
                label: label.into(),
            })
            .collect(),
        source_languages: config::source_languages()
            .into_iter()
            .map(|(code, label)| Language {
                code: code.into(),
                label: label.into(),
            })
            .collect(),
        config_path: config::config_path().to_string_lossy().to_string(),
        platform: std::env::consts::OS,
        ocr_engine: if cfg!(target_os = "macos") {
            "Apple Vision（系统自带，离线）"
        } else if cfg!(target_os = "windows") {
            "Windows.Media.Ocr（系统自带，离线）"
        } else {
            "Tesseract（需自行安装）"
        },
    }
}

// ------------------------------------------------------------------ 翻译 / 提取

#[tauri::command]
pub async fn translate_text(
    app: AppHandle,
    text: String,
    source: Option<String>,
    target: Option<String>,
    provider: Option<String>,
) -> Result<translate::Translation, String> {
    translate::translate(
        &app,
        &text,
        source.as_deref(),
        target.as_deref(),
        provider.as_deref(),
    )
        .await
        .map_err(to_message)
}

/// 查询离线语言包状态：installed / needs-download / unsupported / unavailable
#[tauri::command]
pub async fn language_pack_status(
    app: AppHandle,
    source: String,
    target: String,
) -> Result<String, String> {
    use translate::system::Availability;

    let status =
        tokio::task::spawn_blocking(move || translate::system::availability(&app, &source, &target))
        .await
        .map_err(to_message)?;

    Ok(match status {
        Availability::Installed => "installed",
        Availability::NeedsDownload => "needs-download",
        Availability::Unsupported => "unsupported",
        Availability::Unavailable => "unavailable",
    }
    .to_string())
}

/// 触发系统语言包下载（会弹出系统界面等用户确认）
#[tauri::command]
pub async fn download_language_pack(
    app: AppHandle,
    source: String,
    target: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || translate::system::prepare(&app, &source, &target))
        .await
        .map_err(to_message)?
        .map_err(to_message)
}

// ------------------------------------------------------------------ 离线模型

#[tauri::command]
pub fn local_model_list() -> Vec<localmodel::ModelInfo> {
    localmodel::list()
}

/// 下载一个语言方向的离线模型。进度通过 model:progress 事件推给前端。
#[tauri::command]
pub async fn local_model_download(app: AppHandle, id: String) -> Result<(), String> {
    localmodel::download(app, id).await.map_err(to_message)
}

#[tauri::command]
pub fn local_model_remove(id: String) -> Result<(), String> {
    localmodel::remove(&id).map_err(to_message)
}

// ------------------------------------------------------------------ 取词 / OCR

#[tauri::command]
pub async fn capture_selection() -> Result<Option<String>, String> {
    // 取词要模拟按键 + 轮询剪贴板，是阻塞操作，必须挪出异步运行时的工作线程
    tokio::task::spawn_blocking(selection::capture_selected_text)
        .await
        .map_err(to_message)?
        .map_err(to_message)
}

#[tauri::command]
pub async fn run_ocr(app: AppHandle) -> Result<Option<String>, String> {
    crate::hooks::suspend(true);
    let outcome = async {
        let Some(path) = capture::select_region().await.map_err(to_message)? else {
            return Ok(None); // 用户按 Esc 取消
        };
        let text = ocr::recognize(&app, &path).await.map_err(to_message);
        capture::cleanup(&path);
        text.map(Some)
    }
    .await;
    crate::hooks::suspend(false);
    outcome
}

// ------------------------------------------------------------------ 剪贴板

#[tauri::command]
pub fn copy_text(text: String) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut cb| cb.set_text(text))
        .map_err(to_message)
}

// ------------------------------------------------------------------ 收集夹

#[tauri::command]
pub fn collector_list() -> Vec<collector::Item> {
    collector::list()
}

#[tauri::command]
pub fn collector_add(
    app: AppHandle,
    text: String,
    translation: Option<String>,
    target: Option<String>,
    source: String,
) -> collector::Item {
    let item = collector::add(text, translation, target, source);
    windows::notify_collector_changed(&app);
    crate::refresh_tray(&app);
    // 图标条点完就收起来了，光靠按钮上那 620ms 的打勾很容易没看见
    let _ = windows::show_toast(&app, "选中内容已收集");
    item
}

#[tauri::command]
pub fn collector_remove(app: AppHandle, id: String) {
    collector::remove(&id);
    windows::notify_collector_changed(&app);
    crate::refresh_tray(&app);
}

#[tauri::command]
pub fn collector_clear(app: AppHandle) {
    collector::clear();
    windows::notify_collector_changed(&app);
    crate::refresh_tray(&app);
}

#[tauri::command]
pub fn collector_merged(bilingual: bool) -> String {
    if bilingual {
        collector::merged_bilingual()
    } else {
        collector::merged("\n\n")
    }
}

/// 把一条收集内容导出成本地 Markdown 文件。
///
/// 直接落到「下载」文件夹而不是弹保存对话框：这是个「一键导出」，
/// 多一次选路径的交互就不叫一键了。导完在访达里定位到该文件，
/// 用户既知道存哪了，也能马上拖去别处。
#[tauri::command]
pub fn collector_export_item(app: AppHandle, id: String) -> Result<String, String> {
    let (name, markdown) =
        collector::item_markdown(&id).ok_or_else(|| "这条内容已经不在收集夹里了".to_string())?;

    let dir = dirs::download_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| "找不到下载文件夹".to_string())?;
    let path = dir.join(&name);

    std::fs::write(&path, markdown).map_err(|e| format!("写入文件失败: {e}"))?;

    // 在访达里选中它。这一步本身就是最好的反馈——文件和它的位置都摆在眼前，
    // 再弹一个「已导出」的提示纯属重复告知。
    {
        use tauri_plugin_opener::OpenerExt;
        let _ = app.opener().reveal_item_in_dir(&path);
    }

    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn collector_markdown() -> String {
    collector::to_markdown()
}

#[tauri::command]
pub async fn collector_translate_all(
    app: AppHandle,
    target: Option<String>,
) -> Result<Vec<collector::Item>, String> {
    for item in collector::list() {
        if item.translation.is_some() {
            continue; // 已经翻过的跳过，省钱也省时间
        }
        let result = translate::translate(&app, &item.text, None, target.as_deref(), None)
            .await
            .map_err(to_message)?;
        collector::set_translation(&item.id, result.text, result.target);
    }
    windows::notify_collector_changed(&app);
    Ok(collector::list())
}

// ------------------------------------------------------------------ 窗口

#[tauri::command]
pub fn open_window(app: AppHandle, name: String) -> Result<(), String> {
    match name.as_str() {
        "collector" => windows::show_collector(&app).map_err(to_message),
        "settings" => windows::show_settings(&app).map_err(to_message),
        other => Err(format!("未知窗口: {other}")),
    }
}

#[tauri::command]
pub fn show_result_window(
    app: AppHandle,
    text: String,
    source: String,
    auto_translate: bool,
) -> Result<(), String> {
    windows::show_result(
        &app,
        windows::Payload {
            text,
            source,
            auto_translate,
        },
        None, // 沿用上一次划词记下的锚点
    )
    .map_err(to_message)
}

#[tauri::command]
pub fn hide_bubble(app: AppHandle) {
    windows::hide_bubble(&app);
}

/// 窗口加载完后主动来取内容。
///
/// 走「拉」而不是「推」：窗口是现建的，emit 发出时前端往往还没注册监听器。
#[tauri::command]
pub fn take_pending(label: String) -> Option<windows::Payload> {
    windows::take_pending(&label)
}

#[tauri::command]
pub fn hide_window(app: AppHandle, label: String) {
    windows::hide_window(&app, &label);
}
