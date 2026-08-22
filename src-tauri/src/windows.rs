use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
    WindowEvent,
};

pub const BUBBLE: &str = "bubble";
pub const RESULT: &str = "result";
pub const COLLECTOR: &str = "collector";
pub const SETTINGS: &str = "settings";
pub const TOAST: &str = "toast";

// 图标条宽度要正好裹住内容：翻译/复制/收集三项 + 分隔线 + 关闭。
// 「提取」已经移到截图流程里，不在划词上下文中出现。
// 窗口比可见内容大一圈：多出来的是给 CSS 阴影的透明余量。
// 不留这圈余量，阴影会被窗口边界裁成硬边灰矩形。
const BUBBLE_W: f64 = 252.0;
const BUBBLE_H: f64 = 64.0;

// 翻译面板贴着光标弹出，尺寸按「一屏能读完一段」来定，不做成大窗口
/// 窗口离光标的间距，以及窗口内那层 CSS margin。
///
/// place_near 摆的是**窗口**，用户看到的却是窗口内缩进一层 margin 之后的内容，
/// 所以传给它的间距要先把 margin 减掉，否则视觉间距会凭空多出一个 margin。
/// 写成两个常量相减而不是直接写差值：差值是多少不重要，「间距减 margin」
/// 这件事才是要留给下一个人看的。
const BUBBLE_GAP: f64 = 18.0;
const BUBBLE_MARGIN: f64 = 14.0;
const RESULT_GAP: f64 = 16.0;
const RESULT_MARGIN: f64 = 16.0;

const RESULT_W: f64 = 420.0;
const RESULT_H: f64 = 360.0;

// 操作确认提示。宽度按最长的一句文案留，窗口比可见内容大一圈装阴影。
const TOAST_W: f64 = 240.0;
const TOAST_H: f64 = 76.0;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    pub text: String,
    /// "selection" | "ocr"
    pub source: String,
    pub auto_translate: bool,
}

/// 待取的载荷，按窗口 label 存放。
///
/// 不能只靠 `emit` 把数据推给窗口：窗口是现建的，此刻 WebView 还没加载完
/// JS，监听器根本没注册，事件直接丢失——表现就是面板空着、不会自动翻译。
/// 所以数据先落在这里，前端加载完主动来取；`emit` 退化成一个「有新内容了」
/// 的信号，给已经开着的窗口用。
static PENDING: Lazy<Mutex<HashMap<&'static str, Payload>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// 最近一次划词手势的位置，翻译面板也要贴着它弹出。
///
/// 面板可能是用户点了图标条上的「翻译」才打开的，那条链路经过前端，
/// 手势坐标传不过去，所以在这里存一份。
static LAST_ANCHOR: Lazy<Mutex<Option<(f64, f64)>>> = Lazy::new(|| Mutex::new(None));

fn put_pending(label: &'static str, payload: Payload) {
    if let Ok(mut map) = PENDING.lock() {
        map.insert(label, payload);
    }
}

/// 前端取走载荷（取走即清空，避免下次刷新时重复加载旧内容）
pub fn take_pending(label: &str) -> Option<Payload> {
    let mut map = PENDING.lock().ok()?;
    let key = match label {
        BUBBLE => BUBBLE,
        RESULT => RESULT,
        TOAST => TOAST,
        _ => return None,
    };
    map.remove(key)
}

pub fn remember_anchor(anchor: (f64, f64)) {
    if let Ok(mut slot) = LAST_ANCHOR.lock() {
        *slot = Some(anchor);
    }
}

/// 丢弃记下的手势位置。
///
/// 快捷键和截图翻译不是划词触发的，上一次划词的坐标可能是几分钟前、
/// 甚至另一个屏幕上的位置，拿来定位面板会很突兀——清掉它，让面板
/// 回退到当前光标处弹出。
pub fn forget_anchor() {
    if let Ok(mut slot) = LAST_ANCHOR.lock() {
        *slot = None;
    }
}

/// 拿一个用于定位弹窗的坐标：优先用记下的手势位置，其次查实时光标
fn anchor_or_cursor(app: &AppHandle, anchor: Option<(f64, f64)>, scale: f64) -> (f64, f64) {
    anchor
        .or_else(|| LAST_ANCHOR.lock().ok().and_then(|slot| *slot))
        .or_else(|| app.cursor_position().ok().map(|p| (p.x / scale, p.y / scale)))
        .unwrap_or((0.0, 0.0))
}

/// 把窗口摆在锚点下方，并保证整体留在屏幕内
fn place_near(
    window: &tauri::WebviewWindow,
    cursor: (f64, f64),
    size: (f64, f64),
    gap: f64,
) -> (f64, f64) {
    let mut x = cursor.0 - size.0 / 2.0;
    let mut y = cursor.1 + gap;

    // 坐标直接传逻辑值，不要乘缩放系数。
    // 之前乘了 scale：在 3024x1964 的 Retina 屏上（逻辑 1512x982）查询点会算到
    // (1690,886)，落在逻辑边界之外，于是查不到显示器，整段边界收拢被静默跳过。
    if let Ok(Some(monitor)) = window.monitor_from_point(cursor.0, cursor.1) {
        let area = monitor.size().to_logical::<f64>(monitor.scale_factor());
        let origin = monitor.position().to_logical::<f64>(monitor.scale_factor());

        let min_x = origin.x + 6.0;
        let max_x = origin.x + area.width - size.0 - 6.0;
        let min_y = origin.y + 6.0;
        let max_y = origin.y + area.height - size.1 - 6.0;

        // 窗口比可用区域还高（或还宽）时 max < min，f64::clamp 会直接 panic
        if max_x > min_x {
            x = x.clamp(min_x, max_x);
        }
        if y + size.1 + 6.0 > origin.y + area.height {
            y = cursor.1 - size.1 - gap; // 下方放不下就翻到上方
        }
        if max_y > min_y {
            y = y.clamp(min_y, max_y);
        }

        log::debug!(
            "定位: 锚点=({:.0},{:.0}) 窗口={:.0}x{:.0} 屏原点=({:.0},{:.0}) 屏幕={:.0}x{:.0} → ({:.0},{:.0})",
            cursor.0, cursor.1, size.0, size.1,
            origin.x, origin.y, area.width, area.height, x, y
        );
    } else {
        log::debug!(
            "定位: 锚点=({:.0},{:.0}) 没查到所在显示器，跳过边界收拢",
            cursor.0, cursor.1
        );
    }

    (x.max(0.0), y.max(0.0))
}

/// 已存在就复用，不存在才创建。窗口创建有开销，划词是高频操作。
fn ensure(
    app: &AppHandle,
    label: &'static str,
    page: &str,
    title: &str,
    size: (f64, f64),
    configure: impl FnOnce(
        WebviewWindowBuilder<'_, tauri::Wry, AppHandle>,
    ) -> WebviewWindowBuilder<'_, tauri::Wry, AppHandle>,
) -> Result<tauri::WebviewWindow> {
    if let Some(existing) = app.get_webview_window(label) {
        return Ok(existing);
    }

    let builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App(page.into()))
        .title(title)
        .inner_size(size.0, size.1)
        .visible(false);

    let window = configure(builder)
        .build()
        .map_err(|e| anyhow!("创建窗口 {label} 失败: {e}"))?;

    // 关窗口只是收起来，不销毁。
    //
    // 这是常驻后台的工具：窗口一旦被销毁，下次划词又要重建 WebView（慢），
    // 而且 macOS 上销毁最后一个窗口容易把整个应用带走。
    let hidden = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = hidden.hide();
        }
    });

    Ok(window)
}

/// 划词后贴在光标下方的小图标条。
///
/// `anchor` 是手势发生那一刻的光标位置（逻辑点）。取词要花几百毫秒，
/// 等气泡弹出时鼠标可能已经移开，所以不能用当时的实时位置。
pub fn show_bubble(app: &AppHandle, payload: Payload, anchor: Option<(f64, f64)>) -> Result<()> {
    if let Some(at) = anchor {
        remember_anchor(at);
    }
    put_pending(BUBBLE, payload);

    let window = ensure(app, BUBBLE, "bubble.html", "SnapLingo", (BUBBLE_W, BUBBLE_H), |b| {
        b.decorations(false)
            .resizable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            // 透明 + 关闭系统阴影：圆角和阴影都由 CSS 画，
            // 否则系统会按矩形窗口投影，圆角外面露出一圈方角阴影
            .transparent(true)
            .shadow(false)
            .focused(false)
    })?;

    let scale = window.scale_factor().unwrap_or(1.0);
    let cursor = anchor_or_cursor(app, anchor, scale);
    let (x, y) = place_near(&window, cursor, (BUBBLE_W, BUBBLE_H), BUBBLE_GAP - BUBBLE_MARGIN);

    let _ = window.set_size(LogicalSize::new(BUBBLE_W, BUBBLE_H));
    let _ = window.set_position(LogicalPosition::new(x, y));

    // 已经开着的窗口靠这个信号刷新；刚建的窗口会在加载完后自己来取
    let _ = window.emit("bubble:pending", ());
    window.show()?;
    let _ = window.set_always_on_top(true);
    // 同上：show 之后再摆一次，防止被系统的默认摆放覆盖
    let _ = window.set_position(LogicalPosition::new(x, y));
    Ok(())
}

/// 一闪即逝的操作确认提示。
///
/// 收集这类操作做完之后界面上什么都不变，用户无从判断成没成——快捷键触发时
/// 更是全程静默。这个小窗贴着光标弹一下再自己消失，成本极低。
///
/// 不抢焦点、不吃鼠标事件：它只是个提示，不该打断用户手上的动作。
pub fn show_toast(app: &AppHandle, message: impl Into<String>) -> Result<()> {
    put_pending(
        TOAST,
        Payload { text: message.into(), source: "toast".into(), auto_translate: false },
    );

    let window = ensure(app, TOAST, "toast.html", "SnapLingo", (TOAST_W, TOAST_H), |b| {
        b.decorations(false)
            .resizable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .transparent(true)
            .shadow(false)
            .focused(false)
    })?;

    let _ = window.set_ignore_cursor_events(true);

    let scale = window.scale_factor().unwrap_or(1.0);
    let cursor = anchor_or_cursor(app, None, scale);
    // gap 减去 CSS padding（14px），让可见提示落在光标下方约 22px 处
    let (x, y) = place_near(&window, cursor, (TOAST_W, TOAST_H), 36.0 - 14.0);

    let _ = window.set_size(LogicalSize::new(TOAST_W, TOAST_H));
    let _ = window.set_position(LogicalPosition::new(x, y));
    let _ = window.emit("toast:pending", ());
    window.show()?;
    let _ = window.set_always_on_top(true);
    let _ = window.set_position(LogicalPosition::new(x, y));
    Ok(())
}

pub fn hide_bubble(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(BUBBLE) {
        let _ = window.hide();
    }
}

pub fn hide_window(app: &AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.hide();
    }
}

/// 翻译面板：贴着选中位置弹出的无边框小窗
pub fn show_result(app: &AppHandle, payload: Payload, anchor: Option<(f64, f64)>) -> Result<()> {
    hide_bubble(app);

    if let Some(at) = anchor {
        remember_anchor(at);
    }
    put_pending(RESULT, payload);

    let window = ensure(app, RESULT, "result.html", "SnapLingo", (RESULT_W, RESULT_H), |b| {
        b.decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .transparent(true)
            .shadow(false)
            .min_inner_size(320.0, 220.0)
    })?;

    let scale = window.scale_factor().unwrap_or(1.0);
    let cursor = anchor_or_cursor(app, anchor, scale);
    let (x, y) = place_near(&window, cursor, (RESULT_W, RESULT_H), RESULT_GAP - RESULT_MARGIN);

    let _ = window.set_size(LogicalSize::new(RESULT_W, RESULT_H));
    let _ = window.set_position(LogicalPosition::new(x, y));

    let _ = window.emit("result:pending", ());
    window.show()?;
    window.set_focus()?;
    let _ = window.set_always_on_top(true);

    // 再摆一次。macOS 上对还没显示过的窗口设置的位置，
    // 会在 show() 时被窗口管理器的默认摆放覆盖掉——这就是面板跑到屏幕
    // 顶部的原因。show 之后重设一次才稳。
    let _ = window.set_position(LogicalPosition::new(x, y));
    if let Ok(actual) = window.outer_position() {
        // 换算成逻辑坐标再记，否则和期望值不是一个量纲、根本没法对比
        log::debug!(
            "面板最终位置: 期望=({x:.0},{y:.0}) 实际=({:.0},{:.0}) 缩放={scale}",
            actual.x as f64 / scale,
            actual.y as f64 / scale
        );
    }
    Ok(())
}

pub fn show_collector(app: &AppHandle) -> Result<()> {
    let window = ensure(
        app,
        COLLECTOR,
        "collector.html",
        "SnapLingo · 收集夹",
        (720.0, 600.0),
        |b| b.min_inner_size(480.0, 320.0),
    )?;
    window.emit("collector:changed", ())?;
    window.show()?;
    window.set_focus()?;
    Ok(())
}

pub fn show_settings(app: &AppHandle) -> Result<()> {
    let window = ensure(
        app,
        SETTINGS,
        "settings.html",
        "SnapLingo · 设置",
        (640.0, 700.0),
        |b| b.min_inner_size(520.0, 420.0),
    )?;
    window.show()?;
    window.set_focus()?;
    Ok(())
}

/// 收集夹内容变了，通知已打开的窗口刷新
pub fn notify_collector_changed(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(COLLECTOR) {
        let _ = window.emit("collector:changed", ());
    }
}
