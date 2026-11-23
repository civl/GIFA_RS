'use strict';

/* 原神跟跑助手 —— 浮窗渲染层（M6 移植版）
 *
 * 这份是原版 `renderer/app.js` 的**桥接改写**：视觉、类名、交互手感照搬，
 * 但把 Electron 的 `window.api` / `<webview>` 换成本项目的 IPC：
 *
 *     页面 → Rust ：window.ipc.postMessage(JSON)
 *     Rust → 页面 ：window.gifaState(state)      （应用层状态）
 *                  window.gifaCalib(...)         （标定：测试/自动化用）
 *
 * 舞台视频是**另一个 WebView2**（外壳里的 float 窗），不在这张页面里；
 * 这张页面只负责"画框"：标题栏、控制条、进度条、缩放手柄。
 */

const $ = (id) => document.getElementById(id);

// 自检期把页面异常回报给 Rust：静默失败最难查
window.addEventListener('error', (e) => {
  try {
    window.ipc.postMessage(JSON.stringify({ t: 'debug', err: String(e.message), at: `${e.filename}:${e.lineno}` }));
  } catch (_) { /* ignore */ }
});
const dpr = () => window.devicePixelRatio || 1;

const state = {
  v: { title: '', part: 1, pages: [], paused: true, time: 0, dur: 0, rate: 1, muted: false, hasVideo: false },
  layouts: [],
  activeLayout: '',
  seeking: false,
  uiTimer: 0,
  toastTimer: 0,
  calibOpen: false,
  calib: { cx: 0, cy: 0, r: 80 },
};

function post(obj) {
  try {
    window.ipc.postMessage(JSON.stringify(obj));
  } catch (_) {
    /* 独立打开页面时没有 ipc */
  }
}

/* ------------------------------------------------------------ 尺寸上报 */

function rectPx(el) {
  const r = el.getBoundingClientRect();
  const k = dpr();
  return {
    x: Math.round(r.left * k),
    y: Math.round(r.top * k),
    w: Math.round(r.width * k),
    h: Math.round(r.height * k),
  };
}

/** 告诉外壳：标题栏/底栏多高、URL 条在哪、还有哪些浮层要"存在"。
 *  外壳据此算舞台矩形和 chrome 窗的区域（区域外的像素点击穿透）。 */
function reportLayout() {
  const urlbar = $('urlbar');
  const overlay = [rectPx($('grip'))];
  /* 快捷键面板是**浮在舞台上方**的：chrome 窗口的区域只包含上报过的矩形，
   * 不把它加进来，那一片会被裁掉 —— 表现就是"点开设置，面板被视频盖住"。 */
  const hk = $('hkPanel');
  if (hk && !hk.classList.contains('hidden')) overlay.push(rectPx(hk));
  const ul = $('urlList');
  if (ul && !ul.classList.contains('hidden')) overlay.push(rectPx(ul));
  if (state.calibOpen) overlay.push(rectPx($('calib')));
  const toast = $('toast');
  if (toast.classList.contains('show')) overlay.push(rectPx(toast));
  // 拖动区：标题栏减掉右侧工具区（按钮/下拉不参与拖动）
  const bar = $('titlebar').getBoundingClientRect();
  const toolsR = document.querySelector('#titlebar .tools');
  const dragW = toolsR
    ? Math.max(0, toolsR.getBoundingClientRect().left - bar.left)
    : bar.width;
  const k = dpr();
  post({
    t: 'layout',
    drag: {
      x: Math.round(bar.left * k),
      y: Math.round(bar.top * k),
      w: Math.round(dragW * k),
      h: Math.round(bar.height * k),
    },
    titlebar: Math.round($('titlebar').getBoundingClientRect().height * dpr()),
    footer: Math.round($('controls').getBoundingClientRect().height * dpr()),
    urlbar: urlbar.classList.contains('hidden') ? null : rectPx(urlbar),
    overlay,
  });
}

/* ------------------------------------------------------------ 状态应用 */

const fmt = (s) => {
  if (!Number.isFinite(s) || s < 0) s = 0;
  const m = Math.floor(s / 60);
  const r = Math.floor(s % 60);
  return `${String(m).padStart(2, '0')}:${String(r).padStart(2, '0')}`;
};

/* 快捷键面板的状态：**必须声明在 applyState 之前** ——
 * applyState 里会读它们，若声明在下面（TDZ），首次调用就抛错，
 * 后面 renderParts/renderLayouts 全被跳过（界面看起来像"控件没渲染"）。 */
let recording = null;
let lastHotkeys = [];
/* 历史 / 收藏：由 Rust 推过来（applyState），下拉只负责渲染 */
let historyList = [];
let favoritesList = [];
let urlListMode = null; // 'history' | 'favorites'

function fmtAgo(ms) {
  if (!ms) return '';
  const d = Math.max(0, Date.now() - ms);
  const m = Math.floor(d / 60000);
  if (m < 1) return '刚刚';
  if (m < 60) return m + ' 分钟前';
  const h = Math.floor(m / 60);
  if (h < 24) return h + ' 小时前';
  return Math.floor(h / 24) + ' 天前';
}

function renderUrlList() {
  const box = $('urlListBody');
  if (!box) return;
  const list = urlListMode === 'favorites' ? favoritesList : historyList;
  $('urlListTitle').textContent = urlListMode === 'favorites' ? '收藏' : '历史';
  $('urlListClear').classList.toggle('hidden', urlListMode === 'favorites');
  box.innerHTML = '';
  if (!list.length) {
    const empty = document.createElement('div');
    empty.className = 'ul-empty';
    empty.textContent = urlListMode === 'favorites'
      ? '还没有收藏。打开一个视频后点右边的 ☆'
      : '还没有历史记录';
    box.appendChild(empty);
    reportLayout();
    return;
  }
  for (const e of list) {
    const row = document.createElement('div');
    row.className = 'ul-row';
    const main = document.createElement('div');
    main.className = 'ul-main';
    const title = document.createElement('div');
    title.className = 'ul-title';
    title.textContent = e.title || e.text;
    const meta = document.createElement('div');
    meta.className = 'ul-meta';
    meta.textContent = [e.part > 1 ? 'P' + e.part : '', e.text, fmtAgo(e.at)]
      .filter(Boolean)
      .join(' · ');
    main.append(title, meta);
    const del = document.createElement('button');
    del.className = 'ul-del';
    del.textContent = urlListMode === 'favorites' ? '移除' : '删除';
    del.addEventListener('click', (ev) => {
      ev.stopPropagation();
      post(urlListMode === 'favorites'
        ? { t: 'favorite-remove', text: e.text, part: e.part }
        : { t: 'history-remove', text: e.text, part: e.part });
      if (urlListMode === 'favorites') favoritesList = favoritesList.filter((x) => !(x.text === e.text && x.part === e.part));
      else historyList = historyList.filter((x) => !(x.text === e.text && x.part === e.part));
      renderUrlList();
    });
    row.append(main, del);
    row.addEventListener('click', () => {
      closeUrlList();
      // 带上分 P：历史里存的是 bvid + part 两个字段，光发 text 会落回第 1 P
      post({ t: 'load', text: e.text, part: e.part || 1 });
    });
    box.appendChild(row);
  }
  reportLayout();
}

function openUrlList(mode) {
  urlListMode = mode;
  $('urlList').classList.remove('hidden');
  renderUrlList();
}

function closeUrlList() {
  urlListMode = null;
  $('urlList').classList.add('hidden');
  reportLayout();
}

/* ---------------- 快捷键面板 ----------------
 * 点「改」进入录制态：页面发 {t:'capture',on:true}，Rust 那边把所有热键都挡住，
 * 按键只用来捕获；捕获到就把 {t:'bind'} 发回去，Rust 用 set_binding 换绑（冲突自动交换）。 */
const HK_LABELS = {
  togglePlay: '播放/暂停',
  seekFwd: '快进 5 秒',
  seekBack: '后退 5 秒',
  nextPart: '下一分 P',
  prevPart: '上一分 P',
  holdSpeed: '按住加速',
  cycleSpeed: '循环倍速',
  toggleImmersion: '沉浸模式',
  togglePass: '鼠标穿透',
  toggleOpacity: '透明度',
  toggleVisible: '隐藏/显示',
};

function renderHotkeys(list) {
  const box = $('hkList');
  if (!box) return;
  box.innerHTML = '';
  (list || []).forEach(({ action, accel }) => {
    const row = document.createElement('div');
    row.className = 'hk-row' + (recording === action ? ' rec' : '');
    row.dataset.action = action;
    const name = document.createElement('span');
    name.className = 'hk-name';
    name.textContent = HK_LABELS[action] || action;
    const key = document.createElement('span');
    key.className = 'hk-key';
    key.textContent = recording === action ? '按下…' : accel || '—';
    const btn = document.createElement('button');
    btn.textContent = recording === action ? '取消' : '改';
    btn.addEventListener('click', () => {
      if (recording === action) { stopRecording(); return; }
      recording = action;
      post({ t: 'capture', on: true });
      renderHotkeys(lastHotkeys);
    });
    row.append(name, key, btn);
    box.appendChild(row);
  });
  // 面板高度随行数变化：区域要跟着重算，否则底部会被裁掉（看起来像"只显示了标题"）
  reportLayout();
}
function stopRecording() {
  recording = null;
  post({ t: 'capture', on: false });
  renderHotkeys(lastHotkeys);
}

// 把 KeyboardEvent 转成加速器字符串（Alt+J / F7 / Ctrl+Shift+K）
function accelOf(e) {
  const k = e.key;
  if (['Control', 'Alt', 'Shift', 'Meta'].includes(k)) return null; // 只按了修饰键
  let key = k.length === 1 ? k.toUpperCase() : k.replace('Arrow', '');
  if (key === ' ') key = 'Space';
  if (key === 'Escape') key = 'Esc';
  const mods = [];
  if (e.ctrlKey) mods.push('Ctrl');
  if (e.altKey) mods.push('Alt');
  if (e.shiftKey) mods.push('Shift');
  if (e.metaKey) mods.push('Super');
  return [...mods, key].join('+');
}

window.addEventListener('keydown', (e) => {
  if (!recording) return;
  e.preventDefault();
  e.stopPropagation();
  if (e.key === 'Escape') { stopRecording(); return; }
  const accel = accelOf(e);
  if (!accel) return;
  post({ t: 'bind', action: recording, accel });
  recording = null;
  post({ t: 'capture', on: false });
}, true);

$('btnSettings').addEventListener('click', () => {
  const p = $('hkPanel');
  p.classList.toggle('hidden');
  if (!p.classList.contains('hidden')) renderHotkeys(lastHotkeys);
  reportLayout(); // 区域要跟着变，否则面板落在挖空区外，看不见也点不到
});
$('hkClose').addEventListener('click', () => {
  stopRecording();
  $('hkPanel').classList.add('hidden');
  reportLayout();
});

/* ---------------- 历史 / 收藏 ---------------- */
/* 收藏的目标：**优先当前正在播放的视频**（星标的提示就是"收藏当前视频"）；
 * 没在播放时才退回输入框里那串。 */
function currentTarget() {
  const playing = (state.v.bvid || '').trim();
  const typed = ($('urlInput').value || '').trim();
  return {
    text: playing || typed,
    title: state.v.title || '',
    part: state.v.part || 1,
  };
}
function syncStar() {
  const t = currentTarget();
  if (!t.text) {
    $('btnStar').classList.remove('on');
    return;
  }
  const key = t.text.toLowerCase();
  const on = favoritesList.some(
    (e) => (e.text || '').toLowerCase() === key && (e.part || 1) === t.part,
  );
  $('btnStar').classList.toggle('on', on);
  $('btnStar').textContent = on ? '★' : '☆';
}

$('btnLink').addEventListener('click', () => {
  // 打开时把当前视频填进去，"收藏"才知道收藏的是哪一个
  const inp = $('urlInput');
  if (!inp.value && state.v.bvid) inp.value = state.v.bvid;
  syncStar();
});
$('btnHistory').addEventListener('click', () => {
  if (urlListMode === 'history') closeUrlList();
  else openUrlList('history');
});
$('btnFav').addEventListener('click', () => {
  if (urlListMode === 'favorites') closeUrlList();
  else openUrlList('favorites');
});
$('urlListClose').addEventListener('click', closeUrlList);
$('urlListClear').addEventListener('click', () => {
  post({ t: 'history-clear' });
  historyList = [];
  renderUrlList();
});
$('btnStar').addEventListener('click', () => {
  const t = currentTarget();
  if (!t.text) {
    toast('先载入一个视频');
    return;
  }
  post({ t: 'favorite-toggle', text: t.text, title: t.title, part: t.part });
});
$('urlInput').addEventListener('input', syncStar);

function applyState(s) {
  if (!s || typeof s !== 'object') return;
  const v = state.v;
  v.title = s.title || '';
  v.part = s.part || 1;
  v.paused = !!s.paused;
  v.time = s.time || 0;
  v.dur = s.dur || 0;
  v.rate = s.rate || 1;
  v.muted = !!s.muted;
  v.hasVideo = !!s.hasVideo;
  v.pages = Array.isArray(s.pages) ? s.pages : [];
  v.bvid = s.bvid || '';
  if (Array.isArray(s.history)) historyList = s.history;
  if (Array.isArray(s.favorites)) favoritesList = s.favorites;
  if (urlListMode) renderUrlList();
  syncStar();
  // 快捷键面板：收到新映射就重绘（录制中不覆盖"按下…"提示）
  if (Array.isArray(s.hotkeys)) {
    lastHotkeys = s.hotkeys;
    if (recording === null && !$('hkPanel').classList.contains('hidden')) {
      renderHotkeys(lastHotkeys);
    }
  }

  $('videoTitle').textContent = v.title;
  $('btnPlay').classList.toggle('playing', !v.paused);
  $('btnMute').classList.toggle('muted', v.muted);
  $('btnSpeed').textContent = `${Number(v.rate).toFixed(1).replace(/\.0$/, '.0')}×`;
  $('timeText').textContent = `${fmt(v.time)} / ${fmt(v.dur)}`;
  if (!state.seeking) paintProgress(v.time, v.dur);

  renderParts();
  renderLayouts(s);

  // 有视频才显示控制条；占位图在有视频时收起
  $('placeholder').classList.toggle('hidden', v.hasVideo);

  if (s.immersive) $('app').classList.add('video-only');
  else $('app').classList.remove('video-only');

  if (s.toast && s.toast !== state.lastToast) toast(s.toast, 3600);
  state.lastToast = s.toast;

  reportLayout();
}

/* ------------------------------------------------------------ 进度条 */

function paintProgress(t, d) {
  const pct = d > 0 ? Math.min(100, Math.max(0, (t / d) * 100)) : 0;
  $('bar').style.width = `${pct}%`;
  $('knob').style.left = `${pct}%`;
}

function wireProgress() {
  const el = $('progress');
  const ratio = (e) => {
    const r = el.getBoundingClientRect();
    return Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
  };
  const seekTo = (e) => {
    const d = state.v.dur;
    if (!d) return;
    const t = ratio(e) * d;
    paintProgress(t, d);
    $('timeText').textContent = `${fmt(t)} / ${fmt(d)}`;
    post({ t: 'seek', value: t });
  };

  el.addEventListener('pointerdown', (e) => {
    if (!state.v.dur) {
      toast('视频还没准备好');
      return;
    }
    state.seeking = true;
    el.classList.add('dragging');
    el.setPointerCapture(e.pointerId);
    seekTo(e);
  });
  el.addEventListener('pointermove', (e) => {
    if (!state.seeking) return;
    const d = state.v.dur;
    if (!d) return;
    const t = ratio(e) * d;
    paintProgress(t, d);
    $('timeText').textContent = `${fmt(t)} / ${fmt(d)}`;
  });
  el.addEventListener('pointerup', (e) => {
    if (!state.seeking) return;
    state.seeking = false;
    el.classList.remove('dragging');
    seekTo(e);
  });
}

/* ------------------------------------------------------------ 分 P / 布局 */

function renderParts() {
  const sel = $('partSelect');
  const pages = state.v.pages || [];
  if (pages.length <= 1) {
    sel.classList.add('hidden');
    return;
  }
  // p.page 是序号（数字），p.title 是标题文本 —— 别把标题当 value
  const key = pages.map((p) => `${p.page}:${p.title}`).join(',');
  if (sel.dataset.key !== key) {
    sel.dataset.key = key;
    sel.innerHTML = '';
    for (const p of pages) {
      const o = document.createElement('option');
      o.value = String(p.page);
      const label = p.title ? `P${p.page} · ${p.title}` : `P${p.page}`;
      o.textContent = p.duration ? `${label}（${fmt(p.duration)}）` : label;
      sel.appendChild(o);
    }
  }
  sel.value = String(state.v.part || 1);
  sel.classList.remove('hidden');
}

function renderLayouts(s) {
  const sel = $('layoutSelect');
  const list = s.layouts || state.layouts || [];
  const id = s.activeLayout || state.activeLayout || '';
  const key = list.map((l) => l.id).join(',');
  if (sel.dataset.key !== key) {
    sel.dataset.key = key;
    sel.innerHTML = '';
    for (const l of list) {
      const o = document.createElement('option');
      o.value = l.id;
      o.textContent = l.name;
      sel.appendChild(o);
    }
  }
  sel.value = id;
  sel.classList.toggle('hidden', list.length === 0);
}

/* ------------------------------------------------------------ toast */

function toast(text, ms = 2600) {
  const el = $('toast');
  el.textContent = text;
  el.classList.add('show');
  reportLayout();
  clearTimeout(state.toastTimer);
  state.toastTimer = setTimeout(() => {
    el.classList.remove('show');
    reportLayout();
  }, ms);
}

/* ------------------------------------------------------------ 自动隐藏 */

function wakeUi() {
  document.body.classList.add('ui');
  clearTimeout(state.uiTimer);
  state.uiTimer = setTimeout(() => {
    if (state.v.paused) return; // 播放中才收；暂停时留着方便操作
    if (state.seeking || state.calibOpen) return;
    if ($('urlbar').matches(':hover') || $('titlebar').matches(':hover')) return;
    document.body.classList.remove('ui');
  }, 2600);
}

/* ------------------------------------------------------------ 标定 */

function openCalib() {
  state.calibOpen = true;
  const stage = $('stage').getBoundingClientRect();
  // 先显示再量：display:none 时 getBoundingClientRect 是 0×0
  $('calib').classList.remove('hidden');
  state.calib.cx = stage.left + stage.width / 2;
  state.calib.cy = stage.top + stage.height / 2;
  state.calib.r = Math.max(40, Math.min(stage.width, stage.height) * 0.18);
  paintCalib();
  reportLayout();
}

function closeCalib(send) {
  if (!state.calibOpen) return;
  state.calibOpen = false;
  $('calib').classList.add('hidden');
  reportLayout();
  if (!send) return;
  const stage = $('stage').getBoundingClientRect();
  const k = dpr();
  // 舞台相对坐标（物理像素）
  post({
    t: 'calib-done',
    cx: Math.round(state.calib.cx * k),
    cy: Math.round((state.calib.cy - stage.top) * k),
    r: Math.round(state.calib.r * k),
  });
}

function paintCalib() {
  const box = $('calibBox');
  const c = state.calib;
  box.style.left = `${c.cx}px`;
  box.style.top = `${c.cy}px`;
  box.style.width = `${c.r * 2}px`;
  box.style.height = `${c.r * 2}px`;
  $('calibSize').textContent = `半径 ${Math.round(c.r)}px`;
}

/* ------------------------------------------------------------ 事件接线 */

function wireUi() {
  // 标题栏拖动：交给外壳走系统移动循环
  $('titlebar').addEventListener('mousedown', (e) => {
    if (e.button !== 0) return;
    if (e.target.closest('button,select,input')) return;
    post({ t: 'drag' });
  });

  // 右下角缩放手柄
  $('grip').addEventListener('mousedown', (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    post({ t: 'resize' });
  });

  $('btnPlay').addEventListener('click', () => post({ t: 'toggle' }));
  $('btnMute').addEventListener('click', () => post({ t: 'mute', on: !state.v.muted }));
  $('btnSpeed').addEventListener('click', () => {
    const steps = [1, 1.25, 1.5, 2, 0.75];
    const i = steps.findIndex((x) => Math.abs(x - state.v.rate) < 0.01);
    post({ t: 'rate', value: steps[(i + 1 + steps.length) % steps.length] });
  });

  $('btnLink').addEventListener('click', () => {
    const bar = $('urlbar');
    const show = bar.classList.contains('hidden');
    bar.classList.toggle('hidden', !show);
    reportLayout();
    if (show) $('urlInput').focus();
  });
  $('btnLoad').addEventListener('click', () => {
    const text = $('urlInput').value.trim();
    if (!text) return;
    post({ t: 'load', text });
    $('urlInput').value = '';
    $('urlbar').classList.add('hidden');
    reportLayout();
  });
  $('urlInput').addEventListener('keydown', (e) => {
    if (e.key === 'Enter') $('btnLoad').click();
    if (e.key === 'Escape') {
      $('urlbar').classList.add('hidden');
      reportLayout();
    }
    e.stopPropagation();
  });

  /* 输入态：在输入框里打字时，单键热键要让路（否则那个字符打不出来） */
  $('urlInput').addEventListener('focus', () => post({ t: 'typing', on: true }));
  $('urlInput').addEventListener('blur', () => post({ t: 'typing', on: false }));
  $('btnMin').addEventListener('click', () => post({ t: 'min' }));
  $('btnClose').addEventListener('click', () => post({ t: 'close' }));

  $('partSelect').addEventListener('change', (e) => {
    const p = Number(e.target.value);
    if (!Number.isFinite(p) || p < 1) return; // 防御：以前这里会发出 NaN
    post({ t: 'part', p });
  });
  $('layoutSelect').addEventListener('change', (e) => {
    post({ t: 'layout-select', id: e.target.value });
  });

  $('calibCancel').addEventListener('click', () => closeCalib(false));
  $('calibOk').addEventListener('click', () => closeCalib(true));
  $('calib').addEventListener('wheel', (e) => {
    if (!state.calibOpen) return;
    e.preventDefault();
    state.calib.r = Math.max(20, Math.min(400, state.calib.r + (e.deltaY > 0 ? 6 : -6)));
    paintCalib();
    reportLayout();
  }, { passive: false });

  document.addEventListener('mousedown', (e) => {
    post({ t: 'debug', at: 'page-mousedown', x: e.clientX, y: e.clientY, target: (e.target && e.target.id) || '' });
  }, true);
  document.addEventListener('mousemove', wakeUi);
  window.addEventListener('resize', reportLayout);

  // 标定：测试与自动化入口（没有真实鼠标时也能驱动）
  window.gifaCalib = (on, cx, cy, r) => {
    if (!on) {
      closeCalib(false);
      return;
    }
    if (!state.calibOpen) openCalib();
    if (typeof cx === 'number') state.calib.cx = cx;
    if (typeof cy === 'number') state.calib.cy = cy;
    if (typeof r === 'number') state.calib.r = r;
    paintCalib();
    reportLayout();
  };
  window.gifaCalibDone = () => closeCalib(true);

  // 自检用：读回界面几何并主动上报（Rust 侧只能收，不能返回值）
  window.gifaProbePost = () => post(Object.assign({ t: 'probe' }, window.gifaProbe()));

  // 自检用：读回界面几何
  window.gifaProbe = () => ({
    titlebar: Math.round($('titlebar').getBoundingClientRect().height),
    footer: Math.round($('controls').getBoundingClientRect().height),
    hasUi: document.body.classList.contains('ui'),
    playing: $('btnPlay').classList.contains('playing'),
    muted: $('btnMute').classList.contains('muted'),
    speed: $('btnSpeed').textContent,
    time: $('timeText').textContent,
    parts: $('partSelect').options.length,
    partHidden: $('partSelect').classList.contains('hidden'),
    layouts: $('layoutSelect').options.length,
    calib: state.calibOpen,
    urlbar: !$('urlbar').classList.contains('hidden'),
  });
}

/* ------------------------------------------------------------ 启动 */

window.gifaState = applyState;
// 自检/截图用：把最新热键表重绘一遍
window.__gifaRenderHotkeys = () => renderHotkeys(lastHotkeys);
window.__gifaHkCount = () => (lastHotkeys || []).length;
// 自检/截图用
window.__gifaOpenUrlList = (mode) => openUrlList(mode === 'favorites' ? 'favorites' : 'history');
window.__gifaReportLayout = () => reportLayout();

document.addEventListener('DOMContentLoaded', () => {
  // URL 条默认收起：标记里没写 hidden，必须在这里收，否则舞台一开始就被顶下去
  $('urlbar').classList.add('hidden');
  wireUi();
  wireProgress();
  wakeUi();
  reportLayout();
  // 页面字体加载完再报一次，避免用回退字体的宽度
  if (document.fonts && document.fonts.ready) document.fonts.ready.then(reportLayout);
});
