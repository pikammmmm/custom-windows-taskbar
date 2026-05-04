const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const itemsRoot = document.getElementById('items');
const filterInput = document.getElementById('filter');
const clearBtn = document.getElementById('clear');

let entries = [];
let filtered = [];
let activeIdx = 0;

async function refresh() {
  try {
    entries = await invoke('clipboard_history');
  } catch {
    entries = [];
  }
  applyFilter();
}

function applyFilter() {
  const q = filterInput.value.trim().toLowerCase();
  filtered = q
    ? entries.filter(e => e.text.toLowerCase().includes(q))
    : entries.slice();
  if (activeIdx >= filtered.length) activeIdx = 0;
  render();
}

function render() {
  itemsRoot.innerHTML = '';
  if (filtered.length === 0) {
    const empty = document.createElement('div');
    empty.className = 'clip-empty';
    empty.textContent = entries.length === 0
      ? 'Copy something to see it here.'
      : 'No matches.';
    itemsRoot.appendChild(empty);
    return;
  }
  filtered.forEach((entry, idx) => {
    const li = document.createElement('li');
    li.className = 'clip-row';
    if (idx === activeIdx) li.classList.add('active');

    const text = document.createElement('div');
    text.className = 'clip-row-text';
    text.textContent = entry.text;

    const meta = document.createElement('div');
    meta.className = 'clip-row-meta';
    const lines = entry.text.split('\n').length;
    meta.textContent = `${formatAge(entry.age_secs)} · ${entry.text.length} ch${lines > 1 ? ` · ${lines} lines` : ''}`;

    li.appendChild(text);
    li.appendChild(meta);
    li.addEventListener('click', () => useEntry(entry.text));
    itemsRoot.appendChild(li);
  });
  // Keep the active row in view when navigating via keyboard.
  const activeRow = itemsRoot.children[activeIdx];
  if (activeRow && activeRow.scrollIntoView) {
    activeRow.scrollIntoView({ block: 'nearest' });
  }
}

function formatAge(secs) {
  if (secs < 5)   return 'just now';
  if (secs < 60)  return `${secs}s ago`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
  return `${Math.floor(secs / 86400)}d ago`;
}

async function useEntry(text) {
  try {
    await invoke('clipboard_use_entry', { text });
  } catch {}
}

filterInput.addEventListener('input', applyFilter);

clearBtn.addEventListener('click', async () => {
  try { await invoke('clipboard_clear'); } catch {}
  await refresh();
});

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    invoke('hide_clipboard').catch(() => {});
    return;
  }
  if (e.key === 'ArrowDown') {
    if (filtered.length === 0) return;
    activeIdx = (activeIdx + 1) % filtered.length;
    render();
    e.preventDefault();
    return;
  }
  if (e.key === 'ArrowUp') {
    if (filtered.length === 0) return;
    activeIdx = (activeIdx - 1 + filtered.length) % filtered.length;
    render();
    e.preventDefault();
    return;
  }
  if (e.key === 'Enter') {
    if (filtered[activeIdx]) {
      useEntry(filtered[activeIdx].text);
    }
    e.preventDefault();
  }
});

const win = getCurrentWindow();
win.onFocusChanged(({ payload: focused }) => {
  // Auto-dismiss on click-away — same UX as the right-click menu and
  // spotlight. Without this the panel sticks around blocking clicks.
  if (!focused) invoke('hide_clipboard').catch(() => {});
});

// Refresh on every show so the user always sees up-to-the-second history.
listen('clipboard:show', () => {
  filterInput.value = '';
  activeIdx = 0;
  refresh().then(() => filterInput.focus());
});

// First paint — covers the case where Tauri's show() races our listener
// registration on the very first invocation of a session.
refresh();
