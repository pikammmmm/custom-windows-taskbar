const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const itemsRoot = document.getElementById('items');

// Render the items payload sent by the dock via Rust's `menu:items` event.
// Shape: array of { kind: 'header'|'separator'|'item', ...fields }.
listen('menu:items', (e) => {
  const items = Array.isArray(e.payload) ? e.payload : [];
  itemsRoot.innerHTML = '';
  for (const item of items) {
    itemsRoot.appendChild(renderItem(item));
  }
});

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

// Auto-dismiss: Escape key, or losing focus (clicking anywhere outside).
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') invoke('hide_menu').catch(() => {});
});

// Tauri's window blur fires when the user clicks outside the menu — exactly
// what we want for click-anywhere-to-dismiss. Listening on the focused-changed
// event covers it cross-platform.
const win = getCurrentWindow();
win.onFocusChanged(({ payload: focused }) => {
  if (!focused) invoke('hide_menu').catch(() => {});
});
