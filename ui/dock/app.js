const { invoke, Channel } = window.__TAURI__.core;
const { listen, emit } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

// 1×1 transparent PNG that the OS uses as the drag follow-image. The drop
// target paints its own preview anyway, so a no-op image is fine.
const TRANSPARENT_PNG_B64 =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=';

/// Attach a native (OLE) drag source to `el`. Triggers after the cursor
/// moves >5px from mousedown so a normal click on the same element still
/// fires its click handler. Used so dock icons + stash rows can be dragged
/// out into Explorer / Discord / email attachments / etc.
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
      down = null;
      const onEvent = new Channel();
      invoke('plugin:drag|start_drag', {
        item: paths,
        image: TRANSPARENT_PNG_B64,
        onEvent,
      }).catch((err) => console.error('start_drag failed', err));
    }
  });
  el.addEventListener('mouseup',    () => { down = null; });
  el.addEventListener('mouseleave', () => { down = null; });
  el.addEventListener('dragstart',  (ev) => ev.preventDefault());
}

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
      isPinned: true,
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
      isPinned: false,
    });
    nodeByExe.set(a.exe_path.toLowerCase(), node);
    center.appendChild(node);
  }

  applyForegroundHighlight();
}

async function iconNode({ exe, label, running, animateLaunch, isPinned }) {
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
  // Pinned icons are reorderable via HTML5 drag — running-only "extras" are
  // not, since their order is implicit (process appearance order). We drop
  // the OLE drag-out path here: starting an OS drag races the reorder
  // gesture and the dock's primary use is rearranging, not exporting paths.
  if (isPinned) makeReorderable(node, exe);
  return node;
}

// ─────────────────────────────────────────────────────────────────────────
// Smooth pointer-event reorder. We avoid HTML5 native drag here because its
// browser-rendered ghost looks pixelated and other icons can't shift around
// it. Instead: capture the pointer, translate the dragged icon to follow
// the cursor, and slide siblings out of the way via CSS transitions.
// ─────────────────────────────────────────────────────────────────────────
const REORDER_THRESHOLD_PX = 5;
// Icon (44px) + center gap (6px). Used to compute the slide amount when
// shifting siblings out of the dragged icon's slot.
const SLOT_PX = 50;

function makeReorderable(node, exe) {
  node.dataset.reorderable = 'true';
  let active = null;

  node.addEventListener('pointerdown', (ev) => {
    if (ev.button !== 0) return;
    active = {
      pointerId: ev.pointerId,
      downX: ev.clientX,
      downY: ev.clientY,
      dragging: false,
      reorderables: null,
      originIndex: 0,
      insertIndex: null,
      // Cache the dragged icon's start position so we can compute the
      // cursor-relative translate without rect-querying every move.
      originLeft: 0,
    };
  });

  node.addEventListener('pointermove', (ev) => {
    if (!active || ev.pointerId !== active.pointerId) return;
    const dx = ev.clientX - active.downX;
    const dy = ev.clientY - active.downY;

    if (!active.dragging) {
      if (Math.hypot(dx, dy) < REORDER_THRESHOLD_PX) return;
      active.dragging = true;
      try { node.setPointerCapture(active.pointerId); } catch {}
      node.classList.add('dragging');
      active.reorderables = [...center.querySelectorAll('.dock-icon[data-reorderable]')];
      active.originIndex = active.reorderables.indexOf(node);
      active.originLeft = node.getBoundingClientRect().left;
    }

    ev.preventDefault();
    // Dragged icon follows cursor (lifted slightly + scaled). The vertical
    // translate is dampened so the user can flick sideways without the icon
    // drifting up off the dock.
    node.style.transform = `translate(${dx}px, ${Math.max(-12, dy * 0.4)}px) scale(1.12)`;

    const insertIndex = computeInsertIndex(ev.clientX, active.reorderables, node);
    if (insertIndex !== active.insertIndex) {
      active.insertIndex = insertIndex;
      applySlotShifts(active.reorderables, node, active.originIndex, insertIndex);
    }
  });

  function endDrag() {
    if (!active) return;
    const a = active;
    active = null;
    if (!a.dragging) return;

    // Suppress the click that fires right after pointerup — pointer events
    // don't auto-suppress synthetic clicks like HTML5 drag does, so without
    // this guard every reorder would also launch the app.
    node._suppressNextClick = true;

    // Reset siblings — their re-render after pinned:changed will paint them
    // in the right place, and clearing inline transforms lets the CSS
    // transition glide them home if the order didn't change.
    for (const ic of a.reorderables) {
      if (ic !== node) { ic.style.transform = ''; }
    }

    if (a.insertIndex !== null && a.insertIndex !== a.originIndex) {
      const exes = pinned.map(p => p.path);
      const draggedLc = exe.toLowerCase();
      const filtered = exes.filter(p => p.toLowerCase() !== draggedLc);
      const originalExe = exes.find(p => p.toLowerCase() === draggedLc) || exe;
      const insertAt = Math.min(a.insertIndex, filtered.length);
      const newOrder = [
        ...filtered.slice(0, insertAt),
        originalExe,
        ...filtered.slice(insertAt),
      ];
      invoke('set_pinned_order', { paths: newOrder }).catch(() => {});
    }

    node.style.transform = '';
    node.classList.remove('dragging');
  }

  node.addEventListener('pointerup', endDrag);
  node.addEventListener('pointercancel', endDrag);
  // Capture-phase click guard — read the flag set in endDrag and swallow
  // the click before iconNode's own click handler fires.
  node.addEventListener('click', (ev) => {
    if (node._suppressNextClick) {
      node._suppressNextClick = false;
      ev.stopImmediatePropagation();
      ev.preventDefault();
    }
  }, true);
}

/// Find the index in `reorderables` where the dragged icon would land if
/// dropped at `cursorX`. Returns the original index when the cursor is
/// over the dragged icon itself so we don't oscillate when stationary.
function computeInsertIndex(cursorX, reorderables, draggedNode) {
  let idx = reorderables.length;
  for (let i = 0; i < reorderables.length; i++) {
    const ic = reorderables[i];
    if (ic === draggedNode) continue;
    const rect = ic.getBoundingClientRect();
    if (cursorX < rect.left + rect.width / 2) {
      idx = reorderables.indexOf(ic);
      break;
    }
  }
  return idx;
}

/// Translate every sibling so that the dragged icon's destination slot is
/// empty. Icons between origin and destination shift one slot toward origin.
function applySlotShifts(reorderables, draggedNode, originIndex, insertIndex) {
  for (let i = 0; i < reorderables.length; i++) {
    const ic = reorderables[i];
    if (ic === draggedNode) continue;
    let shift = 0;
    if (insertIndex > originIndex) {
      // Dragged is moving right — icons between (origin, insertIndex] slide left.
      if (i > originIndex && i < insertIndex) shift = -SLOT_PX;
    } else if (insertIndex < originIndex) {
      // Dragged is moving left — icons between [insertIndex, origin) slide right.
      if (i >= insertIndex && i < originIndex) shift = SLOT_PX;
    }
    ic.style.transform = shift ? `translateX(${shift}px)` : '';
  }
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
  // Glass tile with a magnifier inside — signals "launcher" (this opens the
  // spotlight) without being yet another Windows Start riff. Stroke-only so
  // the dock's hover/blur effects show through cleanly.
  node.innerHTML = `
    <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
      <rect x="3" y="3" width="18" height="18" rx="5"
            fill="rgba(92, 182, 255, 0.10)"
            stroke="rgba(180, 215, 255, 0.85)" stroke-width="1.5"/>
      <circle cx="11" cy="11" r="3.6" fill="none"
              stroke="#5cb6ff" stroke-width="1.7"/>
      <line x1="13.6" y1="13.6" x2="16.5" y2="16.5"
            stroke="#5cb6ff" stroke-width="1.8" stroke-linecap="round"/>
    </svg>
    <div class="tooltip">Open launcher</div>
  `;
  node.addEventListener('click', (e) => {
    e.stopPropagation();
    // Show our glassy launcher instead of routing to Windows' Start menu
    // (we still expose `open_start_menu` for the rare case anyone wants it).
    invoke('show_spotlight').catch(() => {});
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

// Tray chips → HUD for net + clock. Volume gets a custom popup with the
// list of output devices, so click is more useful than toggling the HUD.
for (const id of ['tray-net', 'tray-clock']) {
  document.getElementById(id).addEventListener('click', () => {
    invoke('toggle_hud').catch(() => {});
  });
}
tray.vol.addEventListener('click', async (e) => {
  const items = [{ kind: 'header', name: 'Output device' }];
  try {
    const devices = await invoke('list_audio_devices');
    if (Array.isArray(devices) && devices.length > 0) {
      items.push({ kind: 'separator' });
      for (const d of devices) {
        items.push({
          kind: 'item',
          glyph: d.is_default ? '●' : '○',
          label: d.name,
          action: 'set_default_audio_device',
          args: { id: d.id },
        });
      }
    } else {
      items.push({ kind: 'item', glyph: '·', label: 'No output devices', action: '', args: {} });
    }
  } catch {
    items.push({ kind: 'item', glyph: '!', label: 'Failed to list devices', action: '', args: {} });
  }
  items.push({ kind: 'separator' });
  items.push({
    kind: 'item', glyph: '⚙', label: 'Sound settings',
    action: 'launch_uri', args: { uri: 'ms-settings:sound' },
  });
  const dpr = window.devicePixelRatio || 1;
  await invoke('show_menu', {
    args: {
      items,
      x: Math.round(e.screenX * dpr),
      y: Math.round(e.screenY * dpr),
      width: 240,
      height: estimateMenuHeight(items),
    },
  }).catch(() => {});
});

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

  // Generic "Recent files" — top items from %APPDATA%\Microsoft\Windows\Recent.
  // Not per-app (would need parsing the binary AutomaticDestinations files);
  // best-effort signal that catches anything the user touched recently.
  try {
    const recents = await invoke('recent_files');
    if (Array.isArray(recents) && recents.length > 0) {
      items.push({ kind: 'separator' });
      for (const r of recents) {
        items.push({
          kind: 'item',
          glyph: '🕘',
          label: truncate(r.name, 36),
          action: 'launch_uri',
          args: { uri: r.path },
        });
      }
    }
  } catch {}

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
    // Tauri 2 deserializes JS args as camelCase and renames to snake_case
    // for Rust params — so `displayName` here lands as `display_name` in
    // pin_app's signature. Sending snake_case from JS does NOT match.
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

  // Drag-and-drop pinning. Tauri's window-scoped drag-drop event delivers
  // an array of OS file paths on drop; we hand them to pin_dropped which
  // resolves .lnk → exe and ignores anything that isn't launchable.
  await getCurrentWindow().onDragDropEvent((e) => {
    if (e.payload?.type === 'over') {
      document.body.classList.add('drop-active');
    } else if (e.payload?.type === 'leave') {
      document.body.classList.remove('drop-active');
    } else if (e.payload?.type === 'drop') {
      document.body.classList.remove('drop-active');
      const paths = e.payload.paths || [];
      if (paths.length > 0) invoke('pin_dropped', { paths }).catch(() => {});
    }
  });

  tickClock();
  setInterval(tickClock, 5_000);
  setInterval(pollForeground, 500);
  pollForeground();

  await renderCenter();
}
init();
