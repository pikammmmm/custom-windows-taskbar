const { invoke } = window.__TAURI__.core;
const { listen, emit } = window.__TAURI__.event;

const left = document.getElementById('dock-left');
const center = document.getElementById('dock-center');
const hudToggleSlot = document.getElementById('dock-hud-toggle');
const tray = {
  net: document.getElementById('tray-net'),
  netIcon: document.getElementById('tray-net-icon'),
  vol: document.getElementById('tray-vol'),
  volIcon: document.getElementById('tray-vol-icon'),
  volVal: document.getElementById('tray-vol-val'),
  clock: document.getElementById('tray-clock'),
  time: document.getElementById('tray-time'),
};

let pinned = [];
let running = [];
// Tracks which exes had at least one open window in the last render — a
// transition from absent → present is the "just launched" signal that
// triggers the bounce animation. Tracking pinned membership here would
// miss the case of a pinned-but-not-yet-running app starting up.
let prevRunningExes = new Set();
let firstRender = true;
let foregroundHwnd = null;
const iconCache = new Map();
// exe → DOM node, so foreground updates flip a class without a full re-render
// (which would kill hover states and replay animations on every focus change).
const nodeByExe = new Map();

async function getIcon(exePath, hwnd) {
  if (iconCache.has(exePath)) return iconCache.get(exePath);
  try {
    const url = await invoke('get_icon', { exePath, hwnd: hwnd ?? null });
    iconCache.set(exePath, url);
    return url;
  } catch {
    iconCache.set(exePath, '');
    return '';
  }
}

const SYSTEM_EXES = new Set([
  'applicationframehost', 'sihost', 'startmenuexperiencehost',
  'searchhost', 'shellexperiencehost', 'textinputhost', 'systemsettings',
  'lockapp', 'usercpl', 'fontdrvhost', 'dwm', 'csrss', 'wininit',
  'services', 'lsass', 'svchost', 'taskhostw', 'runtimebroker',
  'ctfmon', 'conhost', 'dllhost', 'wmiprvse', 'explorer',
]);
function exeName(exe) {
  return exe.split('\\').pop().replace(/\.exe$/i, '');
}
function isSystemExe(exe) {
  return SYSTEM_EXES.has(exeName(exe).toLowerCase());
}

// ─────────────────────────────────────────────────────────────────────────
// Center section: pinned + running app icons
// ─────────────────────────────────────────────────────────────────────────
async function renderCenter() {
  const pinSet = new Set(pinned.map(p => p.path.toLowerCase()));
  const visibleRunning = running.filter(a => !isSystemExe(a.exe_path));
  const runByExe = new Map(visibleRunning.map(a => [a.exe_path.toLowerCase(), a]));
  const pinnedExtras = visibleRunning.filter(a => !pinSet.has(a.exe_path.toLowerCase()));

  // Bounce any exe that has a running window now but didn't last render.
  // We compare against the running set (not the union with pinned) so that
  // launching a pinned app still triggers the animation.
  const currentRunningExes = new Set(visibleRunning.map(a => a.exe_path.toLowerCase()));
  const newlyLaunched = new Set();
  if (!firstRender) {
    for (const exe of currentRunningExes) {
      if (!prevRunningExes.has(exe)) newlyLaunched.add(exe);
    }
  }
  prevRunningExes = currentRunningExes;
  firstRender = false;

  center.innerHTML = '';
  nodeByExe.clear();

  for (const p of pinned) {
    const exe = p.path;
    const node = await iconNode({
      exe,
      label: p.display_name,
      running: runByExe.get(exe.toLowerCase()),
      animateLaunch: newlyLaunched.has(exe.toLowerCase()),
    });
    nodeByExe.set(exe.toLowerCase(), node);
    center.appendChild(node);
  }
  if (pinned.length && pinnedExtras.length) {
    const div = document.createElement('div');
    div.className = 'dock-divider';
    center.appendChild(div);
  }
  for (const a of pinnedExtras) {
    const node = await iconNode({
      exe: a.exe_path,
      label: exeName(a.exe_path),
      running: a,
      animateLaunch: newlyLaunched.has(a.exe_path.toLowerCase()),
    });
    nodeByExe.set(a.exe_path.toLowerCase(), node);
    center.appendChild(node);
  }

  applyForegroundHighlight();
}

async function iconNode({ exe, label, running, animateLaunch }) {
  const node = document.createElement('div');
  node.className = 'dock-icon';
  if (animateLaunch) node.classList.add('just-launched');

  const hwnd = running?.windows?.[0]?.hwnd ?? null;
  const url = await getIcon(exe, hwnd);
  if (url) {
    const img = document.createElement('img');
    img.src = url;
    img.alt = label;
    img.addEventListener('error', () => img.replaceWith(makeFallback(label)));
    node.appendChild(img);
  } else {
    node.appendChild(makeFallback(label));
  }

  const tip = document.createElement('div');
  tip.className = 'tooltip';
  tip.textContent = running ? `${label} (${running.windows.length})` : label;
  node.appendChild(tip);

  if (running && running.windows.length > 0) {
    const ind = document.createElement('div');
    ind.className = 'indicator';
    // Cap at 4 visible segments so a process with many windows doesn't
    // blow out the indicator strip.
    const segCount = Math.min(running.windows.length, 4);
    for (let i = 0; i < segCount; i++) {
      const seg = document.createElement('span');
      seg.className = 'seg';
      ind.appendChild(seg);
    }
    node.appendChild(ind);
  }

  // Track the running record on the node so foreground polling can read
  // the hwnd list without rebuilding state.
  node.dataset.exe = exe.toLowerCase();
  node._running = running || null;

  node.addEventListener('click', () => onClick(exe, label, running));
  node.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    onRightClick(exe, label, running, e);
  });
  return node;
}

const FALLBACK_PALETTE = [
  '#4f7cff', '#ef476f', '#06d6a0', '#ffd166',
  '#9d4edd', '#f8961e', '#43aa8b', '#ec4899',
  '#22d3ee', '#f97316', '#84cc16', '#a855f7',
];
function colorFor(name) {
  let h = 0;
  for (let i = 0; i < name.length; i++) {
    h = ((h << 5) - h + name.charCodeAt(i)) | 0;
  }
  return FALLBACK_PALETTE[Math.abs(h) % FALLBACK_PALETTE.length];
}
function makeFallback(label) {
  const span = document.createElement('span');
  span.className = 'icon-fallback';
  const cleaned = (label || '?').trim();
  span.textContent = (cleaned.charAt(0) || '?').toUpperCase();
  span.style.background = `linear-gradient(135deg, ${colorFor(cleaned)}, ${colorFor(cleaned + 'x')})`;
  return span;
}

// ─────────────────────────────────────────────────────────────────────────
// Left + HUD toggle (rendered once)
// ─────────────────────────────────────────────────────────────────────────
function makeStartButton() {
  const node = document.createElement('div');
  node.className = 'dock-icon start-button';
  node.innerHTML = `
    <svg viewBox="0 0 24 24" width="24" height="24" aria-hidden="true">
      <path fill="#5cb6ff" d="M3 5.5 11 4.4v7.1H3zM12 4.3 21 3v8.5h-9zM3 12.5h8v7.1L3 18.5zM12 12.5h9V21l-9-1.3z"/>
    </svg>
    <div class="tooltip">Start</div>
  `;
  node.addEventListener('click', (e) => {
    e.stopPropagation();
    invoke('open_start_menu').catch(() => {});
  });
  return node;
}

function makeHudToggleButton() {
  const node = document.createElement('div');
  node.className = 'dock-icon hud-toggle';
  node.innerHTML = `
    <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
      <rect x="3" y="3"  width="8" height="8"  rx="1.5" fill="none" stroke="#cfd6e4" stroke-width="1.6"/>
      <rect x="13" y="3" width="8" height="5"  rx="1.5" fill="#cfd6e4"/>
      <rect x="13" y="10" width="8" height="11" rx="1.5" fill="none" stroke="#cfd6e4" stroke-width="1.6"/>
      <rect x="3" y="13" width="8" height="8"  rx="1.5" fill="#cfd6e4"/>
    </svg>
    <div class="tooltip">Toggle widget panel</div>
  `;
  node.addEventListener('click', (e) => {
    e.stopPropagation();
    invoke('toggle_hud').catch(() => {});
  });
  return node;
}

// ─────────────────────────────────────────────────────────────────────────
// Foreground tracking — polled because Windows has no cheap "foreground
// changed" event without a winevent hook. 500ms feels live without spam.
// ─────────────────────────────────────────────────────────────────────────
async function pollForeground() {
  try {
    const hwnd = await invoke('foreground_hwnd');
    if (hwnd !== foregroundHwnd) {
      foregroundHwnd = hwnd;
      applyForegroundHighlight();
    }
  } catch {}
}
function applyForegroundHighlight() {
  for (const node of nodeByExe.values()) {
    const r = node._running;
    const isFg = !!(r && r.windows.some(w => w.hwnd === foregroundHwnd));
    node.classList.toggle('foreground', isFg);
  }
}

// ─────────────────────────────────────────────────────────────────────────
// Tray (clock + status chips) — driven by hud:update event broadcast by
// widget_state. The dock subscribes alongside the HUD window.
// ─────────────────────────────────────────────────────────────────────────
function tickClock() {
  const now = new Date();
  tray.time.textContent =
    `${String(now.getHours()).padStart(2,'0')}:${String(now.getMinutes()).padStart(2,'0')}`;
}

function volIconFor(pct, muted) {
  if (muted) return '🔇';
  if (pct === 0) return '🔈';
  if (pct < 50) return '🔉';
  return '🔊';
}

function updateTray(snap) {
  if (snap.audio && snap.audio.has_device) {
    tray.volIcon.textContent = volIconFor(snap.audio.volume_percent, snap.audio.muted);
    tray.volVal.textContent = `${snap.audio.volume_percent}%`;
  }
  if (snap.internet) {
    tray.netIcon.textContent = snap.internet.online ? '●' : '○';
    tray.net.classList.toggle('offline', !snap.internet.online);
    tray.net.title = snap.internet.online
      ? `Online${snap.internet.ping_ms != null ? ` · ${snap.internet.ping_ms} ms` : ''}`
      : 'Offline';
  }
}

// Tray chips all open the HUD when clicked — that's where the full controls
// (volume slider, settings shortcuts, network detail) actually live.
for (const id of ['tray-vol', 'tray-net', 'tray-clock']) {
  document.getElementById(id).addEventListener('click', () => {
    invoke('toggle_hud').catch(() => {});
  });
}

// ─────────────────────────────────────────────────────────────────────────
// Click handlers + right-click menu (unchanged behavior, just relocated
// here from the previous flat structure)
// ─────────────────────────────────────────────────────────────────────────
async function onClick(exe, _label, running) {
  if (!running || running.windows.length === 0) {
    await invoke('launch', { path: exe });
    return;
  }
  const targetHwnd = running.windows[0].hwnd;
  const fg = await invoke('foreground_hwnd');
  if (fg === targetHwnd) {
    await invoke('minimize_window', { hwnd: targetHwnd });
  } else {
    await invoke('focus_window', { hwnd: targetHwnd });
  }
}

async function onRightClick(exe, label, running, event) {
  const items = [];
  items.push({ kind: 'header', icon: await getIcon(exe, null), name: label });
  fetchAppInfo(exe, label, items).catch(() => {});

  if (running && running.windows.length > 0) {
    items.push({ kind: 'separator' });
    for (const w of running.windows) {
      items.push({
        kind: 'item',
        windowRow: true,
        glyph: '▢',
        label: truncate(w.title || '(untitled)', 36),
        action: 'focus_window',
        args: { hwnd: w.hwnd },
      });
    }
  }

  items.push({ kind: 'separator' });
  items.push({ kind: 'item', glyph: '↗', label: 'Launch new instance', action: 'launch',           args: { path: exe } });
  items.push({ kind: 'item', glyph: '📂', label: 'Show in Explorer',    action: 'show_in_explorer', args: { path: exe } });
  items.push({ kind: 'item', glyph: '📋', label: 'Copy path',           action: 'copy_to_clipboard', args: { text: exe } });
  items.push({ kind: 'item', glyph: 'ⓘ', label: 'Properties',          action: 'show_properties',  args: { path: exe } });
  items.push({ kind: 'item', glyph: '⛨', label: 'Run as administrator', action: 'run_as_admin',     args: { path: exe } });

  const isPinned = pinned.some(p => p.path.toLowerCase() === exe.toLowerCase());
  items.push({
    kind: 'item',
    glyph: isPinned ? '✕' : '📌',
    label: isPinned ? 'Unpin from dock' : 'Pin to dock',
    action: isPinned ? 'unpin_app' : 'pin_app',
    args: isPinned ? { path: exe } : { path: exe, displayName: label },
  });

  if (running && running.windows.length > 0) {
    items.push({ kind: 'separator' });
    items.push({
      kind: 'item',
      danger: true,
      glyph: '✕',
      label: `Close all (${running.windows.length})`,
      action: 'close_hwnds',
      args: { hwnds: running.windows.map(w => w.hwnd) },
    });
  }

  const dpr = window.devicePixelRatio || 1;
  await invoke('show_menu', {
    args: {
      items,
      x: Math.round(event.screenX * dpr),
      y: Math.round(event.screenY * dpr),
      width: 240,
      height: estimateMenuHeight(items),
    },
  }).catch(() => {});
}

async function fetchAppInfo(exe, _label, items) {
  const info = await invoke('app_info', { exePath: exe }).catch(() => null);
  if (!info) return;
  const idx = items.findIndex(i => i.kind === 'header');
  if (idx < 0) return;
  items[idx] = {
    ...items[idx],
    version: info.version || undefined,
    size: info.size_bytes ? formatSize(info.size_bytes) : undefined,
  };
  emit('menu:items', items).catch(() => {});
}

function estimateMenuHeight(items) {
  let total = 12;
  for (const item of items) {
    if (item.kind === 'separator') total += 9;
    else if (item.kind === 'header') total += 56;
    else total += 32;
  }
  return total;
}
function truncate(s, n) { return s.length > n ? s.slice(0, n - 1) + '…' : s; }
function formatSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

// Vertical wheel becomes horizontal scroll for the center icons row.
center.addEventListener('wheel', (e) => {
  if (center.scrollWidth <= center.clientWidth) return;
  if (e.deltaX === 0 && e.deltaY !== 0) {
    center.scrollLeft += e.deltaY;
    e.preventDefault();
  }
}, { passive: false });

// ─────────────────────────────────────────────────────────────────────────
// Init
// ─────────────────────────────────────────────────────────────────────────
async function init() {
  left.appendChild(makeStartButton());
  hudToggleSlot.appendChild(makeHudToggleButton());

  pinned = await invoke('get_pinned');

  await listen('pinned:changed', (e) => { pinned = e.payload; renderCenter(); });
  await listen('apps:changed',   (e) => { running = e.payload; renderCenter(); });
  await listen('hud:update',     (e) => updateTray(e.payload));

  tickClock();
  setInterval(tickClock, 5_000);
  setInterval(pollForeground, 500);
  pollForeground();

  await renderCenter();
}
init();
