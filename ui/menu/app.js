const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const itemsRoot = document.getElementById('items');

function renderItems(items) {
  const list = Array.isArray(items) ? items : [];
  if (list.length === 0) return;
  itemsRoot.innerHTML = '';
  for (const item of list) {
    itemsRoot.appendChild(renderItem(item));
  }
}

// Fast path: the dock-side `show_menu` emits this just before showing the
// window. Once the listener is registered (steady state), this is the only
// path that needs to fire.
listen('menu:items', (e) => renderItems(e.payload));

// Reliable path: pull from Rust's stashed last_menu_items. Used both on
// focus and as a polling safety-net while the menu is visible-but-empty —
// the listener-registration race on first right-click can otherwise leave
// the menu blank indefinitely.
let pollHandle = null;
async function pullItems() {
  try {
    const items = await invoke('get_menu_items');
    if (Array.isArray(items) && items.length > 0) {
      renderItems(items);
      stopPolling();
    }
  } catch {}
}
function startPolling() {
  if (pollHandle != null) return;
  pollHandle = setInterval(pullItems, 200);
}
function stopPolling() {
  if (pollHandle != null) { clearInterval(pollHandle); pollHandle = null; }
}

const win = getCurrentWindow();
win.onFocusChanged(({ payload: focused }) => {
  if (focused) {
    // Re-render every show, in case the items array changed since last time.
    itemsRoot.innerHTML = '';
    pullItems();
    startPolling();
  } else {
    stopPolling();
    invoke('hide_menu').catch(() => {});
  }
});

// Cold-start nudge: if the focus event doesn't fire on the first show
// (focus quirks on always-on-top windows), the poll picks it up anyway.
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
