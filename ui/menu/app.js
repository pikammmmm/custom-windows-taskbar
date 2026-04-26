const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const itemsRoot = document.getElementById('items');

function renderItems(items) {
  const list = Array.isArray(items) ? items : [];
  itemsRoot.innerHTML = '';
  for (const item of list) {
    itemsRoot.appendChild(renderItem(item));
  }
}

// Fast path — the dock-side `show_menu` emits this right before showing
// the window. Works once the listener is registered (steady state).
listen('menu:items', (e) => renderItems(e.payload));

// Reliable path — pull the last items via a Tauri command. Guarantees the
// menu populates even when the event arrived before the listener was ready
// (cold-start race on first right-click of a session).
async function pullItems() {
  try {
    const items = await invoke('get_menu_items');
    if (Array.isArray(items) && items.length > 0) renderItems(items);
  } catch {}
}

// Re-pull on every show. The dock calls show_menu → window becomes visible
// → onFocusChanged fires (focused=true on first show, focused=false on
// dismiss). Pull on focused=true; dismiss on focused=false.
const win = getCurrentWindow();
win.onFocusChanged(({ payload: focused }) => {
  if (focused) {
    pullItems();
  } else {
    invoke('hide_menu').catch(() => {});
  }
});

// First-load pull — covers the case where onFocusChanged doesn't fire on
// the very first show (no-activate windows can be flaky about it).
pullItems();

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
