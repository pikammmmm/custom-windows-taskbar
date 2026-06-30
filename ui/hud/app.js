const { invoke, Channel } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow, LogicalPosition } = window.__TAURI__.window;

const el = {
  time: document.getElementById('time'),
  date: document.getElementById('date'),
  down: document.getElementById('down'),
  up: document.getElementById('up'),
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
  tempVal: document.getElementById('temp-val'),
  tempBar: document.getElementById('temp-bar'),
  wxIcon: document.getElementById('wx-icon'),
  wxTemp: document.getElementById('wx-temp'),
  wxCond: document.getElementById('wx-cond'),
  warpBtn: document.getElementById('warp-btn'),
  wxLoc: document.getElementById('wx-loc'),
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

// Volume user-intent cache. Set when the user moves the slider so
// render() can prefer it over snapshot.audio for ~1.2s — without
// this guard a snapshot poll landing during the drag (or just after)
// overwrites the slider with the previous value, making the drag
// look like it snapped back to the old volume.
let volumeIntent = null;
let volumeIntentAt = 0;

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

  // Media controls live in the dock tray now — see ui/dock/app.js.

  // Audio — user-intent has priority over snapshot for ~1.2s after
  // the user touches the slider. Without this guard, a snapshot poll
  // that lands during/just-after the drag would overwrite the slider
  // and the displayed % with the previous (stale) value, making the
  // drag look like it "snapped back" to the old volume.
  const a = lastSnapshot.audio;
  const recentDrag = volumeIntent !== null && (Date.now() - volumeIntentAt) < 1200;
  if (recentDrag) {
    el.volSlider.value = volumeIntent;
    el.volPct.textContent = `${volumeIntent}%`;
    el.volIcon.textContent = volIconFor(volumeIntent, false);
  } else if (a && a.has_device) {
    volumeIntent = null;
    if (document.activeElement !== el.volSlider) {
      el.volSlider.value = a.volume_percent;
    }
    el.volPct.textContent = `${a.volume_percent}%`;
    el.volIcon.textContent = volIconFor(a.volume_percent, a.muted);
  } else {
    el.volPct.textContent = '--';
    el.volIcon.textContent = '🔈';
  }

  // Sysstats — server now sends f32 percents so the values fluctuate
  // and trigger snapshot emits even on a quiet system. We round only at
  // display time so the bar fill animates with sub-percent precision.
  const ss = lastSnapshot.sysstats;
  if (ss) {
    el.cpuVal.textContent = `${Math.round(ss.cpu_percent)}%`;
    setBarLevel(el.cpuBar, ss.cpu_percent);
    el.ramVal.textContent = `${Math.round(ss.mem_percent)}%`;
    setBarLevel(el.ramBar, ss.mem_percent);
  }

  // CPU temperature — tries ACPI thermal zones first, falls back to
  // LibreHardwareMonitor / OpenHardwareMonitor WMI namespaces if either
  // is running. Bar treats 30°C → 100°C as the visual range. When no
  // source is available the chip becomes a clickable hint that opens
  // the LHM download page directly (set up once in init below).
  const tempChip = document.getElementById('temp-chip');
  const t = lastSnapshot.thermal;
  if (t && typeof t.celsius === 'number') {
    el.tempVal.textContent = `${t.celsius}°C`;
    const tempPct = Math.max(0, Math.min(100, ((t.celsius - 30) / 70) * 100));
    setBarLevel(el.tempBar, tempPct);
    tempChip.title = `CPU temperature · source: ${t.source || 'unknown'}`;
    tempChip.classList.remove('chip-nodata');
    tempChip.style.cursor = 'default';
  } else {
    el.tempVal.textContent = '—°';
    setBarLevel(el.tempBar, 0);
    tempChip.title =
      'No CPU temperature source found.\n' +
      'Click to open LibreHardwareMonitor — install it, enable ' +
      '"Publish to WMI" in its Options menu, leave it running. ' +
      'glassbar will pick up readings automatically.';
    tempChip.classList.add('chip-nodata');
    tempChip.style.cursor = 'pointer';
  }

  // Cloudflare WARP — colour the button + tooltip from the latest probe.
  const wp = lastSnapshot.warp;
  if (wp) {
    const status = !wp.installed ? 'unknown' : (wp.connected ? 'connected' : 'disconnected');
    el.warpBtn.dataset.status = status;
    el.warpBtn.title = !wp.installed
      ? 'Cloudflare WARP — not installed'
      : wp.connected
        ? 'Cloudflare WARP — Connected (click to disconnect)'
        : 'Cloudflare WARP — Disconnected (click to connect)';
  }

  // Weather
  const w = lastSnapshot.weather;
  if (w) {
    if (w.temp_c != null) {
      el.wxTemp.textContent = `${Math.round(w.temp_c)}°`;
      el.wxIcon.textContent = wxGlyph(w.code ?? -1);
      el.wxCond.textContent = w.condition || '';
      if (w.location) el.wxLoc.textContent = w.location;
    } else if (!w.location) {
      // No city set yet — point user at Settings → Weather city.
      el.wxTemp.textContent = '—';
      el.wxIcon.textContent = '·';
      el.wxCond.textContent = 'Pick a city in Settings';
      el.wxLoc.textContent = '';
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
// System tray mirror — visible Windows notification-area icons re-surfaced.
// Rust's tray_mirror poller emits `tray:changed` (Vec<TrayIcon>) while the
// HUD is visible; on show we also invoke `list_tray_icons` directly so we can
// tell "no icons" (resolves empty) from "tray unavailable" (rejects).
// ─────────────────────────────────────────────────────────────────────────
const trayBlock = document.getElementById('tray-block');
const trayToggle = document.getElementById('tray-toggle');
const trayBody = document.getElementById('tray-body');
const trayGrid = document.getElementById('tray-grid');
const trayLabel = document.getElementById('tray-label');
const trayEmpty = document.getElementById('tray-empty');
const trayMenu = document.getElementById('tray-menu');

let trayIcons = [];
let trayAvailable = true;
let trayMenuTarget = null;

function trayArgs(t) {
  // Rust params are snake_case; Tauri maps these camelCase keys for us.
  return { ownerHwnd: t.owner_hwnd, uid: t.uid, callbackMsg: t.callback_msg, version: t.version };
}

function trayName(t) {
  if (t.tooltip) return t.tooltip;
  const base = (t.exe_path || '').split('\\').pop().replace(/\.exe$/i, '');
  return base || 'app';
}

function renderTray() {
  trayLabel.textContent = trayIcons.length ? `System Tray · ${trayIcons.length}` : 'System Tray';
  trayGrid.innerHTML = '';
  if (!trayAvailable) {
    trayEmpty.textContent = 'Tray unavailable';
    trayEmpty.style.display = '';
    return;
  }
  if (!trayIcons.length) {
    trayEmpty.textContent = 'No tray icons';
    trayEmpty.style.display = '';
    return;
  }
  trayEmpty.style.display = 'none';
  for (const t of trayIcons) {
    const btn = document.createElement('button');
    btn.className = 'tray-icon';
    btn.title = trayName(t);
    const img = document.createElement('img');
    img.src = t.icon;
    img.alt = '';
    img.draggable = false;
    // If the data URL fails to decode, fall back to a letter chip so the grid
    // never shows a broken-image glyph.
    img.addEventListener('error', () => {
      btn.classList.add('tray-icon-fallback');
      btn.textContent = (trayName(t)[0] || '?').toUpperCase();
    });
    btn.appendChild(img);
    btn.addEventListener('click', (e) => {
      e.preventDefault();
      hideTrayMenu();
      invoke('tray_activate', trayArgs(t)).catch(() => {});
    });
    btn.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      openTrayMenu(e, t);
    });
    trayGrid.appendChild(btn);
  }
}

function refreshTray() {
  invoke('list_tray_icons')
    .then((list) => { trayAvailable = true; trayIcons = Array.isArray(list) ? list : []; renderTray(); })
    .catch((err) => {
      trayAvailable = false;
      renderTray();
      invoke('dbg_log', { message: `list_tray_icons failed: ${err}` }).catch(() => {});
    });
}

function openTrayMenu(e, t) {
  trayMenuTarget = t;
  trayMenu.hidden = false;
  // Measure after un-hiding so width/height are real, then clamp to viewport.
  const rect = trayMenu.getBoundingClientRect();
  const vw = document.documentElement.clientWidth;
  const vh = document.documentElement.clientHeight;
  let x = e.clientX;
  let y = e.clientY;
  if (x + rect.width > vw) x = vw - rect.width - 6;
  if (y + rect.height > vh) y = vh - rect.height - 6;
  trayMenu.style.left = `${Math.max(6, x)}px`;
  trayMenu.style.top = `${Math.max(6, y)}px`;
}

function hideTrayMenu() {
  trayMenu.hidden = true;
  trayMenuTarget = null;
}

trayMenu.querySelectorAll('[data-tray-action]').forEach((item) => {
  item.addEventListener('click', () => {
    const t = trayMenuTarget;
    if (!t) return;
    const action = item.dataset.trayAction;
    if (action === 'activate') invoke('tray_activate', trayArgs(t)).catch(() => {});
    else if (action === 'menu') invoke('tray_open_menu', trayArgs(t)).catch(() => {});
    else if (action === 'quit') invoke('tray_quit', { ownerHwnd: t.owner_hwnd }).catch(() => {});
    else if (action === 'force-quit') invoke('tray_force_quit', { ownerHwnd: t.owner_hwnd }).catch(() => {});
    hideTrayMenu();
  });
});

// Dismiss the menu on any outside click, Escape, or scroll.
window.addEventListener('click', (e) => {
  if (!trayMenu.hidden && !trayMenu.contains(e.target)) hideTrayMenu();
});
window.addEventListener('keydown', (e) => { if (e.key === 'Escape') hideTrayMenu(); });
document.querySelector('.hud-scroll')?.addEventListener('scroll', hideTrayMenu, { passive: true });

trayToggle.addEventListener('click', () => {
  const expanded = trayToggle.getAttribute('aria-expanded') === 'true';
  trayToggle.setAttribute('aria-expanded', String(!expanded));
  trayBody.classList.toggle('collapsed', expanded);
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

// Tauri's window-scoped drag-drop event delivers OS file paths. The HUD
// has no other drop targets, so we just accept anywhere on the window —
// trying to gate on dropzone-bounding-rect was racing scroll offsets and
// dpr conversion and silently dropping otherwise-valid payloads.
getCurrentWindow().onDragDropEvent((e) => {
  const p = e.payload;
  if (p?.type === 'over' || p?.type === 'enter') {
    stashDropzone.classList.add('over');
  } else if (p?.type === 'leave') {
    stashDropzone.classList.remove('over');
  } else if (p?.type === 'drop') {
    stashDropzone.classList.remove('over');
    if (Array.isArray(p.paths) && p.paths.length > 0) {
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

// ─────────────────────────────────────────────────────────────────────────
// Weather-city picker — Open-Meteo geocoding behind a debounced input. Pick
// a result and we save name + lat + lon; weather probe re-reads settings
// on its next tick and the HUD flips without a glassbar restart.
// ─────────────────────────────────────────────────────────────────────────
const citySearch = document.getElementById('city-search');
const cityResults = document.getElementById('city-results');
const cityCurrent = document.getElementById('settings-city-current');

invoke('get_weather_city')
  .then((c) => { if (c) cityCurrent.textContent = c.name; })
  .catch(() => {});

let cityDebounceTimer = null;
citySearch?.addEventListener('input', () => {
  clearTimeout(cityDebounceTimer);
  const q = citySearch.value.trim();
  if (!q) { cityResults.innerHTML = ''; return; }
  // 280 ms debounce — long enough that fast typists don't fire 8 requests
  // for "ljubljana" but short enough to feel responsive.
  cityDebounceTimer = setTimeout(async () => {
    const results = await invoke('geocode_city', { query: q }).catch(() => []);
    cityResults.innerHTML = '';
    for (const r of results) {
      const li = document.createElement('li');
      const name = document.createElement('span');
      name.className = 'city-name';
      name.textContent = r.name;
      li.appendChild(name);
      const subParts = [r.admin, r.country].filter(Boolean);
      if (subParts.length) {
        const sub = document.createElement('span');
        sub.className = 'city-sub';
        sub.textContent = subParts.join(', ');
        li.appendChild(sub);
      }
      li.addEventListener('click', async () => {
        await invoke('set_weather_city', { name: r.name, lat: r.lat, lon: r.lon }).catch(() => {});
        cityCurrent.textContent = r.name;
        citySearch.value = '';
        cityResults.innerHTML = '';
      });
      cityResults.appendChild(li);
    }
  }, 280);
});

// ─────────────────────────────────────────────────────────────────────────
// Theme picker — accent swatches + glass tint + glass opacity.
// shared/theme.js already applies the live theme; this block just sends
// the user's edits back to the backend, which persists + re-broadcasts so
// every other panel (dock, clipboard, spotlight, menu) repaints too.
// ─────────────────────────────────────────────────────────────────────────
const themeSwatches = document.querySelectorAll('.theme-swatch[data-accent]');
const themeCustom = document.getElementById('theme-accent-custom');
const themeHue = document.getElementById('theme-hue');
const themeHueVal = document.getElementById('theme-hue-val');
const themeOpacity = document.getElementById('theme-opacity');
const themeOpacityVal = document.getElementById('theme-opacity-val');

let themeState = { accent: '#5cb6ff', glass_hue: 0, glass_opacity: 1.0 };

function syncThemeControls() {
  themeCustom.value = themeState.accent;
  themeHue.value = String(themeState.glass_hue ?? 0);
  themeHueVal.textContent = `${themeState.glass_hue ?? 0}°`;
  themeOpacity.value = String(themeState.glass_opacity ?? 1);
  themeOpacityVal.textContent = `${Math.round((themeState.glass_opacity ?? 1) * 100)}%`;
  themeSwatches.forEach(sw => {
    sw.classList.toggle('selected', sw.dataset.accent.toLowerCase() === themeState.accent.toLowerCase());
  });
}

function pushTheme() {
  invoke('set_theme', {
    accent: themeState.accent,
    glassHue: themeState.glass_hue,
    glassOpacity: themeState.glass_opacity,
  }).catch((err) => invoke('dbg_log', { message: `set_theme failed: ${err}` }).catch(() => {}));
}

// Seed the controls from the persisted theme on first paint.
invoke('get_theme').then((t) => {
  if (t) { themeState = t; syncThemeControls(); }
}).catch(() => {});

themeSwatches.forEach(sw => {
  sw.addEventListener('click', () => {
    themeState.accent = sw.dataset.accent;
    syncThemeControls();
    pushTheme();
  });
});
function localPreview() {
  // Live-paint just this window's CSS vars so the slider/picker feels
  // instant. pushTheme persists + broadcasts, which is on `change`
  // (commit), not `input` (every micro-movement) to keep settings.json
  // writes sane.
  const m = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(themeState.accent || '');
  const root = document.documentElement.style;
  root.setProperty('--accent', themeState.accent);
  if (m) root.setProperty('--accent-rgb', `${parseInt(m[1],16)}, ${parseInt(m[2],16)}, ${parseInt(m[3],16)}`);
  root.setProperty('--glass-hue', `${themeState.glass_hue}deg`);
  root.setProperty('--glass-opacity', String(themeState.glass_opacity));
}
themeCustom.addEventListener('input', (e) => {
  themeState.accent = e.target.value;
  syncThemeControls();
  localPreview();
});
themeCustom.addEventListener('change', pushTheme);
themeHue.addEventListener('input', (e) => {
  themeState.glass_hue = parseInt(e.target.value, 10);
  themeHueVal.textContent = `${themeState.glass_hue}°`;
  localPreview();
});
themeHue.addEventListener('change', pushTheme);
themeOpacity.addEventListener('input', (e) => {
  themeState.glass_opacity = parseFloat(e.target.value);
  themeOpacityVal.textContent = `${Math.round(themeState.glass_opacity * 100)}%`;
  localPreview();
});
themeOpacity.addEventListener('change', pushTheme);
// Keep the picker in sync if another window changes the theme.
listen('theme:changed', (e) => { themeState = e.payload; syncThemeControls(); });

// Quick toggles — `data-uri` opens a Windows Settings deep link,
// `data-action` runs a named backend command instead.
const QUICK_ACTIONS = {
  'minimize-all': () => invoke('minimize_all_windows'),
};
document.querySelectorAll('.quick-btn').forEach(btn => {
  btn.addEventListener('click', () => {
    const uri = btn.dataset.uri;
    const action = btn.dataset.action;
    const launchPath = btn.dataset.launch;
    if (uri) invoke('launch_uri', { uri }).catch(() => {});
    else if (launchPath) invoke('launch', { path: launchPath }).catch(() => {});
    else if (action && QUICK_ACTIONS[action]) QUICK_ACTIONS[action]().catch(() => {});
  });
});

// ─────────────────────────────────────────────────────────────────────────
// Power menu — collapsed by default. Destructive actions (signout/restart/
// shutdown) require a confirm tap within 3s; lock and sleep fire instantly.
// ─────────────────────────────────────────────────────────────────────────
const powerToggle = document.getElementById('power-toggle');
const powerGrid = document.getElementById('power-grid');
const powerCaret = document.getElementById('power-caret');
powerToggle?.addEventListener('click', () => {
  const expanded = powerToggle.getAttribute('aria-expanded') === 'true';
  powerToggle.setAttribute('aria-expanded', String(!expanded));
  powerGrid.classList.toggle('collapsed', expanded);
  powerCaret.textContent = expanded ? '▸' : '▾';
});

const POWER_DANGEROUS = new Set(['signout', 'restart', 'shutdown']);
const powerArmed = new Map();   // action → timeoutId
document.querySelectorAll('[data-power-action]').forEach((btn) => {
  const action = btn.dataset.powerAction;
  const label = btn.dataset.powerLabel;
  btn.addEventListener('click', () => {
    if (POWER_DANGEROUS.has(action) && !powerArmed.has(action)) {
      // First click — arm and wait for confirm. Disarm any other armed
      // dangerous button so only one is hot at a time.
      for (const [other, t] of powerArmed) {
        clearTimeout(t);
        const otherBtn = document.querySelector(`[data-power-action="${other}"]`);
        if (otherBtn) {
          otherBtn.classList.remove('armed');
          otherBtn.textContent = otherBtn.dataset.powerLabel;
        }
      }
      powerArmed.clear();
      btn.classList.add('armed');
      btn.textContent = `Tap to confirm`;
      const id = setTimeout(() => {
        btn.classList.remove('armed');
        btn.textContent = label;
        powerArmed.delete(action);
      }, 3000);
      powerArmed.set(action, id);
      return;
    }
    // Second click (or non-dangerous) — fire.
    if (powerArmed.has(action)) {
      clearTimeout(powerArmed.get(action));
      powerArmed.delete(action);
      btn.classList.remove('armed');
      btn.textContent = label;
    }
    invoke('power_action', { action }).catch(() => {});
  });
});

// WARP button: left-click toggles connection, right-click opens the app.
// Click is optimistic — we flip the local snapshot immediately so the
// button visually changes state without waiting for the next probe
// tick. Backend's warp_toggle then fires a warp:changed event after
// running the CLI so the truth catches up within ~400ms.
el.warpBtn.addEventListener('click', async () => {
  const wasConnected = lastSnapshot?.warp?.connected ?? false;
  const target = !wasConnected;
  // Optimistic flip so the button state visibly changes on every click,
  // not just after the next snapshot lands. The 5s polling lag was
  // making rapid clicks look like no-ops because lastSnapshot.warp
  // stayed at the pre-toggle value across multiple clicks.
  if (lastSnapshot && lastSnapshot.warp) {
    lastSnapshot.warp = { ...lastSnapshot.warp, connected: target };
    render();
  }
  try {
    await invoke('warp_toggle', { connect: target });
    showToast(target ? 'Connecting WARP…' : 'Disconnecting WARP…');
  } catch (err) {
    const msg = String(err || 'WARP toggle failed');
    // Optimistic flip was wrong — revert so the button reflects reality.
    if (lastSnapshot && lastSnapshot.warp) {
      lastSnapshot.warp = { ...lastSnapshot.warp, connected: wasConnected };
      render();
    }
    showToast(msg, 'bad');
    invoke('dbg_log', { message: `warp click error: ${msg}` }).catch(() => {});
  }
});

// warp:changed fires from Rust right after warp_toggle's CLI call so
// dock/HUD don't have to wait the full 5s probe interval to see the new
// state. Snapshot's next tick (400ms later) will carry the authoritative
// status — this event is just a "look alive" nudge.
listen('warp:changed', (e) => {
  if (lastSnapshot && lastSnapshot.warp) {
    lastSnapshot.warp = { ...lastSnapshot.warp, connected: !!e.payload };
    render();
  }
});
el.warpBtn.addEventListener('contextmenu', (e) => {
  e.preventDefault();
  invoke('launch', {
    path: 'C:\\Program Files\\Cloudflare\\Cloudflare WARP\\Cloudflare WARP.exe',
  }).catch(() => {});
});

// Media-control click handlers moved to dock/app.js (chip lives there now).

el.volSlider.addEventListener('input', async (e) => {
  const pct = parseInt(e.target.value, 10);
  // Optimistic UI first so the drag feels instant.
  volumeIntent = pct;
  volumeIntentAt = Date.now();
  el.volPct.textContent = `${pct}%`;
  el.volIcon.textContent = volIconFor(pct, false);
  try {
    // set_volume now returns the percent Windows actually committed —
    // endpoints snap to discrete steps so 75% can become 74 or 76. We
    // re-sync the intent to the returned value so the slider doesn't
    // visually jump when the intent window expires and the next
    // snapshot lands.
    const actual = await invoke('set_volume', { percent: pct });
    if (typeof actual === 'number') {
      volumeIntent = actual;
      volumeIntentAt = Date.now();
      if (actual !== pct) {
        if (document.activeElement !== el.volSlider) {
          el.volSlider.value = actual;
        }
        el.volPct.textContent = `${actual}%`;
        el.volIcon.textContent = volIconFor(actual, false);
      }
    }
  } catch {}
});
el.volIcon.addEventListener('click', () => {
  if (!lastSnapshot || !lastSnapshot.audio) return;
  invoke('set_mute', { muted: !lastSnapshot.audio.muted }).catch(() => {});
});

// Seed the slider from the *current* system volume on HUD reopen, plus
// pin the result into lastSnapshot.audio so the next render() doesn't
// immediately overwrite it with a stale snapshot value. Previously the
// seed was a brief flash and the next 1s render tick reset the slider
// to whatever lastSnapshot still held — that was the "volume breaks on
// reopen" symptom.
async function seedVolumeFromSettings() {
  try {
    // Live system query first.
    const state = await invoke('get_current_volume');
    let pct = null;
    let muted = false;
    let hasDevice = false;
    if (state && state.has_device) {
      pct = state.volume_percent;
      muted = !!state.muted;
      hasDevice = true;
    } else {
      // Fallback to persisted glassbar value if no audio device is
      // currently routable (briefly the case during device switches).
      const fallback = await invoke('get_settings_volume');
      if (typeof fallback === 'number') pct = fallback;
    }
    invoke('dbg_log', {
      message: `hud seed volume pct=${pct} muted=${muted} hasDevice=${hasDevice}`,
    }).catch(() => {});
    if (pct === null) return;
    if (document.activeElement !== el.volSlider) {
      el.volSlider.value = pct;
    }
    el.volPct.textContent = `${pct}%`;
    el.volIcon.textContent = volIconFor(pct, muted);
    // Pin into snapshot so the next 1s render() tick reads the same
    // value instead of pulling a pre-show audio reading and overwriting
    // the slider mid-frame.
    if (!lastSnapshot) lastSnapshot = {};
    lastSnapshot.audio = {
      ...(lastSnapshot.audio || {}),
      volume_percent: pct,
      muted,
      has_device: hasDevice || (lastSnapshot.audio && lastSnapshot.audio.has_device),
    };
  } catch (err) {
    invoke('dbg_log', { message: `hud seed volume FAILED: ${err}` }).catch(() => {});
  }
}

async function init() {
  await listen('hud:update', (e) => { lastSnapshot = e.payload; render(); });
  await listen('apps:changed', (e) => { runningApps = e.payload; renderApps(); });
  // System tray — poller emits this while the HUD is visible.
  await listen('tray:changed', (e) => {
    trayAvailable = true;
    trayIcons = Array.isArray(e.payload) ? e.payload : [];
    renderTray();
  });

  // Replay the entrance / exit CSS animations whenever the dock-toggle button
  // shows or hides the HUD. CSS animations don't auto-replay on window.show()
  // because the DOM doesn't change — we force it by toggling .hud-replay,
  // which restarts `animation: hud-in` via a no-op style flush.
  const hudEl = document.getElementById('hud');
  // Globals so Rust can poke us via WebviewWindow::eval as a backstop
  // — same pattern as the clipboard panel; emit_to('hud', …) was being
  // silently dropped on Tauri 2, leaving the volume slider stuck on
  // its previous value (the "sound resets when I reopen the HUD"
  // symptom: render() runs against a stale snapshot until the next
  // tick lands, so the slider shows whatever value it had + missing
  // entrance animation).
  window.__glassbarHudPlayShowAnim = () => {
    hudEl.classList.remove('hiding');
    hudEl.style.animation = 'none';
    void hudEl.offsetHeight;
    hudEl.style.animation = '';
    seedVolumeFromSettings();
    // Pull a fresh tray read on every show — also resolves the "Tray
    // unavailable" state that the event stream can't express.
    refreshTray();
    hideTrayMenu();
  };
  window.__glassbarHudPlayHideAnim = () => {
    hudEl.classList.add('hiding');
  };
  await listen('hud:show-anim', () => window.__glassbarHudPlayShowAnim());
  await listen('hud:hide-anim', () => window.__glassbarHudPlayHideAnim());

  // TEMP chip click → kicks off LHM install via elevated PowerShell.
  // The diagnostic dbg_log lets us confirm in debug.log that the click
  // actually fired — last build saw zero install_lhm activity and we
  // couldn't tell whether JS swallowed the event or the user never
  // clicked.
  document.getElementById('temp-chip').addEventListener('click', async () => {
    const t = lastSnapshot?.thermal;
    invoke('dbg_log', {
      message: `hud temp click thermalCelsius=${t?.celsius ?? 'null'} source=${t?.source ?? 'null'}`,
    }).catch(() => {});
    if (t && typeof t.celsius === 'number') return;
    showToast('Installing LibreHardwareMonitor… accept the UAC prompt');
    try {
      await invoke('install_lhm');
    } catch (err) {
      showToast(`LHM install failed: ${err}`, 'bad');
      invoke('dbg_log', { message: `install_lhm JS error: ${err}` }).catch(() => {});
    }
  });

  render();
  renderApps();
  renderTray();
  setInterval(render, 1000);
  // Seed once on first paint too so the very first show of the HUD
  // isn't blank-then-flash either.
  seedVolumeFromSettings();
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
