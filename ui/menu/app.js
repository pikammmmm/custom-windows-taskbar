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
// window. Works once the listener is registered (steady state).
listen('menu:items', (e) => maybeRender(e.payload));

// Reliable path: an unconditional 250ms poll of Rust's stashed
// last_menu_items. On first right-click of a session the listener race
// AND the focused=true event can both miss; on later right-clicks the
// items array changes and we need to detect it. Polling all the time is
// the only thing that's reliably caught both. The cost is one tiny IPC
// call every 250ms (a JSON pull); we only re-render when the payload
// actually differs from the one currently on screen.
let lastSig = '';
function sigOf(items) {
  // Cheap structural fingerprint — array length + per-item kind/label is
  // enough to spot any new menu invocation without diffing icons/etc.
  if (!Array.isArray(items)) return '';
  return items.map(i => `${i.kind}|${i.label || i.name || ''}`).join('§');
}
function maybeRender(items) {
  const sig = sigOf(items);
  if (sig === lastSig) return;
  lastSig = sig;
  renderItems(items);
}
async function pullItems() {
  try {
    const items = await invoke('get_menu_items');
    maybeRender(items);
  } catch {}
}
// 500ms is plenty: the menu is only useful for as long as the user can
// keep their cursor still on it, so a half-second to fill is fine and
// halves IPC traffic relative to the 250ms diagnostic interval.
setInterval(pullItems, 500);
pullItems();

const win = getCurrentWindow();
win.onFocusChanged(({ payload: focused }) => {
  if (!focused) invoke('hide_menu').catch(() => {});
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

function renderRow({ label, action, args, danger, windowRow }) {
  const row = document.createElement('div');
  row.className = 'menu-item';
  if (danger) row.classList.add('danger');
  if (windowRow) row.classList.add('window-row');
  // Glyphs were emoji-noisy and made the menu feel cluttered. Pure text
  // rows lean into the dock's minimal aesthetic; the row's accent colour
  // (blue for window-row, red for danger) carries enough signal.
  const lab = document.createElement('span');
  lab.className = 'label';
  lab.textContent = label;
  row.appendChild(lab);
  row.addEventListener('click', async () => {
    if (action) {
      // Log every menu action so we have a trace when something silently
      // fails — the catch{} below otherwise swallows the error and the
      // user just sees "nothing happened" with no breadcrumb.
      invoke('dbg_log', {
        message: `menu click action=${action} args=${JSON.stringify(args || {})}`,
      }).catch(() => {});
      try {
        await invoke(action, args || {});
      } catch (err) {
        invoke('dbg_log', {
          message: `menu click action=${action} FAILED: ${err}`,
        }).catch(() => {});
      }
    }
    await invoke('hide_menu').catch(() => {});
  });
  return row;
}

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') invoke('hide_menu').catch(() => {});
});
