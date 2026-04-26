const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const itemsRoot = document.getElementById('items');
const probe = document.getElementById('probe');

function setProbe(s) { if (probe) probe.textContent = s; }
setProbe('js');

function renderItems(items) {
  const list = Array.isArray(items) ? items : [];
  if (list.length === 0) {
    setProbe(`render:0`);
    return; // keep "no items yet" placeholder so we can see we got here
  }
  itemsRoot.innerHTML = '';
  for (const item of list) {
    itemsRoot.appendChild(renderItem(item));
  }
  setProbe(`render:${list.length}`);
}

// Fast path
listen('menu:items', (e) => {
  setProbe(`evt:${Array.isArray(e.payload) ? e.payload.length : '?'}`);
  renderItems(e.payload);
}).then(() => setProbe('listening'));

// Pull path — also runs on a slow tick so timing/focus quirks can't keep
// the menu blank forever. Cheap (single IPC call).
async function pullItems() {
  try {
    const items = await invoke('get_menu_items');
    setProbe(`pull:${Array.isArray(items) ? items.length : '?'}`);
    if (Array.isArray(items) && items.length > 0) renderItems(items);
  } catch (e) {
    setProbe(`err:${String(e).slice(0, 12)}`);
  }
}

const win = getCurrentWindow();
win.onFocusChanged(({ payload: focused }) => {
  setProbe(`focus:${focused}`);
  if (focused) pullItems();
  else invoke('hide_menu').catch(() => {});
});

pullItems();
setInterval(pullItems, 250); // safety-net poll while diagnosing

function renderItem(item) {
  if (item.kind === 'separator') {
    const d = document.createElement('div');
    d.className = 'menu-sep';
    return d;
  }
  if (item.kind === 'header') {
    return renderHeader(item);
  }
  return renderRow(item);
}

function renderHeader({ icon, name, version, size }) {
  const row = document.createElement('div');
  row.className = 'menu-header';
  if (icon) {
    const img = document.createElement('img');
    img.className = 'icon';
    img.src = icon;
    img.addEventListener('error', () => img.replaceWith(makeFallback(name)));
    row.appendChild(img);
  } else {
    row.appendChild(makeFallback(name));
  }
  const meta = document.createElement('div');
  meta.className = 'meta';
  const n = document.createElement('div');
  n.className = 'name';
  n.textContent = name || '';
  const s = document.createElement('div');
  s.className = 'sub';
  s.textContent = [version, size].filter(Boolean).join(' · ');
  meta.appendChild(n);
  if (s.textContent) meta.appendChild(s);
  row.appendChild(meta);
  return row;
}

function makeFallback(name) {
  const span = document.createElement('span');
  span.className = 'icon-fallback';
  span.textContent = ((name || '?').trim().charAt(0) || '?').toUpperCase();
  return span;
}

function renderRow({ label, glyph, action, args, danger, windowRow }) {
  const row = document.createElement('div');
  row.className = 'menu-item';
  if (danger) row.classList.add('danger');
  if (windowRow) row.classList.add('window-row');
  if (glyph) {
    const g = document.createElement('span');
    g.className = 'glyph';
    g.textContent = glyph;
    row.appendChild(g);
  }
  const lab = document.createElement('span');
  lab.className = 'label';
  lab.textContent = label;
  row.appendChild(lab);
  row.addEventListener('click', async () => {
    if (action) {
      try { await invoke(action, args || {}); } catch {}
    }
    await invoke('hide_menu').catch(() => {});
  });
  return row;
}

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') invoke('hide_menu').catch(() => {});
});
