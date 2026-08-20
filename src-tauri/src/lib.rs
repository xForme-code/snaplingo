mod capture;
mod collector;
mod commands;
mod config;
mod hooks;
mod localmodel;
mod logging;
mod ocr;
mod permissions;
mod secrets;
mod selection;
mod translate;
mod updater;
mod windows;

use tauri::menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, RunEvent};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use config::TriggerMode;
use std::sync::atomic::{AtomicBool, Ordering};

/// 首次运行要开设置窗口，但必须等到应用 Ready 之后
static PENDING_FIRST_RUN: AtomicBool = AtomicBool::new(false);

pub fn run() {
    logging::init();

    tauri::Builder::default()
        // 菜单栏工具没有窗口，用户重复双击时看不到任何反馈，
        // 还会多出一个托盘图标。这里拦住第二个实例并打开设置窗口作为可见回应。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            log::info!("检测到重复启动，打开设置窗口");
            if let Err(err) = windows::show_settings(app) {
                log::error!("打开设置窗口失败: {err}");
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    // 只在按下时触发，松开时忽略，否则会响两次
                    if event.state() != tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        return;
                    }
                    handle_shortcut(app, &shortcut.to_string());
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::set_config,
            commands::get_meta,
            commands::translate_text,
            commands::capture_selection,
            commands::run_ocr,
            commands::copy_text,
            commands::collector_list,
            commands::collector_add,
            commands::collector_remove,
            commands::collector_clear,
            commands::collector_merged,
            commands::collector_markdown,
            commands::collector_export_item,
            commands::collector_translate_all,
            commands::open_window,
            commands::show_result_window,
            commands::hide_bubble,
            commands::take_pending,
            commands::hide_window,
            commands::download_language_pack,
            commands::language_pack_status,
            commands::local_model_list,
            commands::local_model_download,
            commands::local_model_remove,
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // 配置文件还不存在 = 第一次运行。菜单栏工具首次启动毫无动静，
            // 用户会以为程序没起来，所以主动开一次设置窗口。
            let first_run = !config::config_path().exists();

            // 常驻托盘的工具，不该占着 Dock 图标
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // 权限状态是排查一切「没反应」问题的第一现场，必须记下来
            log::info!(
                "权限状态 — 辅助功能: {} / 屏幕录制: {}",
                if permissions::is_granted(permissions::Permission::Accessibility) {
                    "已授权"
                } else {
                    "未授权"
                },
                if permissions::is_granted(permissions::Permission::ScreenRecording) {
                    "已授权"
                } else {
                    "未授权"
                }
            );

            match build_tray(&handle) {
                Ok(()) => log::info!("托盘图标已创建"),
                // 托盘建不出来，用户就完全没有入口了，必须让它显式可见
                Err(err) => log::error!("托盘创建失败: {err}"),
            }

            register_shortcuts(&handle);
            start_selection_pipeline(&handle);
            apply_autostart(&handle, config::get().autostart);

            // 只在首次运行时提示权限；之后交给功能失败时的按需提示
            permissions::check_on_startup(&handle, first_run);
            updater::check_on_startup(&handle);

            if first_run {
                // 落盘一份默认配置，这样「首次运行」只判定一次
                let _ = config::save(config::get());
                // 注意：不能在 setup 里直接建窗口，此时 macOS 的应用生命周期
                // 还没走完，WKWebView 的内容进程会立刻被终止。推迟到 Ready 事件。
                PENDING_FIRST_RUN.store(true, Ordering::Relaxed);
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("SnapLingo 启动失败")
        .run(|app, event| match event {
            // 关掉所有窗口不等于退出——这是常驻后台的工具。
            //
            // 但只能拦「窗口关完」触发的那次退出：此时 code 是 None。
            // 用户从托盘点「退出」走的是 app.exit(0)，code 是 Some(0)，
            // 必须放行，否则程序就永远关不掉了。
            RunEvent::ExitRequested { api, code, .. } => {
                if code.is_none() {
                    api.prevent_exit();
                }
            }

            // 应用真正就绪后再开首次运行的设置窗口
            RunEvent::Ready => {
                if PENDING_FIRST_RUN.swap(false, Ordering::Relaxed) {
                    if let Err(err) = windows::show_settings(app) {
                        log::error!("首次运行打开设置窗口失败: {err}");
                    }
                }
            }
            _ => {}
        });
}

// ---------------------------------------------------------------- 划词流水线

/// 鼠标钩子 → 取词 → 弹气泡/直接翻译
fn start_selection_pipeline(app: &AppHandle) {
    match hooks::start_mouse_watcher() {
        Ok(()) => log::info!("全局鼠标监听已启动"),
        Err(err) => log::warn!("划词监听未启动：{err}。快捷键触发仍可用。"),
    }

    let Some(receiver) = hooks::take_receiver() else {
        return;
    };
    let handle = app.clone();

    std::thread::Builder::new()
        .name("snaplingo-selection".into())
        .spawn(move || {
            // 借用而不是消费 receiver：下面要在循环体内继续用它清空队列
            for gesture in &receiver {
                // 取词期间新来的手势会在队列里堆积，逐个处理会让气泡越弹越晚。
                // 只保留最后一次——用户关心的永远是刚划的那段。
                let mut gesture = gesture;
                let mut skipped = 0;
                while let Ok(newer) = receiver.try_recv() {
                    gesture = newer;
                    skipped += 1;
                }
                if skipped > 0 {
                    log::debug!("丢弃 {skipped} 个过期手势，只处理最新一次");
                }

                log::debug!("收到划选事件，准备取词");

                // 让目标 App 先完成选区状态更新，否则可能取到上一次的内容
                std::thread::sleep(std::time::Duration::from_millis(90));

                let cfg = config::get();
                let text = match selection::capture_with_mode(selection::CaptureMode::Fast) {
                    Ok(Some(text)) => {
                        log::debug!("取词成功，共 {} 字", text.chars().count());
                        text
                    }
                    Ok(None) => {
                        log::debug!("取词为空（目标 App 可能不支持复制，或实际没有选中内容）");
                        continue;
                    }
                    Err(err) => {
                        log::warn!("取词失败: {err}");
                        continue;
                    }
                };

                let length = text.chars().count();
                if length < cfg.min_length || length > cfg.max_length {
                    log::debug!("文本长度 {length} 超出 [{}, {}]，跳过", cfg.min_length, cfg.max_length);
                    continue;
                }

                let handle = handle.clone();
                let payload = windows::Payload {
                    text,
                    source: "selection".into(),
                    auto_translate: cfg.trigger_mode == TriggerMode::Auto,
                };
                let anchor = (gesture.x, gesture.y);

                // 所有窗口操作必须回到主线程
                let _ = handle.clone().run_on_main_thread(move || {
                    let result = if payload.auto_translate {
                        windows::show_result(&handle, payload, Some(anchor))
                    } else {
                        windows::show_bubble(&handle, payload, Some(anchor))
                    };
                    if let Err(err) = result {
                        log::error!("展示划词结果失败: {err}");
                    }
                });
            }
        })
        .ok();
}

// ---------------------------------------------------------------- 快捷键

fn register_shortcuts(app: &AppHandle) {
    let cfg = config::get();
    let manager = app.global_shortcut();
    let _ = manager.unregister_all();

    let mut registered = Vec::new();
    for accelerator in [
        &cfg.hotkeys.translate,
        &cfg.hotkeys.ocr,
        &cfg.hotkeys.extract,
        &cfg.hotkeys.collect,
        &cfg.hotkeys.collector,
    ] {
        if accelerator.trim().is_empty() {
            continue;
        }
        match manager.register(accelerator.as_str()) {
            Ok(()) => registered.push(accelerator.clone()),
            Err(err) => log::warn!("快捷键 {accelerator} 注册失败（可能被其它程序占用）: {err}"),
        }
    }
    log::info!("已注册快捷键: {}", registered.join(", "));
}

fn handle_shortcut(app: &AppHandle, pressed: &str) {
    let cfg = config::get();
    let matches = |configured: &str| shortcut_eq(configured, pressed);

    // 「按了快捷键没反应」是最难查的一类问题：不记这一行，就分不清
    // 到底是没收到按键、没匹配上、还是后续流程失败了。
    log::info!("收到快捷键: {pressed}");

    if matches(&cfg.hotkeys.translate) {
        translate_selection(app, true);
    } else if matches(&cfg.hotkeys.collect) {
        translate_selection(app, false);
    } else if matches(&cfg.hotkeys.ocr) {
        run_capture_flow(app, true);
    } else if matches(&cfg.hotkeys.extract) {
        run_capture_flow(app, false);
    } else if matches(&cfg.hotkeys.collector) {
        let _ = windows::show_collector(app);
    } else {
        log::warn!(
            "快捷键 {pressed} 没有匹配到任何功能（配置: {:?}）",
            cfg.hotkeys
        );
    }
}

/// 比较两个快捷键字符串是否指同一个组合。
///
/// 必须按「修饰键集合 + 主键」比，不能把分隔符去掉后直接比字符串：
/// 配置里写的是 `Alt+Shift+Z`，而 Tauri 上报的是 `shift+alt+KeyZ`（它有自己的
/// 规范顺序）。按字符串比就是 `altshiftz` vs `shiftaltz`，永远不相等——
/// 这个 bug 让所有快捷键都静默失效过。
fn shortcut_eq(a: &str, b: &str) -> bool {
    match (canonical_shortcut(a), canonical_shortcut(b)) {
        (Some(left), Some(right)) => left == right,
        // 没配（空串）或没有主键的，一律不匹配
        _ => false,
    }
}

/// 拆成（排序后的修饰键, 主键）。无法解析出主键时返回 None。
fn canonical_shortcut(value: &str) -> Option<(Vec<&'static str>, String)> {
    let mut modifiers: Vec<&'static str> = Vec::new();
    let mut main: Option<String> = None;

    for raw in value.split(['+', '-']) {
        let token = raw.trim().to_lowercase();
        if token.is_empty() {
            continue;
        }
        let modifier = match token.as_str() {
            "commandorcontrol" | "cmdorctrl" | "command" | "cmd" | "meta" | "super" => "cmd",
            "control" | "ctrl" => "ctrl",
            "option" | "alt" => "alt",
            "shift" => "shift",
            other => {
                // 主键：Tauri 上报的是 KeyZ / Digit1 这类 code，剥掉前缀
                let stripped = other
                    .strip_prefix("key")
                    .or_else(|| other.strip_prefix("digit"))
                    .unwrap_or(other);
                main = Some(stripped.to_string());
                continue;
            }
        };
        modifiers.push(modifier);
    }

    modifiers.sort_unstable();
    modifiers.dedup();
    main.map(|key| (modifiers, key))
}

/// translate=true 弹翻译面板，false 只加入收集夹
fn translate_selection(app: &AppHandle, show_panel: bool) {
    // 没有辅助功能权限时，模拟复制会静默失败。在用户主动按下快捷键的
    // 这一刻提示，比在启动时干扰他更有用。
    if !permissions::prompt_if_missing(app, permissions::Permission::Accessibility) {
        return;
    }

    let handle = app.clone();
    std::thread::spawn(move || {
        let text = match selection::capture_selected_text() {
            Ok(Some(text)) => text,
            Ok(None) => {
                log::info!("没有取到选中的文字（可能确实没选中，或目标 App 不支持复制）");
                return;
            }
            Err(err) => {
                log::warn!("取词失败: {err}");
                return;
            }
        };

        if show_panel {
            let payload = windows::Payload {
                text,
                source: "selection".into(),
                auto_translate: true,
            };
            let inner = handle.clone();
            let _ = handle.run_on_main_thread(move || {
                // 快捷键触发：面板跟着当前光标走，不要用上一次划词的位置
                windows::forget_anchor();
                let _ = windows::show_result(&inner, payload, None);
            });
        } else {
            collector::add(text, None, None, "selection".into());
            let inner = handle.clone();
            let _ = handle.run_on_main_thread(move || {
                windows::notify_collector_changed(&inner);
                refresh_tray(&inner);
                // 快捷键收集原本全程静默：内容进了收集夹，界面上却什么都不变，
                // 用户没法判断到底成没成。给一个一闪即逝的确认。
                let _ = windows::show_toast(&inner, "选中内容已收集");
            });
        }
    });
}

/// 截图翻译是否正在进行。
///
/// 没有这个闸门时，重复按快捷键会并发起多个 `screencapture -i`，
/// 后来的那个通常会立刻退出、什么都不做——表现就是「按了没反应」。
static OCR_BUSY: AtomicBool = AtomicBool::new(false);

/// 框选一块屏幕 → OCR → 翻译或提取。
///
/// `translate=true` 是「截图翻译」：把图里的外文译过来。
/// `translate=false` 是「截图提取」：把图里的文字、号码、链接抠成可复制的文本，
/// 不做翻译——这才是「我有张图，想把里面的内容拿出来用」的场景。
fn run_capture_flow(app: &AppHandle, translate: bool) {
    let action = if translate { "截图翻译" } else { "截图提取" };

    // 没有屏幕录制权限时 screencapture 会静默产出一张黑图，
    // 与其让用户对着空结果发懵，不如先触发系统授权弹窗。
    if !permissions::is_granted(permissions::Permission::ScreenRecording) {
        log::warn!("{action}：缺少屏幕录制权限，拉起系统授权");
        permissions::request_screen_recording();
        return;
    }

    // 上一次框选还没结束就再按，直接忽略——否则两个框选 UI 会互相顶掉
    if OCR_BUSY.swap(true, Ordering::SeqCst) {
        log::info!("{action}：上一次框选还在进行中，忽略这次触发");
        return;
    }

    log::info!("{action}：开始框选");

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        hooks::suspend(true);
        let result = async {
            let path = capture::select_region().await?;
            let Some(path) = path else {
                return Ok::<Option<String>, anyhow::Error>(None);
            };
            log::debug!("{action}：已截图，开始识别");
            let text = ocr::recognize(&handle, &path).await;
            capture::cleanup(&path);
            text.map(Some)
        }
        .await;
        hooks::suspend(false);
        OCR_BUSY.store(false, Ordering::SeqCst);

        // 无论成败都要给用户一个可见的回应。
        // 之前失败/空结果只写日志，用户什么都看不到，只会以为快捷键坏了。
        let (payload, log_line) = match result {
            Ok(Some(text)) if !text.trim().is_empty() => {
                log::info!("{action}：识别到 {} 字", text.chars().count());
                (
                    // auto_translate=false 时前端走提取，把号码/链接/日期分组列出来
                    windows::Payload { text, source: "ocr".into(), auto_translate: translate },
                    None,
                )
            }
            Ok(Some(_)) => (
                notice("这块区域里没有识别出文字。换一块试试，或者放大一点再截。"),
                Some(format!("{action}：识别结果为空")),
            ),
            Ok(None) => {
                // screencapture 没产出文件：用户按了 Esc，或框选被系统打断
                log::info!("{action}：已取消（没有生成截图）");
                return;
            }
            Err(err) => (
                notice(&format!("{action}失败：{err}")),
                Some(format!("{action}失败: {err}")),
            ),
        };

        if let Some(line) = log_line {
            log::warn!("{line}");
        }

        let inner = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            windows::forget_anchor();
            let _ = windows::show_result(&inner, payload, None);
        });
    });
}

/// 构造一条「只显示提示、不翻译」的面板内容
fn notice(message: &str) -> windows::Payload {
    windows::Payload {
        text: message.to_string(),
        source: "notice".into(),
        auto_translate: false,
    }
}

// ---------------------------------------------------------------- 托盘

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    // 托盘要用纯黑+alpha 的模板图，macOS 才会跟随亮/暗色自动反色。
    // 直接用彩色的 App 图标会被渲染成一坨黑影。
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/trayTemplate.png"))
        .unwrap_or_else(|_| app.default_window_icon().cloned().expect("缺少应用图标"));

    let tray = TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("SnapLingo · 划词翻译")
        .on_menu_event(|app, event| on_tray_menu(app, event.id().as_ref()))
        .build(app)?;

    app.manage(TrayHandle(tray));
    refresh_tray(app);
    Ok(())
}

struct TrayHandle(tauri::tray::TrayIcon);

/// 托盘菜单要反映当前状态（开关、触发方式、收集夹条数），所以每次变更都重建
pub fn refresh_tray(app: &AppHandle) {
    let Some(handle) = app.try_state::<TrayHandle>() else {
        return;
    };
    let cfg = config::get();

    let build = || -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
        let enabled = CheckMenuItemBuilder::with_id("toggle", "启用划词监听")
            .checked(cfg.enabled)
            .build(app)?;

        // 权限状态直接摆在菜单第一屏：没授权时用户一眼能看到问题在哪
        let granted = permissions::is_granted(permissions::Permission::Accessibility);
        let permission_item = MenuItemBuilder::with_id(
            "permissions",
            if granted {
                "辅助功能权限：已授权".to_string()
            } else {
                "⚠️ 辅助功能未授权 —— 点此前往设置".to_string()
            },
        )
        .enabled(!granted)
        .build(app)?;

        let mode = SubmenuBuilder::new(app, "触发方式")
            .item(
                &CheckMenuItemBuilder::with_id("mode:bubble", "划词后显示图标条")
                    .checked(cfg.trigger_mode == TriggerMode::Bubble)
                    .build(app)?,
            )
            .item(
                &CheckMenuItemBuilder::with_id("mode:auto", "划词后自动翻译")
                    .checked(cfg.trigger_mode == TriggerMode::Auto)
                    .build(app)?,
            )
            .item(
                &CheckMenuItemBuilder::with_id("mode:hotkey", "仅快捷键触发")
                    .checked(cfg.trigger_mode == TriggerMode::Hotkey)
                    .build(app)?,
            )
            .build()?;

        MenuBuilder::new(app)
            .item(&permission_item)
            .item(&enabled)
            .separator()
            .item(&MenuItemBuilder::with_id(
                "translate",
                format!("翻译选中内容\t{}", cfg.hotkeys.translate),
            ).build(app)?)
            .item(&MenuItemBuilder::with_id(
                "ocr",
                format!("截图翻译\t{}", cfg.hotkeys.ocr),
            ).build(app)?)
            .item(&MenuItemBuilder::with_id(
                "extract",
                format!("截图提取文字\t{}", cfg.hotkeys.extract),
            ).build(app)?)
            .item(&MenuItemBuilder::with_id(
                "collector",
                format!("收集夹（{}）\t{}", collector::count(), cfg.hotkeys.collector),
            ).build(app)?)
            .separator()
            .item(&mode)
            .item(&MenuItemBuilder::with_id("settings", "设置…").build(app)?)
            .item(&MenuItemBuilder::with_id("update", "检查更新…").build(app)?)
            .separator()
            .item(&MenuItemBuilder::with_id("quit", "退出 SnapLingo").build(app)?)
            .build()
    };

    match build() {
        Ok(menu) => {
            let _ = handle.0.set_menu(Some(menu));
        }
        Err(err) => log::error!("构建托盘菜单失败: {err}"),
    }
}

fn on_tray_menu(app: &AppHandle, id: &str) {
    match id {
        "permissions" => {
            permissions::open_settings_page(app, permissions::Permission::Accessibility)
        }
        "toggle" => {
            let _ = config::update(|c| c.enabled = !c.enabled);
            refresh_tray(app);
        }
        "mode:bubble" => set_mode(app, TriggerMode::Bubble),
        "mode:auto" => set_mode(app, TriggerMode::Auto),
        "mode:hotkey" => set_mode(app, TriggerMode::Hotkey),
        "translate" => translate_selection(app, true),
        "ocr" => run_capture_flow(app, true),
        "extract" => run_capture_flow(app, false),
        "collector" => {
            let _ = windows::show_collector(app);
        }
        "settings" => {
            let _ = windows::show_settings(app);
        }
        "update" => updater::check_manually(app),
        "quit" => app.exit(0),
        _ => {}
    }
}

fn set_mode(app: &AppHandle, mode: TriggerMode) {
    let _ = config::update(|c| c.trigger_mode = mode);
    refresh_tray(app);
}

// ---------------------------------------------------------------- 配置副作用

/// 配置改动后需要立刻生效的部分：快捷键重注册、开机自启、托盘刷新
pub fn apply_config_side_effects(app: &AppHandle, cfg: &config::Config) {
    register_shortcuts(app);
    apply_autostart(app, cfg.autostart);
    refresh_tray(app);
}

fn apply_autostart(app: &AppHandle, enabled: bool) {
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(err) = result {
        log::warn!("设置开机自启失败: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::shortcut_eq;

    /// 这是曾经让全部快捷键静默失效的真实场景：
    /// 配置写 Alt+Shift+Z，系统上报 shift+alt+KeyZ，修饰键顺序相反。
    #[test]
    fn matches_regardless_of_modifier_order() {
        assert!(shortcut_eq("Alt+Shift+Z", "shift+alt+KeyZ"));
        assert!(shortcut_eq("Alt+Shift+T", "shift+alt+KeyT"));
        assert!(shortcut_eq("CommandOrControl+Shift+A", "shift+cmd+KeyA"));
    }

    #[test]
    fn distinguishes_different_keys_and_modifiers() {
        assert!(!shortcut_eq("Alt+Shift+T", "shift+alt+KeyZ"));
        assert!(!shortcut_eq("Alt+Shift+T", "shift+ctrl+KeyT"));
        // 少一个修饰键也不算同一个组合
        assert!(!shortcut_eq("Alt+Shift+T", "alt+KeyT"));
    }

    #[test]
    fn handles_aliases_and_digits() {
        assert!(shortcut_eq("Option+Shift+1", "shift+alt+Digit1"));
        assert!(shortcut_eq("Control+KeyM", "ctrl+m"));
    }

    #[test]
    fn empty_config_never_matches() {
        assert!(!shortcut_eq("", "shift+alt+KeyZ"));
        assert!(!shortcut_eq("   ", "shift+alt+KeyZ"));
        // 只有修饰键、没有主键的也不该匹配
        assert!(!shortcut_eq("Alt+Shift", "shift+alt"));
    }
}
