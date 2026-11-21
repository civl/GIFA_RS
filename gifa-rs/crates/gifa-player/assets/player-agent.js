/* GIFA 播放器代理（页面内 agent）
 *
 * 职责：只在 B 站页面里做三件事 ——
 *   1. 找到 <video>，把播放状态推给 Rust；
 *   2. 接收 Rust 的指令（播放/暂停/跳转/倍速/音量/纯净/沉浸）；
 *   3. 把 HTML 全屏改写成"窗口全屏"请求，避免播放器盖住我们自己的标题栏。
 *
 * 之所以全部留在页面里：切 P、自动连播、播放器改版都由 B 站自己的脚本重建 <video>，
 * 只有页面内代码能跟着它走；Rust 侧不该去解析 B 站的 DOM。
 *
 * 该脚本由 Rust 以 initialization script 注入：**每次导航都会重新执行**，
 * 所以必须自带幂等守卫，并且只在自己的顶层文档里跑。
 */
(function () {
  'use strict';

  if (window.top !== window) return;          // 子框架不跑
  if (window.__gifa) return;                  // 防重复注入

  const send = (m) => {
    try {
      window.ipc.postMessage(JSON.stringify(m));
    } catch (e) {
      /* ipc 不可用时静默：宁可少报状态，也不要在页面里抛错 */
    }
  };

  /** 偏好：Rust 侧是唯一真相源，这里只做缓存，供新 <video> 应用 */
  const prefs = { volume: 100, muted: false, rate: 1 };

  let video = null;
  let pendingPlay = false;
  let cleanOn = false;
  let cleanStyle = null;
  let videoOnly = false;
  let videoOnlyStyle = null;
  let lastSig = '';
  let lastPushAt = 0;

  // ------------------------------------------------------------ 视频元素

  /** 页面里可能有多个 <video>（广告、预览、主播放器），取"最像主播放器"的那个 */
  function pickVideo() {
    const list = document.querySelectorAll('video');
    let best = null;
    for (const v of list) {
      if (!best) { best = v; continue; }
      const score = (x) => (x.readyState || 0) * 1e6 + (x.duration || 0) * 1e3 + (x.videoWidth || 0);
      if (score(v) > score(best)) best = v;
    }
    return best;
  }

  function applyPrefs(v) {
    if (!v) return;
    try {
      if (Number.isFinite(prefs.volume)) v.volume = Math.min(1, Math.max(0, prefs.volume / 100));
      v.muted = !!prefs.muted;
      if (Number.isFinite(prefs.rate) && prefs.rate > 0) {
        v.defaultPlaybackRate = prefs.rate;
        v.playbackRate = prefs.rate;
      }
    } catch (e) { /* 某些阶段设置会抛 InvalidStateError，下一帧会再来 */ }
  }

  function safePlay() {
    if (!video) { pendingPlay = true; return; }
    applyPrefs(video);
    const p = video.play();
    if (p && typeof p.catch === 'function') p.catch(() => {});
  }

  /** 从地址栏解析 BV 号与分 P（自动连播时 ?p= 会变，但 <video> 不一定重建） */
  function urlInfo() {
    const href = location.href;
    const m = href.match(/(BV[0-9A-Za-z]{10})/);
    const p = href.match(/[?&]p=(\d+)/);
    return {
      url: href,
      bvid: m ? m[1] : '',
      part: p ? Number(p[1]) : 1,
    };
  }

  function snapshot() {
    const info = urlInfo();
    const v = video && video.isConnected ? video : null;
    return {
      t: 'state',
      paused: v ? !!v.paused : true,
      ended: v ? !!v.ended : false,
      currentTime: v ? v.currentTime || 0 : 0,
      duration: v ? v.duration || 0 : 0,
      volume: v ? Math.round(v.volume * 100) : prefs.volume,
      muted: v ? !!v.muted : prefs.muted,
      rate: v ? v.playbackRate || 1 : prefs.rate,
      videoW: v ? v.videoWidth || 0 : 0,
      videoH: v ? v.videoHeight || 0 : 0,
      hasVideo: !!v,
      bvid: info.bvid,
      part: info.part,
      title: document.title || '',
      url: info.url,
    };
  }

  /** 状态推送：签名变了立刻推；否则最多 4Hz（进度条要动） */
  function push(force) {
    const s = snapshot();
    const now = Date.now();
    const sig = [
      s.paused, s.ended, Math.round(s.currentTime * 2), Math.round(s.duration),
      s.volume, s.muted, s.rate, s.hasVideo, s.bvid, s.part, s.title,
    ].join('|');
    if (!force && sig === lastSig && now - lastPushAt < 250) return;
    lastSig = sig;
    lastPushAt = now;
    send(s);
  }

  // ------------------------------------------------------------ 事件绑定

  const EVENTS = ['loadedmetadata', 'durationchange', 'play', 'playing', 'pause',
    'ended', 'ratechange', 'volumechange', 'seeked', 'timeupdate', 'resize'];

  function bind(v) {
    if (video === v) return;
    if (video) {
      for (const e of EVENTS) video.removeEventListener(e, onEvent);
    }
    video = v;
    if (video) {
      for (const e of EVENTS) video.addEventListener(e, onEvent, { passive: true });
      applyPrefs(video);
      if (pendingPlay) { pendingPlay = false; safePlay(); }
      push(true);
    } else {
      push(true);
    }
  }

  function onEvent() {
    push(false);
  }

  // ------------------------------------------------------------ 纯净 / 沉浸

  const CLEAN_CSS = `
    .bili-header, #biliMainHeader, .international-header,
    .right-container, #reco_list, .recommend-list-v1,
    .video-toolbar-container, .video-toolbar-left, .video-toolbar-right,
    #commentapp, .bili-comments, .comment-container,
    .video-sections-container, .bili-video-card, .pop-live-small-mode,
    .fixed-sidenav-storage-component, .ad-report { display: none !important; }
    .left-container { width: 100% !important; max-width: 100% !important; padding: 0 !important; margin: 0 !important; }
    html, body { background: #000 !important; }
  `;

  function setClean(on) {
    cleanOn = !!on;
    if (!/^www\.bilibili\.com$/.test(location.hostname)) return;
    if (on && !cleanStyle) {
      cleanStyle = document.createElement('style');
      cleanStyle.id = 'gifa-clean';
      cleanStyle.textContent = CLEAN_CSS;
      (document.head || document.documentElement).appendChild(cleanStyle);
      expandPlayer();
    } else if (!on && cleanStyle) {
      cleanStyle.remove();
      cleanStyle = null;
    }
  }

  /** 小窗里替用户点一下"网页全屏"，比一个个隐藏 DOM 元素可靠得多 */
  function expandPlayer() {
    let tries = 0;
    const timer = setInterval(() => {
      tries += 1;
      if (tries > 12 || !document.body) { clearInterval(timer); return; }
      const btn = document.querySelector('.bpx-player-ctrl-web');
      if (!btn) return;
      btn.click();
      clearInterval(timer);
      send({ t: 'log', msg: '已切到播放器网页全屏' });
    }, 1000);
  }

  function setVideoOnly(on) {
    videoOnly = !!on;
    for (const el of document.querySelectorAll('[data-gifa-video-path]')) {
      el.removeAttribute('data-gifa-video-path');
    }
    if (!on) {
      if (videoOnlyStyle) { videoOnlyStyle.remove(); videoOnlyStyle = null; }
      return;
    }
    if (!videoOnlyStyle && document.documentElement) {
      videoOnlyStyle = document.createElement('style');
      videoOnlyStyle.textContent = `
        html, body { background: #000 !important; overflow: hidden !important; }
        /* 隐藏滚动条：否则播放器会给右侧留出十几像素，沉浸里看得到黑边 */
        ::-webkit-scrollbar { width: 0 !important; height: 0 !important; display: none !important; }
        body * { visibility: hidden !important; }
        [data-gifa-video-path] { transform: none !important; contain: none !important; overflow: visible !important; }
        video[data-gifa-video] { visibility: visible !important; position: fixed !important; inset: 0 !important;
          width: 100vw !important; height: 100vh !important; max-width: none !important; max-height: none !important;
          margin: 0 !important; object-fit: contain !important; z-index: 2147483647 !important; }
      `;
      (document.head || document.documentElement).appendChild(videoOnlyStyle);
    }
    if (video) {
      video.setAttribute('data-gifa-video', '');
      for (let p = video.parentElement; p; p = p.parentElement) {
        p.setAttribute('data-gifa-video-path', '');
      }
    }
    push(true);
  }

  // ------------------------------------------------------------ 全屏拦截

  // B 站的 HTML 全屏会铺满整个客户区、盖住我们自己的标题栏。
  // 改成让 Rust 走窗口全屏 —— 这样标题栏/控件条还在，用户不会"找不到设置"。
  (function interceptFullscreen() {
    const original = Element.prototype.requestFullscreen;
    if (!original) return;
    Element.prototype.requestFullscreen = function () {
      send({ t: 'fullscreen' });
      return Promise.resolve();
    };
  })();

  // ------------------------------------------------------------ B 站数据（同源 fetch）

  /** 取视频信息：分 P 列表、标题、UP。
   *  走 api.bilibili.com 而不是站内相对路径 —— 页面自己也是这么调的，
   *  B 站给 www 源放行了 CORS，`credentials: 'include'` 会带上登录 Cookie。 */
  async function fetchView(m) {
    const id = String(m.id || '').trim();
    if (!id) return send({ t: 'view', ok: false, error: 'no-id' });
    const q = id.startsWith('av') ? 'aid=' + id.slice(2) : 'bvid=' + id;
    try {
      const r = await fetch('https://api.bilibili.com/x/web-interface/view?' + q, {
        credentials: 'include',
        headers: { Accept: 'application/json' },
      });
      const j = await r.json();
      if (j.code !== 0 || !j.data) {
        return send({ t: 'view', ok: false, error: j.message || ('code ' + j.code) });
      }
      const d = j.data;
      send({
        t: 'view',
        ok: true,
        info: {
          bvid: d.bvid || id,
          title: d.title || '',
          owner: (d.owner && d.owner.name) || '',
          pages: (d.pages || []).map((p) => ({
            page: p.page,
            part: p.part || '',
            duration: p.duration || 0,
          })),
        },
      });
    } catch (e) {
      send({ t: 'view', ok: false, error: String((e && e.message) || e) });
    }
  }

  /** 取登录态。未登录时接口返回 code=-101，但 data.isLogin 仍是 false，按 ok 处理。 */
  async function fetchNav() {
    try {
      const r = await fetch('https://api.bilibili.com/x/web-interface/nav', {
        credentials: 'include',
        headers: { Accept: 'application/json' },
      });
      const j = await r.json();
      const d = j.data || {};
      send({
        t: 'nav',
        ok: true,
        login: {
          loggedIn: !!d.isLogin,
          uname: d.uname || '',
          mid: d.mid || 0,
          vip: d.vipStatus === 1,
          level: d.level_info ? d.level_info.current_level || 0 : 0,
        },
      });
    } catch (e) {
      send({ t: 'nav', ok: false, error: String((e && e.message) || e) });
    }
  }

  // ------------------------------------------------------------ 指令入口

  function clamp(n, lo, hi) {
    return Math.min(hi, Math.max(lo, n));
  }

  const handlers = {
    toggle() {
      if (!video) { bind(pickVideo()); if (!video) { pendingPlay = true; return; } }
      if (video.paused || video.ended) safePlay(); else video.pause();
    },
    play() { pendingPlay = true; safePlay(); },
    pause() { pendingPlay = false; if (video) video.pause(); },
    seek(m) {
      if (!video) return;
      video.currentTime = clamp(Number(m.value) || 0, 0, video.duration || 1e9);
    },
    seekBy(m) {
      if (!video) return;
      const t = (video.currentTime || 0) + (Number(m.value) || 0);
      video.currentTime = clamp(t, 0, video.duration || 1e9);
    },
    rate(m) {
      prefs.rate = clamp(Number(m.value) || 1, 0.25, 3);
      if (video) { video.defaultPlaybackRate = prefs.rate; video.playbackRate = prefs.rate; }
      push(true);
    },
    volume(m) {
      prefs.volume = clamp(Number(m.value) || 0, 0, 100);
      prefs.muted = prefs.volume === 0;
      if (video) { video.volume = prefs.volume / 100; video.muted = prefs.muted; }
      push(true);
    },
    mute(m) {
      prefs.muted = !!m.value;
      if (video) video.muted = prefs.muted;
      push(true);
    },
    preferences(m) {
      prefs.volume = clamp(Number(m.volume) || 0, 0, 100);
      prefs.muted = !!m.muted;
      prefs.rate = clamp(Number(m.rate) || 1, 0.25, 3);
      applyPrefs(video);
      push(true);
    },
    clean(m) { setClean(!!m.on); },
    'fetch-view'(m) { fetchView(m); },
    'fetch-nav'() { fetchNav(); },
    presentation(m) { setVideoOnly(!!m.videoOnly); },
    ping() {
      bind(pickVideo());
      push(true);
    },
  };

  window.__gifa = {
    dispatch(msg) {
      if (!msg || typeof msg !== 'object') return;
      const fn = handlers[msg.cmd];
      if (!fn) { send({ t: 'log', msg: '未知指令 ' + msg.cmd }); return; }
      try { fn(msg); } catch (e) { send({ t: 'log', msg: '指令失败 ' + msg.cmd + ': ' + e.message }); }
    },
    /** 调试用：把当前状态立刻取出来 */
    state: snapshot,
  };

  // ------------------------------------------------------------ 启动

  function boot() {
    bind(pickVideo());
    send({ t: 'ready', url: location.href, title: document.title || '' });
    // 页面一就绪就报一次登录态，界面不用等用户点
    fetchNav();

    // B 站是 SPA：切 P / 换片会重建 <video>，也可能只改地址栏
    setInterval(() => {
      const v = pickVideo();
      if (v && v !== video) bind(v);
      else if (!v && video && !video.isConnected) bind(null);
    }, 700);

    let lastHref = location.href;
    setInterval(() => {
      if (location.href === lastHref) return;
      lastHref = location.href;
      if (cleanOn) setClean(true);       // 换片后样式节点还在，但重新点一次网页全屏更稳
      push(true);
    }, 500);

    // 兜底：没有播放事件时也保持状态新鲜
    setInterval(() => push(false), 1000);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot, { once: true });
  } else {
    boot();
  }
})();
