use once_cell::sync::Lazy;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 划词监听可以被临时挂起（截图翻译期间、我们自己的窗口获得焦点时）
static SUSPENDED: AtomicBool = AtomicBool::new(false);

pub fn suspend(value: bool) {
    SUSPENDED.store(value, Ordering::Relaxed);
}

pub fn is_suspended() -> bool {
    SUSPENDED.load(Ordering::Relaxed)
}

/// 一次划选动作，带上抬起时的光标位置（逻辑点，来自系统查询）。
///
/// 必须记下手势发生那一刻的坐标：取词要花几百毫秒，等到气泡真正弹出时
/// 鼠标往往已经移走了，用那时的位置会把气泡放到毫不相干的地方。
#[derive(Debug, Clone, Copy)]
pub struct SelectionGesture {
    pub x: f64,
    pub y: f64,
}

/// 发送端常驻，接收端只取一次（交给消费线程），所以用 Option 包着。
type GestureChannel = (
    Mutex<Sender<SelectionGesture>>,
    Mutex<Option<Receiver<SelectionGesture>>>,
);

static CHANNEL: Lazy<GestureChannel> =
    Lazy::new(|| {
        let (tx, rx) = mpsc::channel();
        (Mutex::new(tx), Mutex::new(Some(rx)))
    });

/// 取出接收端（只能取一次），交给消费线程
pub fn take_receiver() -> Option<Receiver<SelectionGesture>> {
    CHANNEL.1.lock().ok()?.take()
}

/// 直接向系统查询光标位置。
///
/// 不能依赖 rdev 的 MouseMove 来跟踪光标：macOS 在按住左键拖动时发出的是
/// LeftMouseDragged，rdev 不会把它转成 MouseMove，导致整个拖动过程中
/// 我们看到的坐标一直停在按下的那一点，拖动距离恒等于 0。
///
/// Enigo 实例的构造有开销，用 thread_local 复用——回调始终跑在同一个线程上。
fn query_cursor() -> Option<(f64, f64)> {
    use enigo::{Enigo, Mouse, Settings};
    use std::cell::RefCell;

    thread_local! {
        static ENIGO: RefCell<Option<Enigo>> = const { RefCell::new(None) };
    }

    ENIGO.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Enigo::new(&Settings::default()).ok();
        }
        let enigo = slot.as_ref()?;
        let (x, y) = enigo.location().ok()?;
        Some((x as f64, y as f64))
    })
}

#[derive(Default)]
struct GestureState {
    cursor: (f64, f64),
    press_at: Option<(f64, f64)>,
    press_time: Option<Instant>,
    last_release: Option<(Instant, (f64, f64))>,
    /// 按下之后收到了多少个移动事件。用来判断拖拽期间系统是否真的在推送位移。
    moves_since_press: u32,
}

/// 启动全局鼠标监听。
///
/// 失败不是致命错误：拿不到权限时快捷键触发仍然可用，
/// 所以这里把错误返回给调用方去提示，而不是 panic。
pub fn start_mouse_watcher() -> Result<(), String> {
    use rdev::{listen, Button, EventType};

    let sender = CHANNEL
        .0
        .lock()
        .map_err(|_| "内部通道已损坏".to_string())?
        .clone();

    // rdev 的回调是阻塞的，任何耗时操作都必须挪出去，
    // 否则会拖慢整个系统的鼠标事件派发。
    std::thread::Builder::new()
        .name("snaplingo-mouse-hook".into())
        .spawn(move || {
            let mut state = GestureState::default();

            log::info!("鼠标监听线程已进入 rdev::listen（该调用会一直阻塞）");

            // 只在收到第一个事件时记一次：用来区分「钩子建起来了但收不到事件」
            // 和「钩子根本没工作」。每个事件都记会把日志刷爆。
            let mut seen_first_event = false;

            let result = listen(move |event| {
                if !seen_first_event {
                    seen_first_event = true;
                    log::info!("鼠标钩子已收到第一个事件: {:?}", event.event_type);
                }

                match event.event_type {
                    EventType::MouseMove { x, y } => {
                        state.cursor = (x, y);
                        state.moves_since_press = state.moves_since_press.saturating_add(1);
                    }
                    EventType::ButtonPress(Button::Left) => {
                        // 以系统查询到的位置为准，rdev 的缓存坐标在拖动期间不更新
                        let at = query_cursor().unwrap_or(state.cursor);
                        state.press_at = Some(at);
                        state.press_time = Some(Instant::now());
                        state.moves_since_press = 0;
                        log::debug!("鼠标按下 @({:.0},{:.0})", at.0, at.1);
                    }
                    EventType::ButtonRelease(Button::Left) => {
                        let release_at = query_cursor().unwrap_or(state.cursor);
                        let now = Instant::now();

                        let cfg = crate::config::get();
                        let press_at = state.press_at.take();
                        state.press_time.take();
                        let moves = std::mem::take(&mut state.moves_since_press);

                        let previous_release = state.last_release.replace((now, release_at));

                        if !cfg.enabled
                            || is_suspended()
                            || cfg.trigger_mode == crate::config::TriggerMode::Hotkey
                        {
                            log::debug!(
                                "鼠标抬起被忽略（enabled={}, suspended={}, mode={:?}）",
                                cfg.enabled,
                                is_suspended(),
                                cfg.trigger_mode
                            );
                            return;
                        }

                        let distance = press_at
                            .map(|(px, py)| {
                                let dx = release_at.0 - px;
                                let dy = release_at.1 - py;
                                (dx * dx + dy * dy).sqrt()
                            })
                            .unwrap_or(0.0);
                        let dragged = distance >= cfg.drag_threshold;

                        // rdev 不提供点击次数，用「两次抬起间隔短 + 位移小」判定双击选词
                        let double_clicked = cfg.trigger_on_double_click
                            && previous_release
                                .map(|(prev_time, prev_pos)| {
                                    let dx = release_at.0 - prev_pos.0;
                                    let dy = release_at.1 - prev_pos.1;
                                    now.duration_since(prev_time) < Duration::from_millis(400)
                                        && (dx * dx + dy * dy).sqrt() < 6.0
                                })
                                .unwrap_or(false);

                        log::debug!(
                            "鼠标抬起 @({:.0},{:.0}) 按下点={:?} 期间移动事件={} 距离={:.1} 阈值={} 判定: 拖选={} 双击={}",
                            release_at.0,
                            release_at.1,
                            press_at.map(|(x, y)| format!("({x:.0},{y:.0})")),
                            moves,
                            distance,
                            cfg.drag_threshold,
                            dragged,
                            double_clicked
                        );

                        if dragged || double_clicked {
                            match sender.send(SelectionGesture {
                                x: release_at.0,
                                y: release_at.1,
                            }) {
                                Ok(()) => log::debug!("已投递划选事件"),
                                Err(err) => log::error!("投递划选事件失败: {err}"),
                            }
                        }
                    }
                    _ => {}
                }
            });

            // listen 正常情况下永不返回。一旦返回就说明事件钩子挂了，
            // 最常见的原因是 macOS 没有授予辅助功能权限（CGEventTap 创建失败）。
            match result {
                Ok(()) => log::error!("全局鼠标监听意外结束（listen 正常返回）"),
                Err(err) => log::error!(
                    "全局鼠标监听失败: {err:?} —— macOS 上通常是缺少「辅助功能」权限"
                ),
            }
        })
        .map_err(|e| format!("无法启动监听线程: {e}"))?;

    Ok(())
}
