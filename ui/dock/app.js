const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const root = document.getElementById('dock-row');
let pinned = [];
let running = [];
const iconCache = new Map();

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

async function onRightClick(exe, label, running, _event) {
  // Right-click menu added in Task 11.
  console.log('right-click', exe, label, running);
}

async function init() {
  pinned = await invoke('get_pinned');
  await listen('pinned:changed', (e) => { pinned = e.payload; render(); });
  await listen('apps:changed', (e) => { running = e.payload; render(); });
  await render();
}

init();
