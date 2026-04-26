const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow, LogicalPosition } = window.__TAURI__.window;

const el = {
  time: document.getElementById('time'),
  date: document.getElementById('date'),
  down: document.getElementById('down'),
  up: document.getElementById('up'),
  mediaTitle: document.getElementById('media-title'),
  mediaArtist: document.getElementById('media-artist'),
  volSlider: document.getElementById('vol-slider'),
  volPct: document.getElementById('vol-pct'),
  volIcon: document.getElementById('vol-icon'),
  appsList: document.getElementById('apps-list'),
  appsLabel: document.getElementById('apps-label'),
};

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

function render() {
  const now = new Date();
  el.time.textContent = `${String(now.getHours()).padStart(2,'0')}:${String(now.getMinutes()).padStart(2,'0')}`;
  el.date.textContent = now.toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' });

  if (lastSnapshot) {
    el.down.textContent = fmtRate(lastSnapshot.network.down_bps);
    el.up.textContent = fmtRate(lastSnapshot.network.up_bps);

    const m = lastSnapshot.media;
    if (m.has_session && m.title) {
      el.mediaTitle.textContent = (m.playing ? '▶ ' : '⏸ ') + m.title;
      el.mediaArtist.textContent = m.artist || '';
    } else {
      el.mediaTitle.textContent = 'Nothing playing';
      el.mediaArtist.textContent = '';
    }

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
  }
}

function renderApps() {
  el.appsLabel.textContent = `Apps (${runningApps.length})`;
  el.appsList.innerHTML = '';
  const sorted = [...runningApps].sort((x, y) =>
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
    item.appendChild(nameSpan);
    item.appendChild(countSpan);
    item.addEventListener('click', () => {
      if (app.windows.length > 0) {
        invoke('focus_window', { hwnd: app.windows[0].hwnd });
      }
    });
    el.appsList.appendChild(item);
  }
}

function nameOf(app) {
  return app.exe_path.split('\\').pop().replace(/\.exe$/i, '');
}

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
  render();
  renderApps();
  setInterval(render, 1000);
}
init();

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
