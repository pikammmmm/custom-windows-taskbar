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
  media: document.getElementById('tray-media'),
  mediaImg: document.getElementById('tray-media-img'),
  mediaFallback: document.getElementById('tray-media-fallback'),
  mediaOverlay: document.getElementById('tray-media-overlay'),
  mediaPrev: document.getElementById('tray-media-prev'),
  mediaArt: document.getElementById('tray-media-art'),
  mediaNext: document.getElementById('tray-media-next'),
};
// Track the currently-displayed thumbnail so we know when to actually
// touch the <img> src (which forces a re-decode). Comparing against the
// element's existing src is unreliable — browsers normalise data URLs.
let lastThumbnailSig = '';

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
// Recently-closed unpinned apps. Keyed lowercase exe path → { exe, label,
// closedAt }. We keep them visible in the dock with a greyed style for
// RECENTS_TTL_MS so the user can relaunch e.g. Terminal without going
// through the launcher (it isn't pinned, so without this it'd vanish the
// moment it closed).
const recentsByExe = new Map();
const RECENTS_TTL_MS = 5 * 60 * 1000;

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

// Maintain the recents map: exes that vanished from the running list since
// the previous render get an entry, and exes that are running now get
// pulled out. Called from renderCenter() before we compute the layout.
function updateRecents(currentRunningExes, visibleRunning) {
  const now = Date.now();
  // Drop expired entries first so the dock doesn't grow unbounded.
  for (const [key, val] of recentsByExe) {
    if (now - val.closedAt > RECENTS_TTL_MS) recentsByExe.delete(key);
  }
  // Anything we knew was running last tick but isn't now → moved to recents.
  // Skip pinned items (they're already present) and system exes.
  const pinSetLc = new Set(pinned.map(p => p.path.toLowerCase()));
  for (const exeLc of prevRunningExes) {
    if (currentRunningExes.has(exeLc)) continue;
    if (pinSetLc.has(exeLc)) continue;
    if (recentsByExe.has(exeLc)) continue;
    // Need the original-cased path for launch — pull it from the most
    // recent visibleRunning entry that matches when present, otherwise
    // from prevVisibleRunning (kept as a parallel cache below).
    const cached = prevVisibleRunningByExe.get(exeLc);
    if (!cached) continue;
    recentsByExe.set(exeLc, {
      exe: cached.exe_path,
      label: exeName(cached.exe_path),
      closedAt: now,
    });
  }
  // Anything currently running drops out of recents.
  for (const a of visibleRunning) {
    recentsByExe.delete(a.exe_path.toLowerCase());
  }
}

// Mirror of visibleRunning keyed by exe — lets updateRecents look up the
// original-cased exe path for an exe key that's just disappeared.
let prevVisibleRunningByExe = new Map();

// ─────────────────────────────────────────────────────────────────────────
// Center section: pinned + running + recents app icons
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
  updateRecents(currentRunningExes, visibleRunning);
  prevVisibleRunningByExe = new Map(visibleRunning.map(a => [a.exe_path.toLowerCase(), a]));
  prevRunningExes = currentRunningExes;
  firstRender = false;

  // Build a list of "recents to show" — exclude any that are now pinned or
  // running so the same exe never appears twice on the dock.
  const recents = [...recentsByExe.values()].filter(r => {
    const lc = r.exe.toLowerCase();
    return !pinSet.has(lc) && !currentRunningExes.has(lc);
  });

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
  if (recents.length && (pinned.length || pinnedExtras.length)) {
    const div = document.createElement('div');
    div.className = 'dock-divider';
    center.appendChild(div);
  }
  for (const r of recents) {
    const node = await iconNode({
      exe: r.exe,
      label: r.label,
      running: null,
      animateLaunch: false,
      isPinned: false,
      recent: true,
    });
    nodeByExe.set(r.exe.toLowerCase(), node);
    center.appendChild(node);
  }

  applyForegroundHighlight();
}

async function iconNode({ exe, label, running, animateLaunch, isPinned, recent }) {
  const node = document.createElement('div');
  node.className = 'dock-icon';
  if (animateLaunch) node.classList.add('just-launched');
  // Recents render dimmed so they read as "available to relaunch" instead
  // of "currently open". Click handler dispatches to launch instead of
  // focus when there's no running record.
  if (recent) node.classList.add('recent');

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
  if (recent) {
    tip.textContent = `${label} · click to relaunch`;
  } else {
    tip.textContent = running ? `${label} (${running.windows.length})` : label;
  }
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
  // Multi-window hover preview — show a popup listing every open window
  // for the app when the cursor lingers on its dock icon. Skipped for
  // single-window apps (the regular tooltip is enough) and recents
  // (no windows to show).
  attachWindowPreview(node, label, running);
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
  // 3×3 dot grid — universal "app drawer / launcher" icon. Two-tone glow:
  // brighter centre, dimmer corners, so it reads as a focused launcher
  // even when the dock is mid-blur.
  node.innerHTML = `
    <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
      <circle cx="6"  cy="6"  r="2" fill="rgba(180, 215, 255, 0.75)"/>
      <circle cx="12" cy="6"  r="2" fill="rgba(180, 215, 255, 0.85)"/>
      <circle cx="18" cy="6"  r="2" fill="rgba(180, 215, 255, 0.75)"/>
      <circle cx="6"  cy="12" r="2" fill="rgba(180, 215, 255, 0.85)"/>
      <circle cx="12" cy="12" r="2.2" fill="#5cb6ff"/>
      <circle cx="18" cy="12" r="2" fill="rgba(180, 215, 255, 0.85)"/>
      <circle cx="6"  cy="18" r="2" fill="rgba(180, 215, 255, 0.75)"/>
      <circle cx="12" cy="18" r="2" fill="rgba(180, 215, 255, 0.85)"/>
      <circle cx="18" cy="18" r="2" fill="rgba(180, 215, 255, 0.75)"/>
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

/// Pick a single letter for the media-chip fallback from the SMTC
/// source app's AUMID. Strips the package-family prefix (everything
/// before the first underscore) and the "!Method" suffix so we get
/// the human-recognisable app name for things like
/// "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify" → "S".
function mediaFallbackGlyph(sourceApp) {
  if (!sourceApp) return '♪';
  let s = sourceApp;
  // UWP pattern: "Publisher.AppName_hash!Entry" → take "Entry" portion
  // when present, else the AppName portion. Both lead with a recognisable
  // human label.
  const bang = s.indexOf('!');
  if (bang !== -1 && bang + 1 < s.length) {
    s = s.slice(bang + 1);
  } else {
    // Strip "Publisher." prefix on AUMIDs without an entry-point suffix.
    const dot = s.indexOf('.');
    if (dot !== -1 && dot + 1 < s.length) {
      s = s.slice(dot + 1);
    }
    s = s.replace(/\.exe$/i, '');
  }
  const ch = s.trim().charAt(0);
  return ch ? ch.toUpperCase() : '♪';
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
  // Media chip — show whatever's playing right now (Spotify track, browser
  // tab video, anything that registers with SMTC). Hidden when no session.
  const m = snap.media;
  if (m && m.has_session && m.title) {
    tray.media.hidden = false;
    if (m.thumbnail) {
      // Cheap content fingerprint — last 64 chars of the data URL is
      // enough to detect when Spotify swaps to a new cover even if the
      // length is identical. Without this, repeated assignments to
      // .src skip re-decoding and the old image stays on screen.
      const sig = m.thumbnail.slice(-64);
      if (sig !== lastThumbnailSig) {
        lastThumbnailSig = sig;
        tray.mediaImg.src = m.thumbnail;
      }
      tray.mediaImg.style.display = '';
      tray.mediaFallback.style.display = 'none';
    } else {
      // No artwork from the source — show the gradient with the source
      // app's first letter (S for Spotify, Y for YouTube tab, etc.) so
      // the chip carries some identity. Falls back to ♪ for sessions
      // that don't expose a SourceAppUserModelId at all.
      tray.mediaImg.removeAttribute('src');
      tray.mediaImg.style.display = 'none';
      tray.mediaFallback.textContent = mediaFallbackGlyph(m.source_app);
      tray.mediaFallback.style.display = '';
      lastThumbnailSig = '';
    }
    tray.mediaOverlay.textContent = m.playing ? '⏸' : '▶';
    const subtitle = [m.title, m.artist].filter(Boolean).join(' — ');
    tray.media.title = `${subtitle} (click to ${m.playing ? 'pause' : 'play'})`;
  } else {
    tray.media.hidden = true;
    lastThumbnailSig = '';
  }
}

// Tray chips → HUD for net + clock. Volume gets a custom popup with the
// list of output devices, so click is more useful than toggling the HUD.
for (const id of ['tray-net', 'tray-clock']) {
  document.getElementById(id).addEventListener('click', () => {
    invoke('toggle_hud').catch(() => {});
  });
}

// Media chip controls — hand off to the existing SMTC commands. The
// invoke fires-and-forgets; the next snapshot tick reflects the new
// state on the chip.
tray.mediaArt.addEventListener('click', () => {
  invoke('media_toggle_play').catch(() => {});
});
tray.mediaPrev.addEventListener('click', (ev) => {
  ev.stopPropagation();
  invoke('media_prev').catch(() => {});
});
tray.mediaNext.addEventListener('click', (ev) => {
  ev.stopPropagation();
  invoke('media_next').catch(() => {});
});
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
  const fg = await invoke('foreground_hwnd');
  // Single-window apps: classic focus / minimize toggle.
  if (running.windows.length === 1) {
    const target = running.windows[0].hwnd;
    if (fg === target) {
      await invoke('minimize_window', { hwnd: target });
    } else {
      await invoke('focus_window', { hwnd: target });
    }
    return;
  }
  // Multi-window apps: cycle. If one of this app's windows is currently
  // foreground, jump to the NEXT one (round-robin). Otherwise focus the
  // first. Matches the Windows-11 taskbar's "click again to cycle" UX —
  // users with 5 Chrome windows can step through them with repeated
  // dock-icon clicks instead of having to hover the preview every time.
  const idxOfFg = running.windows.findIndex(w => w.hwnd === fg);
  const targetIdx = idxOfFg >= 0
    ? (idxOfFg + 1) % running.windows.length
    : 0;
  await invoke('focus_window', { hwnd: running.windows[targetIdx].hwnd });
}

// ─────────────────────────────────────────────────────────────────────────
// Window-list preview. Hover an icon for ≥ HOVER_DELAY_MS and a card
// floats above it listing every open window for that app: title + glyph,
// click to focus, × to close. Stays open while the cursor is inside the
// card so the user can move into it. No DWM thumbnails — that needs
// per-window DwmRegisterThumbnail wiring; titles + a per-row close are
// the high-value 80% case for "I have 7 Chrome windows, which is which".
// ─────────────────────────────────────────────────────────────────────────
// 220ms is the sweet spot — slow enough that brushing past an icon
// while reordering doesn't pop a popup, fast enough that intentional
// hover feels instant. 350ms (the original) was too slow for users
// who didn't realise the feature existed.
const HOVER_DELAY_MS = 220;
const HIDE_DELAY_MS = 180;
let activePreview = null; // { node, popup, hideTimer, showTimer }

function dismissPreview() {
  if (!activePreview) return;
  if (activePreview.popup && activePreview.popup.parentNode) {
    activePreview.popup.parentNode.removeChild(activePreview.popup);
  }
  if (activePreview.showTimer) clearTimeout(activePreview.showTimer);
  if (activePreview.hideTimer) clearTimeout(activePreview.hideTimer);
  activePreview = null;
}

function attachWindowPreview(node, label, running) {
  if (!running || running.windows.length < 2) return;

  node.addEventListener('mouseenter', () => {
    // Close any other preview before opening this one.
    if (activePreview && activePreview.node !== node) dismissPreview();
    if (activePreview && activePreview.node === node) {
      // Cursor returned before hide-delay elapsed — cancel the hide.
      if (activePreview.hideTimer) {
        clearTimeout(activePreview.hideTimer);
        activePreview.hideTimer = null;
      }
      return;
    }
    const showTimer = setTimeout(() => showPreview(node, label, running), HOVER_DELAY_MS);
    activePreview = { node, popup: null, hideTimer: null, showTimer };
  });

  node.addEventListener('mouseleave', () => {
    if (!activePreview || activePreview.node !== node) return;
    // Defer the hide so the user can move into the popup without it
    // vanishing under their cursor in the gap.
    activePreview.hideTimer = setTimeout(dismissPreview, HIDE_DELAY_MS);
  });
}

function showPreview(anchorNode, label, running) {
  if (!activePreview || activePreview.node !== anchorNode) return;

  const popup = document.createElement('div');
  popup.className = 'window-preview glass-card';
  popup.setAttribute('role', 'menu');

  const head = document.createElement('div');
  head.className = 'window-preview-head';
  head.textContent = `${label} · ${running.windows.length} window${running.windows.length === 1 ? '' : 's'}`;
  popup.appendChild(head);

  const list = document.createElement('div');
  list.className = 'window-preview-list';
  for (const w of running.windows) {
    const row = document.createElement('div');
    row.className = 'window-preview-row';
    if (w.hwnd === foregroundHwnd) row.classList.add('foreground');

    const glyph = document.createElement('span');
    glyph.className = 'window-preview-glyph';
    glyph.textContent = '▢';

    const title = document.createElement('span');
    title.className = 'window-preview-title';
    title.textContent = w.title || '(untitled)';
    title.title = w.title || '';

    const close = document.createElement('button');
    close.className = 'window-preview-close';
    close.textContent = '×';
    close.title = 'Close window';
    close.addEventListener('click', (ev) => {
      ev.stopPropagation();
      invoke('close_hwnds', { hwnds: [w.hwnd] }).catch(() => {});
      row.remove();
      // If we just closed the last window, dismiss the popup.
      if (list.children.length === 0) dismissPreview();
    });

    row.appendChild(glyph);
    row.appendChild(title);
    row.appendChild(close);
    row.addEventListener('click', () => {
      invoke('focus_window', { hwnd: w.hwnd }).catch(() => {});
      dismissPreview();
    });
    list.appendChild(row);
  }
  popup.appendChild(list);

  // Position: anchor above the icon, centered horizontally, clamped to the
  // viewport. Measure after append so getBoundingClientRect reflects real
  // size — the popup uses `visibility: hidden` initially so the position
  // shift isn't visible.
  popup.style.visibility = 'hidden';
  document.body.appendChild(popup);
  const iconRect = anchorNode.getBoundingClientRect();
  const popupRect = popup.getBoundingClientRect();
  let left = iconRect.left + iconRect.width / 2 - popupRect.width / 2;
  const margin = 8;
  if (left < margin) left = margin;
  if (left + popupRect.width > window.innerWidth - margin) {
    left = window.innerWidth - margin - popupRect.width;
  }
  // Stack above the icon with a small gap; the dock sits at the bottom of
  // the screen, so "above" is always free space.
  let top = iconRect.top - popupRect.height - 10;
  if (top < margin) top = iconRect.bottom + 10; // fall back to below if no room above
  popup.style.left = `${Math.round(left)}px`;
  popup.style.top = `${Math.round(top)}px`;
  popup.style.visibility = '';

  // Keep the preview alive while the cursor is inside it.
  popup.addEventListener('mouseenter', () => {
    if (activePreview && activePreview.hideTimer) {
      clearTimeout(activePreview.hideTimer);
      activePreview.hideTimer = null;
    }
  });
  popup.addEventListener('mouseleave', () => {
    if (!activePreview) return;
    activePreview.hideTimer = setTimeout(dismissPreview, HIDE_DELAY_MS);
  });

  activePreview.popup = popup;
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
  // Instant volume update — fires from set_volume so the chip doesn't
  // wait for the next snapshot tick (~400ms). Without this listener the
  // dock chip lagged behind the HUD slider and the user saw it as
  // "the taskbar volume doesn't change."
  await listen('audio:changed', (e) => {
    const pct = e.payload;
    if (typeof pct !== 'number') return;
    tray.volIcon.textContent = volIconFor(pct, false);
    tray.volVal.textContent = `${pct}%`;
  });

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
