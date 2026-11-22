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
  const key = pages.map((p) => p.part).join(',');
  if (sel.dataset.key !== key) {
    sel.dataset.key = key;
    sel.innerHTML = '';
    for (const p of pages) {
      const o = document.createElement('option');
      o.value = String(p.part);
      // 当前 P 标出来：原版就是「P1 · 标题」这种写法
      const label = p.title ? `P${p.part} · ${p.title}` : `P${p.part}`;
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

  $('btnSettings').addEventListener('click', () => toast('设置面板在启动器里（M6 之后接）'));
  $('btnMin').addEventListener('click', () => post({ t: 'min' }));
  $('btnClose').addEventListener('click', () => post({ t: 'close' }));

  $('partSelect').addEventListener('change', (e) => {
    post({ t: 'part', p: Number(e.target.value) || 1 });
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
