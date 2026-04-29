const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const q = document.getElementById('q');
const list = document.getElementById('results');

let results = [];
let selected = 0;
const iconCache = new Map();

async function getIcon(exePath) {
  if (iconCache.has(exePath)) return iconCache.get(exePath);
  try {
    const url = await invoke('get_icon', { exePath, hwnd: null });
    iconCache.set(exePath, url);
    return url;
  } catch {
    iconCache.set(exePath, '');
    return '';
  }
}

const FALLBACK_PALETTE = [
  '#4f7cff', '#ef476f', '#06d6a0', '#ffd166',
  '#9d4edd', '#f8961e', '#43aa8b', '#ec4899',
];
function colorFor(s) {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = ((h << 5) - h + s.charCodeAt(i)) | 0;
  return FALLBACK_PALETTE[Math.abs(h) % FALLBACK_PALETTE.length];
}
function makeFallback(name) {
  const span = document.createElement('span');
  span.className = 'icon-fallback';
  const ch = ((name || '?').trim().charAt(0) || '?').toUpperCase();
  span.textContent = ch;
  span.style.background = `linear-gradient(135deg, ${colorFor(name)}, ${colorFor(name + 'x')})`;
  return span;
}

async function refresh() {
  try {
    results = await invoke('search_apps', { query: q.value });
  } catch {
    results = [];
  }
  selected = 0;
  await render();
}

async function render() {
  list.innerHTML = '';
  if (!results.length) {
    const empty = document.createElement('div');
    empty.className = 'spotlight-empty';
    empty.textContent = q.value.trim()
      ? 'No matches. Try a different query.'
      : 'Start typing to search apps…';
    list.appendChild(empty);
    return;
  }
  // Build all rows synchronously, then resolve icons in parallel and patch
  // them in. Avoids the visible left-to-right populate of awaiting per-row.
  const rows = results.map((r, i) => {
    const li = document.createElement('li');
    li.className = 'spotlight-row' + (i === selected ? ' selected' : '');

    const icon = document.createElement('div');
    icon.className = 'icon';
    icon.appendChild(makeFallback(r.name));

    const meta = document.createElement('div');
    meta.className = 'meta';
    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = r.name;
    const path = document.createElement('span');
    path.className = 'path';
    path.textContent = r.path;
    meta.appendChild(name);
    meta.appendChild(path);

    li.appendChild(icon);
    li.appendChild(meta);
    li.addEventListener('click', () => launch(i));
    list.appendChild(li);
    return { li, icon, name: r.name };
  });

  await Promise.all(rows.map(async ({ icon, name }, idx) => {
    const url = await getIcon(results[idx].path);
    if (!url) return;
    const img = document.createElement('img');
    img.src = url;
    img.alt = name;
    img.addEventListener('error', () => img.replaceWith(makeFallback(name)));
    icon.replaceChildren(img);
  }));

  const sel = list.querySelector('.selected');
  if (sel) sel.scrollIntoView({ block: 'nearest' });
}

function move(delta) {
  if (!results.length) return;
  selected = Math.max(0, Math.min(results.length - 1, selected + delta));
  // Update selection class without re-rendering — preserves icons + scroll.
  const rows = list.querySelectorAll('.spotlight-row');
  rows.forEach((r, i) => r.classList.toggle('selected', i === selected));
  const sel = rows[selected];
  if (sel) sel.scrollIntoView({ block: 'nearest' });
}

async function launch(i) {
  const r = results[i];
  if (!r) return;
  await invoke('launch', { path: r.path }).catch(() => {});
  hide();
}

async function hide() {
  q.value = '';
  results = [];
  await invoke('hide_spotlight').catch(() => {});
}

q.addEventListener('input', refresh);
q.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') { e.preventDefault(); hide(); }
  else if (e.key === 'ArrowDown') { e.preventDefault(); move(1); }
  else if (e.key === 'ArrowUp')   { e.preventDefault(); move(-1); }
  else if (e.key === 'Enter')     { e.preventDefault(); launch(selected); }
});

listen('spotlight:show', () => {
  q.value = '';
  q.focus();
  refresh();
});

const win = getCurrentWindow();
win.onFocusChanged(({ payload: focused }) => {
  if (!focused) hide();
});

refresh();
q.focus();
