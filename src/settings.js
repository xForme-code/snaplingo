import { api, errorText , initTheme } from './api.js';

const el = (id) => document.getElementById(id);
const statusEl = el('status');

let config = null;
let meta = null;
let recording = null;

function setStatus(message, kind = 'info') {
  statusEl.textContent = message;
  statusEl.className = 'status' + (kind === 'error' ? ' error' : '');
}

/// 所有字段都是「改完立刻存」，不设保存按钮——设置项少，省一次点击
async function patch(fields) {
  try {
    config = await api.setConfig(fields);
    setStatus('已保存');
  } catch (err) {
    setStatus(errorText(err), 'error');
  }
}

function bindCheckbox(id, key) {
  const input = el(id);
  input.checked = Boolean(config[key]);
  input.addEventListener('change', () => patch({ [key]: input.checked }));
}

function bindText(id, key, transform = (v) => v) {
  const input = el(id);
  input.value = config[key] ?? '';
  // 边打字边存会很吵，改成失焦时存
  input.addEventListener('change', () => patch({ [key]: transform(input.value) }));
}

function bindSelect(id, key) {
  const select = el(id);
  select.value = config[key];
  select.addEventListener('change', () => patch({ [key]: select.value }));
}

// ---------------------------------------------------------------- 快捷键录制

/// 把 KeyboardEvent 转成 Tauri 认识的 accelerator 字符串
function toAccelerator(event) {
  const parts = [];
  if (event.metaKey) parts.push('Command');
  if (event.ctrlKey) parts.push('Control');
  if (event.altKey) parts.push('Alt');
  if (event.shiftKey) parts.push('Shift');

  const key = event.code;
  let main = null;
  if (key.startsWith('Key')) main = key.slice(3);
  else if (key.startsWith('Digit')) main = key.slice(5);
  else if (/^F\d{1,2}$/.test(key)) main = key;
  else if (key === 'Space') main = 'Space';

  if (!main || parts.length === 0) return null;
  parts.push(main);
  return parts.join('+');
}

function renderHotkeys() {
  for (const button of document.querySelectorAll('.hotkey')) {
    button.textContent = config.hotkeys[button.dataset.key] || '未设置';
  }
}

function stopRecording() {
  if (!recording) return;
  recording.classList.remove('recording');
  recording = null;
  renderHotkeys();
}

for (const button of document.querySelectorAll('.hotkey')) {
  button.addEventListener('click', () => {
    stopRecording();
    recording = button;
    button.classList.add('recording');
    button.textContent = '按下组合键…（Esc 取消）';
  });
}

document.addEventListener('keydown', async (event) => {
  if (!recording) return;
  event.preventDefault();

  if (event.key === 'Escape') return stopRecording();
  // 只按住修饰键时先不判定，等真正的主键
  if (['Meta', 'Control', 'Alt', 'Shift'].includes(event.key)) return;

  const accelerator = toAccelerator(event);
  if (!accelerator) {
    recording.textContent = '需要含修饰键的组合，请重按';
    return;
  }

  const key = recording.dataset.key;
  const hotkeys = { ...config.hotkeys, [key]: accelerator };
  stopRecording();
  await patch({ hotkeys });
  renderHotkeys();
});

// ---------------------------------------------------------------- 初始化

async function init() {
  [config, meta] = await Promise.all([api.getConfig(), api.getMeta()]);

  bindCheckbox('enabled', 'enabled');
  bindCheckbox('doubleClick', 'triggerOnDoubleClick');
  bindCheckbox('autostart', 'autostart');
  bindCheckbox('autoCheckUpdate', 'autoCheckUpdate');
  bindCheckbox('splitIdentifiers', 'splitIdentifiers');
  bindCheckbox('deeplPro', 'deeplPro');

  bindSelect('triggerMode', 'triggerMode');
  bindSelect('theme', 'theme');

  bindText('youdaoAppKey', 'youdaoAppKey');
  bindText('youdaoAppSecret', 'youdaoAppSecret');
  bindText('baiduAppId', 'baiduAppId');
  bindText('baiduSecret', 'baiduSecret');
  bindText('openaiApiKey', 'openaiApiKey');
  bindText('openaiBaseUrl', 'openaiBaseUrl');
  bindText('openaiModel', 'openaiModel');
  bindText('deeplApiKey', 'deeplApiKey');
  bindText('claudeApiKey', 'claudeApiKey');
  bindText('claudeModel', 'claudeModel');
  bindText('libreUrl', 'libreUrl');

  const threshold = el('dragThreshold');
  threshold.value = config.dragThreshold;
  threshold.addEventListener('change', () =>
    patch({ dragThreshold: Number(threshold.value) || 6 })
  );

  const ocrLanguages = el('ocrLanguages');
  ocrLanguages.value = (config.ocrLanguages ?? []).join(',');
  ocrLanguages.addEventListener('change', () => {
    const tags = ocrLanguages.value
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean);
    // 语言标签只能是字母/数字/连字符。Windows 的 OCR 把这个值拼进 PowerShell
    // 脚本，非法字符会跳出字符串上下文——后端也会再挡一次，这里是为了让用户
    // 当场看到问题，而不是被静默改成默认值。
    const bad = tags.filter((t) => !/^[A-Za-z0-9-]{1,35}$/.test(t));
    if (bad.length) {
      setStatus(`语言标签不合法：${bad.join(', ')}（只能用字母、数字和连字符）`, 'error');
      return;
    }
    patch({ ocrLanguages: tags });
  });

  const options = (list) =>
    list.map((l) => `<option value="${l.code}">${l.label}</option>`).join('');

  el('sourceLang').innerHTML = options(meta.sourceLanguages);
  // 老配置里可能没有这个字段，空值要落到 auto，否则下拉框会选不中任何项
  config.sourceLang = config.sourceLang || 'auto';
  bindSelect('sourceLang', 'sourceLang');

  el('targetLang').innerHTML = options(meta.languages);
  bindSelect('targetLang', 'targetLang');

  const provider = el('provider');
  provider.innerHTML = meta.providers
    .map((p) => `<option value="${p.id}">${p.label}${p.available ? '' : '（未配置）'}</option>`)
    .join('');
  provider.value = config.provider;

  const showNote = () => {
    const found = meta.providers.find((p) => p.id === provider.value);
    el('providerNote').textContent = found ? found.note : '';
  };
  provider.addEventListener('change', async () => {
    await patch({ provider: provider.value });
    showNote();
  });
  showNote();

  el('ocrEngine').textContent = meta.ocrEngine;
  el('configPath').textContent = meta.configPath;

  renderHotkeys();
  void refreshPackStatus();
  void renderModels();
}

// ---------------------------------------------------------------- 离线语言包

/// 语言包是按「语言对」下载的，所以源语言和目标语言任何一个变了都要重查
async function refreshPackStatus() {
  const label = el('packStatus');
  const button = el('downloadPack');
  const target = el('targetLang').value;

  label.textContent = '正在检查…';
  button.disabled = true;

  let status;
  try {
    status = await api.languagePackStatus(el('sourceLang').value, target);
  } catch (err) {
    label.textContent = `检查失败：${errorText(err)}`;
    return;
  }

  switch (status) {
    case 'installed':
      label.innerHTML = '<b>已下载</b>　断网时可用';
      button.textContent = '已下载';
      button.disabled = true;
      break;
    case 'needs-download':
      label.textContent = '未下载　断网时无法翻译';
      button.textContent = '下载语言包';
      button.disabled = false;
      break;
    case 'unsupported':
      label.textContent = '系统翻译不支持当前目标语言';
      button.disabled = true;
      break;
    default:
      label.textContent = '当前系统没有内置翻译（需要 macOS 15 或更新版本）';
      button.disabled = true;
  }
}

el('downloadPack').addEventListener('click', async (event) => {
  const button = event.currentTarget;
  button.disabled = true;
  button.textContent = '请在系统窗口中确认…';
  try {
    await api.downloadLanguagePack(el('sourceLang').value, el('targetLang').value);
    // 下过一次就不用再提示了
    await api.setConfig({ offlineHintDismissed: true });
  } catch (err) {
    setStatus(`下载失败：${errorText(err)}`, 'error');
  }
  await refreshPackStatus();
});

// 语言对变了，语言包状态也跟着变
el('targetLang').addEventListener('change', () => void refreshPackStatus());
el('sourceLang').addEventListener('change', () => void refreshPackStatus());

// ---------------------------------------------------------------- 离线模型

const MB = 1048576;
const mb = (bytes) => `${(bytes / MB).toFixed(0)} MB`;

async function renderModels() {
  let models;
  try {
    models = await api.localModelList();
  } catch (err) {
    el('modelList').textContent = `读取失败：${errorText(err)}`;
    return;
  }

  el('modelList').innerHTML = models
    .map(
      (m) => `
      <div class="model" data-id="${m.id}">
        <span class="model-name">${m.label}</span>
        <span class="model-size">${mb(m.bytes)}</span>
        <span class="model-state ${m.installed ? 'ok' : ''}">
          ${m.installed ? `已下载（占用 ${mb(m.diskBytes)}）` : '未下载'}
          <span class="bar" hidden><i></i></span>
        </span>
        <button data-act="${m.installed ? 'remove' : 'download'}" ${m.installed ? 'class="danger"' : ''}>
          ${m.installed ? '删除' : '下载'}
        </button>
      </div>`
    )
    .join('');
}

el('modelList').addEventListener('click', async (event) => {
  const button = event.target.closest('button[data-act]');
  if (!button) return;
  const row = button.closest('.model');
  const id = row.dataset.id;

  if (button.dataset.act === 'remove') {
    try {
      await api.localModelRemove(id);
      setStatus('已删除模型');
    } catch (err) {
      setStatus(`删除失败：${errorText(err)}`, 'error');
    }
    await renderModels();
    return;
  }

  button.disabled = true;
  button.textContent = '下载中';
  try {
    // 这个调用会一直等到下载并解压完成，进度靠 model:progress 事件推送
    await api.localModelDownload(id);
    setStatus('模型已就绪');
  } catch (err) {
    setStatus(`下载失败：${errorText(err)}`, 'error');
  }
  await renderModels();
});

/// 进度事件只更新对应那一行，不整表重绘——重绘会打断进度条动画
api.on('model:progress', (p) => {
  const row = document.querySelector(`.model[data-id="${p.id}"]`);
  if (!row) return;
  const state = row.querySelector('.model-state');
  const bar = row.querySelector('.bar');
  const fill = row.querySelector('.bar > i');

  if (p.phase === 'downloading') {
    const pct = p.total ? Math.round((p.received / p.total) * 100) : 0;
    state.className = 'model-state';
    state.firstChild.textContent = `下载中 ${pct}%（${mb(p.received)} / ${mb(p.total)}）`;
    bar.hidden = false;
    fill.style.width = `${pct}%`;
  } else if (p.phase === 'extracting') {
    state.firstChild.textContent = '正在解压…';
    fill.style.width = '100%';
  } else if (p.phase === 'failed') {
    state.className = 'model-state err';
    state.firstChild.textContent = `失败：${p.error ?? '未知原因'}`;
    bar.hidden = true;
  }
});

void init();

void initTheme();
