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

async function getIcon(exePath) {
  if (iconCache.has(exePath)) return iconCache.get(exePath);
  try {
    const url = await invoke('get_icon', { exePath });
    iconCache.set(exePath, url);
    return url;
  } catch {
    return '';
  }
}

function pinnedExePaths() {
  return new Set(pinned.map(p => p.path.toLowerCase()));
}

async function render() {
  const pinSet = pinnedExePaths();
  const runByExe = new Map(running.map(a => [a.exe_path.toLowerCase(), a]));
  const pinnedExtras = running.filter(a => !pinSet.has(a.exe_path.toLowerCase()));

  root.innerHTML = '';

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
  const img = document.createElement('img');
  img.src = await getIcon(exe);
  img.alt = label;
  node.appendChild(img);

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
  // Stop the menu's first click from bubbling and closing it.
  setTimeout(() => menu.addEventListener('click', (e) => e.stopPropagation()), 0);
}

async function init() {
  pinned = await invoke('get_pinned');
  await listen('pinned:changed', (e) => { pinned = e.payload; render(); });
  await listen('apps:changed', (e) => { running = e.payload; render(); });
  await render();
}

init();
