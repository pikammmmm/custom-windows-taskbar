const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const root = document.getElementById('dock-row');
let pinned = [];
let running = [];
const iconCache = new Map();

let openMenu = null;
function closeMenu() {
  if (openMenu) { openMenu.remove(); openMenu = null; }
}
document.addEventListener('click', closeMenu);
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') closeMenu(); });

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

function pinnedExePaths() {
  return new Set(pinned.map(p => p.path.toLowerCase()));
}

const SYSTEM_EXES = new Set([
  'applicationframehost', 'sihost', 'startmenuexperiencehost',
  'searchhost', 'shellexperiencehost', 'textinputhost', 'systemsettings',
  'lockapp', 'usercpl', 'fontdrvhost', 'dwm', 'csrss', 'wininit',
  'services', 'lsass', 'svchost', 'taskhostw', 'runtimebroker',
  'ctfmon', 'conhost', 'dllhost', 'wmiprvse', 'explorer',
]);
function isSystemExe(exe) {
  const name = exe.split('\\').pop().replace(/\.exe$/i, '').toLowerCase();
  return SYSTEM_EXES.has(name);
}

async function render() {
  const pinSet = pinnedExePaths();
  const visibleRunning = running.filter(a => !isSystemExe(a.exe_path));
  const runByExe = new Map(visibleRunning.map(a => [a.exe_path.toLowerCase(), a]));
  const pinnedExtras = visibleRunning.filter(a => !pinSet.has(a.exe_path.toLowerCase()));

  root.innerHTML = '';
  root.appendChild(makeStartButton());
  if (pinned.length || pinnedExtras.length) {
    const sep = document.createElement('div');
    sep.className = 'dock-divider';
    root.appendChild(sep);
  }

  for (const p of pinned) {
    root.appendChild(await iconNode({
      exe: p.path,
      label: p.display_name,
      running: runByExe.get(p.path.toLowerCase()),
    }));
  }
  if (pinned.length && pinnedExtras.length) {
    const div = document.createElement('div');
    div.className = 'dock-divider';
    root.appendChild(div);
  }
  for (const a of pinnedExtras) {
    const label = a.exe_path.split('\\').pop().replace(/\.exe$/i, '');
    root.appendChild(await iconNode({
      exe: a.exe_path,
      label,
      running: a,
    }));
  }
}

async function iconNode({ exe, label, running }) {
  const node = document.createElement('div');
  node.className = 'dock-icon';

  const hwnd = running?.windows?.[0]?.hwnd ?? null;
  const url = await getIcon(exe, hwnd);
  if (url) {
    const img = document.createElement('img');
    img.src = url;
    img.alt = label;
    img.addEventListener('error', () => {
      img.replaceWith(makeFallback(label));
    });
    node.appendChild(img);
  } else {
    node.appendChild(makeFallback(label));
  }

  const tip = document.createElement('div');
  tip.className = 'tooltip';
  tip.textContent = running
    ? `${label} (${running.windows.length})`
    : label;
  node.appendChild(tip);

  if (running && running.windows.length > 0) {
    const dot = document.createElement('div');
    dot.className = 'dot' + (running.windows.length > 1 ? ' multi' : '');
    node.appendChild(dot);
  }

  node.addEventListener('click', () => onClick(exe, label, running));
  node.addEventListener('contextmenu', (e) => { e.preventDefault(); onRightClick(exe, label, running, e); });
  return node;
}

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
  closeMenu();
  const menu = document.createElement('div');
  menu.className = 'menu';
  menu.style.left = `${event.clientX}px`;
  menu.style.bottom = `${window.innerHeight - event.clientY + 8}px`;

  if (running && running.windows.length > 0) {
    for (const w of running.windows) {
      const item = document.createElement('div');
      item.className = 'menu-item';
      item.textContent = w.title || '(untitled)';
      item.addEventListener('click', () => {
        invoke('focus_window', { hwnd: w.hwnd });
        closeMenu();
      });
      menu.appendChild(item);
    }
    const sep = document.createElement('div');
    sep.className = 'menu-sep';
    menu.appendChild(sep);
  }

  const isPinned = pinned.some(p => p.path.toLowerCase() === exe.toLowerCase());
  const pinItem = document.createElement('div');
  pinItem.className = 'menu-item';
  pinItem.textContent = isPinned ? 'Unpin from dock' : 'Pin to dock';
  pinItem.addEventListener('click', async () => {
    if (isPinned) {
      const r = await invoke('unpin_app', { path: exe });
      pinned = r.pinned;
    } else {
      const r = await invoke('pin_app', { path: exe, displayName: label });
      pinned = r.pinned;
    }
    render();
    closeMenu();
  });
  menu.appendChild(pinItem);

  if (running && running.windows.length > 0) {
    const closeAll = document.createElement('div');
    closeAll.className = 'menu-item danger';
    closeAll.textContent = `Close all (${running.windows.length})`;
    closeAll.addEventListener('click', async () => {
      for (const w of running.windows) {
        await invoke('close_window', { hwnd: w.hwnd });
      }
      closeMenu();
    });
    menu.appendChild(closeAll);
  }

  document.body.appendChild(menu);
  openMenu = menu;
  setTimeout(() => menu.addEventListener('click', (e) => e.stopPropagation()), 0);
}

async function init() {
  pinned = await invoke('get_pinned');
  await listen('pinned:changed', (e) => { pinned = e.payload; render(); });
  await listen('apps:changed', (e) => { running = e.payload; render(); });
  await render();
}

init();
