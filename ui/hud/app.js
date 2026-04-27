const { invoke, Channel } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow, LogicalPosition } = window.__TAURI__.window;

const el = {
  time: document.getElementById('time'),
  date: document.getElementById('date'),
  down: document.getElementById('down'),
  up: document.getElementById('up'),
  mediaTitle: document.getElementById('media-title'),
  mediaArtist: document.getElementById('media-artist'),
  mediaPlay: document.getElementById('media-play'),
  mediaNext: document.getElementById('media-next'),
  mediaPrev: document.getElementById('media-prev'),
  volSlider: document.getElementById('vol-slider'),
  volPct: document.getElementById('vol-pct'),
  volIcon: document.getElementById('vol-icon'),
  appsList: document.getElementById('apps-list'),
  appsLabel: document.getElementById('apps-label'),
  appsToggle: document.getElementById('apps-toggle'),
  appsCaret: document.getElementById('apps-caret'),
  netDot: document.getElementById('net-dot'),
  netText: document.getElementById('net-text'),
  netPing: document.getElementById('net-ping'),
  toast: document.getElementById('toast'),
  cpuVal: document.getElementById('cpu-val'),
  cpuBar: document.getElementById('cpu-bar'),
  ramVal: document.getElementById('ram-val'),
  ramBar: document.getElementById('ram-bar'),
  wxIcon: document.getElementById('wx-icon'),
  wxTemp: document.getElementById('wx-temp'),
  wxCond: document.getElementById('wx-cond'),
};

let prevOnline = null;
let toastTimer = null;
function showToast(text, kind) {
  el.toast.textContent = text;
  el.toast.className = `toast show ${kind || ''}`;
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.toast.className = 'toast'; }, 3500);
}

let lastSnapshot = null;
let runningApps = [];

function fmtRate(bps) {
  if (bps < 1024) return `${bps.toFixed(0)} B/s`;
  if (bps < 1024 * 1024) return `${(bps / 1024).toFixed(1)} KB/s`;
  return `${(bps / 1024 / 1024).toFixed(2)} MB/s`;
}

function volIconFor(pct, muted) {
  if (muted) return '🔇';
  if (pct === 0) return '🔈';
  if (pct < 50) return '🔉';
  return '🔊';
}

// WMO weather codes → emoji glyph. Falls back to a neutral dot.
function wxGlyph(code) {
  if (code === 0) return '☀';
  if (code === 1 || code === 2) return '🌤';
  if (code === 3) return '☁';
  if (code === 45 || code === 48) return '🌫';
  if (code >= 51 && code <= 57) return '🌦';
  if ((code >= 61 && code <= 67) || (code >= 80 && code <= 82)) return '🌧';
  if ((code >= 71 && code <= 77) || code === 85 || code === 86) return '❄';
  if (code >= 95 && code <= 99) return '⛈';
  return '·';
}

function setBarLevel(barEl, pct) {
  barEl.style.width = `${Math.max(0, Math.min(100, pct))}%`;
  barEl.classList.toggle('warn', pct >= 70 && pct < 90);
  barEl.classList.toggle('crit', pct >= 90);
}

function render() {
  const now = new Date();
  el.time.textContent = `${String(now.getHours()).padStart(2,'0')}:${String(now.getMinutes()).padStart(2,'0')}`;
  el.date.textContent = now.toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' });

  if (!lastSnapshot) return;

  // Network
  el.down.textContent = fmtRate(lastSnapshot.network.down_bps);
  el.up.textContent = fmtRate(lastSnapshot.network.up_bps);
  const inet = lastSnapshot.internet;
  if (inet) {
    el.netDot.className = 'status-dot ' + (inet.online ? 'on' : 'off');
    el.netText.textContent = inet.online ? 'Online' : 'Offline';
    el.netPing.textContent = inet.online && inet.ping_ms != null
      ? `· ${inet.ping_ms} ms`
      : '';
    if (prevOnline !== null && prevOnline !== inet.online) {
      showToast(inet.online ? 'Internet reconnected' : 'Internet disconnected', inet.online ? 'good' : 'bad');
    }
    prevOnline = inet.online;
  }

  // Media
  const m = lastSnapshot.media;
  if (m.has_session && m.title) {
    el.mediaTitle.textContent = m.title;
    el.mediaArtist.textContent = m.artist || '';
    el.mediaPlay.textContent = m.playing ? '⏸' : '▶';
  } else {
    el.mediaTitle.textContent = 'Nothing playing';
    el.mediaArtist.textContent = '';
    el.mediaPlay.textContent = '▶';
  }

  // Audio
  const a = lastSnapshot.audio;
  if (a && a.has_device) {
    if (document.activeElement !== el.volSlider) {
      el.volSlider.value = a.volume_percent;
    }
    el.volPct.textContent = `${a.volume_percent}%`;
    el.volIcon.textContent = volIconFor(a.volume_percent, a.muted);
  } else {
    el.volPct.textContent = '--';
    el.volIcon.textContent = '🔈';
  }

  // Sysstats
  const ss = lastSnapshot.sysstats;
  if (ss) {
    el.cpuVal.textContent = `${ss.cpu_percent}%`;
    setBarLevel(el.cpuBar, ss.cpu_percent);
    el.ramVal.textContent = `${ss.mem_percent}%`;
    setBarLevel(el.ramBar, ss.mem_percent);
  }

  // Weather
  const w = lastSnapshot.weather;
  if (w) {
    if (w.temp_c != null) {
      el.wxTemp.textContent = `${Math.round(w.temp_c)}°`;
      el.wxIcon.textContent = wxGlyph(w.code ?? -1);
      el.wxCond.textContent = w.condition || '';
    } else {
      el.wxTemp.textContent = '--°';
      el.wxCond.textContent = 'Loading…';
    }
  }
}

const SYSTEM_EXES = new Set([
  'applicationframehost', 'sihost', 'startmenuexperiencehost',
  'searchhost', 'shellexperiencehost', 'textinputhost', 'systemsettings',
  'lockapp', 'usercpl', 'fontdrvhost', 'dwm', 'csrss', 'wininit',
  'services', 'lsass', 'svchost', 'taskhostw', 'runtimebroker',
  'ctfmon', 'conhost', 'dllhost', 'wmiprvse',
]);
function isSystemApp(app) {
  return SYSTEM_EXES.has(nameOf(app).toLowerCase());
}
function nameOf(app) {
  return app.exe_path.split('\\').pop().replace(/\.exe$/i, '');
}

function renderApps() {
  const visible = runningApps.filter(a => !isSystemApp(a));
  el.appsLabel.textContent = `Background apps · ${visible.length}`;
  el.appsList.innerHTML = '';
  const sorted = [...visible].sort((x, y) =>
    nameOf(x).localeCompare(nameOf(y), undefined, { sensitivity: 'base' })
  );
  for (const app of sorted) {
    const item = document.createElement('div');
    item.className = 'apps-item';
    const nameSpan = document.createElement('span');
    nameSpan.className = 'name';
    nameSpan.textContent = nameOf(app);
    const countSpan = document.createElement('span');
    countSpan.className = 'count';
    countSpan.textContent = app.windows.length > 1 ? `×${app.windows.length}` : '';
    const closeBtn = document.createElement('button');
    closeBtn.className = 'app-close';
    closeBtn.title = `Close ${nameOf(app)}`;
    closeBtn.textContent = '×';
    closeBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      const hwnds = app.windows.map(w => w.hwnd);
      invoke('close_hwnds', { hwnds }).catch(() => {});
    });
    item.appendChild(nameSpan);
    item.appendChild(countSpan);
    item.appendChild(closeBtn);
    item.addEventListener('click', () => {
      if (app.windows.length > 0) {
        invoke('focus_window', { hwnd: app.windows[0].hwnd });
      }
    });
    el.appsList.appendChild(item);
  }
}

// Apps section collapse toggle
el.appsToggle.addEventListener('click', () => {
  const expanded = el.appsToggle.getAttribute('aria-expanded') === 'true';
  el.appsToggle.setAttribute('aria-expanded', String(!expanded));
  el.appsList.classList.toggle('collapsed', expanded);
});

// ─────────────────────────────────────────────────────────────────────────
// Files stash — drop files in, drag them back out anywhere
// ─────────────────────────────────────────────────────────────────────────
const stashToggle = document.getElementById('stash-toggle');
const stashBody = document.getElementById('stash-body');
const stashBlock = document.getElementById('stash-block');
const stashDropzone = document.getElementById('stash-dropzone');
const stashList = document.getElementById('stash-list');
const stashLabel = document.getElementById('stash-label');

stashToggle.addEventListener('click', () => {
  const expanded = stashToggle.getAttribute('aria-expanded') === 'true';
  stashToggle.setAttribute('aria-expanded', String(!expanded));
  stashBlock.setAttribute('aria-expanded', String(!expanded));
});

// 1×1 transparent PNG used as the drag image — the OS only needs *something*
// to follow the cursor; the real preview is the drop target's responsibility.
const TRANSPARENT_PNG_B64 =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=';

function renderStash(entries) {
  const list = Array.isArray(entries) ? entries : [];
  stashLabel.textContent = list.length ? `Files · ${list.length}` : 'Files';
  stashList.innerHTML = '';
  for (const e of list) {
    const row = document.createElement('div');
    row.className = 'stash-item';
    row.title = e.path;

    const name = document.createElement('span');
    name.className = 'stash-name';
    name.textContent = e.name;

    const rm = document.createElement('button');
    rm.className = 'stash-remove';
    rm.textContent = '×';
    rm.title = 'Remove from stash';
    rm.addEventListener('click', (ev) => {
      ev.stopPropagation();
      invoke('stash_remove', { path: e.path }).catch(() => {});
    });

    attachNativeDragOut(row, [e.path]);

    row.appendChild(name);
    row.appendChild(rm);
    stashList.appendChild(row);
  }
}

// Wire mousedown-driven OS drag. The browser's HTML5 dragstart event
// fights with `tauri-plugin-drag`'s DoDragDrop call (both want to own the
// cursor), so we trigger the native drag manually after a small movement
// threshold — that lets normal clicks still fire on the row.
function attachNativeDragOut(el, paths) {
  let down = null;
  el.addEventListener('mousedown', (ev) => {
    if (ev.button !== 0) return;
    down = { x: ev.screenX, y: ev.screenY };
  });
  el.addEventListener('mousemove', (ev) => {
    if (!down) return;
    const dx = Math.abs(ev.screenX - down.x);
    const dy = Math.abs(ev.screenY - down.y);
    if (dx > 5 || dy > 5) {
      const target = down;
      down = null;
      const onEvent = new Channel();
      invoke('plugin:drag|start_drag', {
        item: paths,
        image: TRANSPARENT_PNG_B64,
        onEvent,
      }).catch((err) => console.error('start_drag failed', err));
      // Mark unused so target isn't a dead binding warning if something
      // else reads it later for debugging.
      void target;
    }
  });
  el.addEventListener('mouseup',   () => { down = null; });
  el.addEventListener('mouseleave',() => { down = null; });
  // Cancel browser's default text/element drag preview entirely.
  el.addEventListener('dragstart', (ev) => ev.preventDefault());
}

// Tauri's window-scoped drag-drop event delivers OS file paths. We accept
// only drops whose cursor position fell inside the dropzone's bounding box,
// so the user can still drag elsewhere on the HUD without it stealing files.
let stashOver = false;
function pointInDropzone(p) {
  if (!p) return false;
  const r = stashDropzone.getBoundingClientRect();
  // Tauri payload position is in physical pixels; convert with dpr.
  const dpr = window.devicePixelRatio || 1;
  const x = p.x / dpr;
  const y = p.y / dpr;
  return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
}
getCurrentWindow().onDragDropEvent((e) => {
  const p = e.payload;
  if (p?.type === 'over') {
    const over = pointInDropzone(p.position);
    if (over !== stashOver) {
      stashOver = over;
      stashDropzone.classList.toggle('over', over);
    }
  } else if (p?.type === 'leave') {
    stashOver = false;
    stashDropzone.classList.remove('over');
  } else if (p?.type === 'drop') {
    const wasOver = pointInDropzone(p.position);
    stashOver = false;
    stashDropzone.classList.remove('over');
    if (wasOver && Array.isArray(p.paths) && p.paths.length > 0) {
      invoke('stash_add', { paths: p.paths }).catch(() => {});
    }
  }
});

// Initial paint + live updates from Rust.
invoke('stash_list').then(renderStash).catch(() => {});
listen('stash:changed', (e) => renderStash(e.payload));

// Settings section
const settingsToggle = document.getElementById('settings-toggle');
const settingsList = document.getElementById('settings-list');
const setAutostart = document.getElementById('set-autostart');
settingsToggle.addEventListener('click', () => {
  const expanded = settingsToggle.getAttribute('aria-expanded') === 'true';
  settingsToggle.setAttribute('aria-expanded', String(!expanded));
  settingsList.classList.toggle('collapsed', expanded);
});
// Initialise the autostart toggle from the registry-backed truth source.
invoke('get_autostart').then((on) => { setAutostart.checked = !!on; }).catch(() => {});
setAutostart.addEventListener('change', (e) => {
  invoke('set_autostart', { enable: e.target.checked }).catch(() => {});
});

// Quick toggles — `data-uri` opens a Windows Settings deep link,
// `data-action` runs a named backend command instead.
const QUICK_ACTIONS = {
  'minimize-all': () => invoke('minimize_all_windows'),
};
document.querySelectorAll('.quick-btn').forEach(btn => {
  btn.addEventListener('click', () => {
    const uri = btn.dataset.uri;
    const action = btn.dataset.action;
    if (uri) invoke('launch_uri', { uri }).catch(() => {});
    else if (action && QUICK_ACTIONS[action]) QUICK_ACTIONS[action]().catch(() => {});
  });
});

el.mediaPlay.addEventListener('click', () => invoke('media_toggle_play').catch(() => {}));
el.mediaNext.addEventListener('click', () => invoke('media_next').catch(() => {}));
el.mediaPrev.addEventListener('click', () => invoke('media_prev').catch(() => {}));

el.volSlider.addEventListener('input', (e) => {
  const pct = parseInt(e.target.value, 10);
  el.volPct.textContent = `${pct}%`;
  invoke('set_volume', { percent: pct }).catch(() => {});
});
el.volIcon.addEventListener('click', () => {
  if (!lastSnapshot || !lastSnapshot.audio) return;
  invoke('set_mute', { muted: !lastSnapshot.audio.muted }).catch(() => {});
});

async function init() {
  await listen('hud:update', (e) => { lastSnapshot = e.payload; render(); });
  await listen('apps:changed', (e) => { runningApps = e.payload; renderApps(); });

  // Replay the entrance / exit CSS animations whenever the dock-toggle button
  // shows or hides the HUD. CSS animations don't auto-replay on window.show()
  // because the DOM doesn't change — we force it by toggling .hud-replay,
  // which restarts `animation: hud-in` via a no-op style flush.
  const hudEl = document.getElementById('hud');
  await listen('hud:show-anim', () => {
    hudEl.classList.remove('hiding');
    hudEl.style.animation = 'none';
    void hudEl.offsetHeight;
    hudEl.style.animation = '';
  });
  await listen('hud:hide-anim', () => {
    hudEl.classList.add('hiding');
  });

  render();
  renderApps();
  setInterval(render, 1000);
}
init();

// Drag handle
const drag = document.getElementById('drag');
let dragState = null;
drag.addEventListener('mousedown', async (e) => {
  if (e.button !== 0) return;
  const win = getCurrentWindow();
  const pos = await win.outerPosition();
  const scale = await win.scaleFactor();
  dragState = {
    startX: e.screenX,
    startY: e.screenY,
    winX: pos.x / scale,
    winY: pos.y / scale,
  };
  e.preventDefault();
});
window.addEventListener('mousemove', async (e) => {
  if (!dragState) return;
  const win = getCurrentWindow();
  const dx = e.screenX - dragState.startX;
  const dy = e.screenY - dragState.startY;
  await win.setPosition(new LogicalPosition(dragState.winX + dx, dragState.winY + dy));
});
window.addEventListener('mouseup', async () => {
  if (!dragState) return;
  const win = getCurrentWindow();
  const pos = await win.outerPosition();
  const scale = await win.scaleFactor();
  await invoke('set_hud_position', { x: pos.x / scale, y: pos.y / scale });
  dragState = null;
});
