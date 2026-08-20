// 前端与 Rust 之间唯一的通信入口。
// 用 withGlobalTauri 直接拿全局对象，省掉一整套前端打包工具链。
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

export const api = {
  // 配置
  getConfig: () => invoke('get_config'),
  setConfig: (patch) => invoke('set_config', { patch }),
  getMeta: () => invoke('get_meta'),

  // 翻译 / 提取
  translate: (text, source, target, provider) =>
    invoke('translate_text', {
      text,
      source: source ?? null,
      target: target ?? null,
      provider: provider ?? null,
    }),

  // 取词 / OCR
  captureSelection: () => invoke('capture_selection'),
  runOcr: () => invoke('run_ocr'),

  // 系统翻译语言包（离线翻译的前提）
  downloadLanguagePack: (source, target) =>
    invoke('download_language_pack', { source, target }),
  languagePackStatus: (source, target) => invoke('language_pack_status', { source, target }),

  // 离线模型（OPUS-MT，按需下载）
  localModelList: () => invoke('local_model_list'),
  localModelDownload: (id) => invoke('local_model_download', { id }),
  localModelRemove: (id) => invoke('local_model_remove', { id }),

  // 剪贴板
  copy: (text) => invoke('copy_text', { text }),

  // 收集夹
  collectorList: () => invoke('collector_list'),
  collectorAdd: (text, source, translation, target) =>
    invoke('collector_add', {
      text,
      source,
      translation: translation ?? null,
      target: target ?? null,
    }),
  collectorRemove: (id) => invoke('collector_remove', { id }),
  collectorClear: () => invoke('collector_clear'),
  collectorMerged: (bilingual) => invoke('collector_merged', { bilingual }),
  collectorMarkdown: () => invoke('collector_markdown'),
  collectorExportItem: (id) => invoke('collector_export_item', { id }),
  collectorTranslateAll: (target) => invoke('collector_translate_all', { target: target ?? null }),

  // 窗口
  openWindow: (name) => invoke('open_window', { name }),
  showResult: (text, source, autoTranslate) =>
    invoke('show_result_window', { text, source, autoTranslate }),
  hideBubble: () => invoke('hide_bubble'),
  // 走 Rust 命令而不是前端的 window.hide()：后者需要额外的窗口权限，
  // 少配一条就静默失效（Esc 关不掉窗口就是这么来的）
  hideWindow: (label) => invoke('hide_window', { label }),
  closeSelf: () => invoke('hide_window', { label: getCurrentWindow().label }),

  /// 窗口刚建好时 emit 的数据会丢（那会儿还没注册监听器），
  /// 所以内容一律由前端加载完后主动来取
  takePending: (label) => invoke('take_pending', { label }),

  // 事件
  on: (event, handler) => listen(event, (e) => handler(e.payload)),
};

/// 应用主题。
///
/// 三态：system 跟随系统 / light 强制浅色 / dark 强制深色。
/// 实现方式是给 <html> 打 data-theme 属性，CSS 里三条规则各管一态——
/// 不需要在 JS 里搬运任何颜色值。
export function applyTheme(theme) {
  const root = document.documentElement;
  if (theme === 'light' || theme === 'dark') {
    root.dataset.theme = theme;
  } else {
    delete root.dataset.theme;
  }
}

/// 每个窗口加载时调一次：读配置应用主题，并订阅后续变更。
/// 订阅是必要的——设置窗口改了主题，其它已打开的窗口要立刻跟着变。
export async function initTheme() {
  try {
    const cfg = await api.getConfig();
    applyTheme(cfg.theme);
  } catch {
    /* 配置读不到就保持跟随系统 */
  }
  void api.on('config:changed', (cfg) => applyTheme(cfg?.theme));
}

/// 统一的错误文案：Rust 端返回的已经是给人看的中文，直接透传
export function errorText(err) {
  if (typeof err === 'string') return err;
  return err?.message ?? String(err);
}
