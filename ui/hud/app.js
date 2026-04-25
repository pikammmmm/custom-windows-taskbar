const { listen } = window.__TAURI__.event;

const el = {
  time: document.getElementById('time'),
  date: document.getElementById('date'),
  down: document.getElementById('down'),
  up: document.getElementById('up'),
  mediaTitle: document.getElementById('media-title'),
  mediaArtist: document.getElementById('media-artist'),
};

let lastSnapshot = null;

function fmtRate(bps) {
  if (bps < 1024) return `${bps.toFixed(0)} B/s`;
  if (bps < 1024 * 1024) return `${(bps / 1024).toFixed(1)} KB/s`;
  return `${(bps / 1024 / 1024).toFixed(2)} MB/s`;
}

function render() {
  const now = new Date();
  const hh = String(now.getHours()).padStart(2, '0');
  const mm = String(now.getMinutes()).padStart(2, '0');
  el.time.textContent = `${hh}:${mm}`;
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
  }
}

async function init() {
  await listen('hud:update', (e) => { lastSnapshot = e.payload; render(); });
  render();
  setInterval(render, 1000);
}
init();

const { invoke } = window.__TAURI__.core;
const { getCurrentWindow, LogicalPosition } = window.__TAURI__.window;

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
  const newX = dragState.winX + dx;
  const newY = dragState.winY + dy;
  await win.setPosition(new LogicalPosition(newX, newY));
});

window.addEventListener('mouseup', async () => {
  if (!dragState) return;
  const win = getCurrentWindow();
  const pos = await win.outerPosition();
  const scale = await win.scaleFactor();
  await invoke('set_hud_position', { x: pos.x / scale, y: pos.y / scale });
  dragState = null;
});
