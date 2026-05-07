const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const itemsRoot = document.getElementById('items');
const filterInput = document.getElementById('filter');
const clearBtn = document.getElementById('clear');

let entries = [];
let filtered = [];
let activeIdx = 0;

// Tiny log helper — every panel-side event lands in debug.log via the
// dbg_log Tauri command. This was the missing piece for diagnosing the
// long-running 'panel opens but is empty' bug: previously we only saw
// Rust-side events, so we couldn't distinguish 'JS listener never fired'
// from 'JS fired but invoke errored'.
function logj(msg) {
  invoke('dbg_log', { message: `clipboard ${msg}` }).catch(() => {});
}

async function refresh() {
  try {
    entries = await invoke('clipboard_history');
    logj(`refresh ok, got ${entries.length} entries`);
  } catch (e) {
    entries = [];
    logj(`refresh FAILED: ${e}`);
  }
  applyFilter();
}

function applyFilter() {
  const q = filterInput.value.trim().toLowerCase();
  filtered = q
    ? entries.filter(e => entryMatchesQuery(e, q))
    : entries.slice();
  if (activeIdx >= filtered.length) activeIdx = 0;
  render();
}

// Text entries match on substring; image entries match on the literal
// word "image" (or its localised equivalent if we ever add that).
function entryMatchesQuery(entry, q) {
  if (entry.item.kind === 'text') {
    return entry.item.text.toLowerCase().includes(q);
  }
  if (entry.item.kind === 'image') {
    return 'image'.includes(q) || `${entry.item.width}x${entry.item.height}`.includes(q);
  }
  return false;
}

function render() {
  itemsRoot.innerHTML = '';
  if (filtered.length === 0) {
    const empty = document.createElement('div');
    empty.className = 'clip-empty';
    empty.textContent = entries.length === 0
      ? 'Copy something to see it here. Text and images both work.'
      : 'No matches.';
    itemsRoot.appendChild(empty);
    return;
  }
  filtered.forEach((entry, idx) => {
    const li = document.createElement('li');
    li.className = 'clip-row';
    if (entry.item.kind === 'image') li.classList.add('image');
    if (idx === activeIdx) li.classList.add('active');

    if (entry.item.kind === 'image') {
      renderImageRow(li, entry);
    } else {
      renderTextRow(li, entry);
    }

    li.addEventListener('click', () => useEntry(entry.id));
    itemsRoot.appendChild(li);
  });
  // Keep the active row in view when navigating via keyboard.
  const activeRow = itemsRoot.children[activeIdx];
  if (activeRow && activeRow.scrollIntoView) {
    activeRow.scrollIntoView({ block: 'nearest' });
  }
}

function renderTextRow(li, entry) {
  const text = document.createElement('div');
  text.className = 'clip-row-text';
  text.textContent = entry.item.text;
  const meta = document.createElement('div');
  meta.className = 'clip-row-meta';
  const lines = entry.item.text.split('\n').length;
  meta.textContent =
    `${formatAge(entry.age_secs)} · ${entry.item.text.length} ch${lines > 1 ? ` · ${lines} lines` : ''}`;
  li.appendChild(text);
  li.appendChild(meta);
}

function renderImageRow(li, entry) {
  const wrap = document.createElement('div');
  wrap.className = 'clip-row-image-wrap';

  if (entry.item.data_url) {
    const img = document.createElement('img');
    img.src = entry.item.data_url;
    img.alt = `${entry.item.width}×${entry.item.height} image`;
    img.className = 'clip-row-image';
    wrap.appendChild(img);
  } else {
    // Image too big to inline — show a placeholder so the entry is still
    // selectable. Paste-back still works because the full PNG lives on
    // the Rust side.
    const ph = document.createElement('div');
    ph.className = 'clip-row-image-placeholder';
    ph.textContent = '🖼';
    wrap.appendChild(ph);
  }

  const meta = document.createElement('div');
  meta.className = 'clip-row-meta';
  meta.textContent =
    `${formatAge(entry.age_secs)} · ${entry.item.width}×${entry.item.height} · ${formatBytes(entry.item.byte_size)}`;
  li.appendChild(wrap);
  li.appendChild(meta);
}

function formatAge(secs) {
  if (secs < 5)   return 'just now';
  if (secs < 60)  return `${secs}s ago`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
  return `${Math.floor(secs / 86400)}d ago`;
}

function formatBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(2)} MB`;
}

async function useEntry(id) {
  try {
    await invoke('clipboard_use_entry', { id });
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
      useEntry(filtered[activeIdx].id);
    }
    e.preventDefault();
  }
});

const win = getCurrentWindow();
logj('panel JS module loaded, attaching listeners');
win.onFocusChanged(({ payload: focused }) => {
  logj(`focus changed: focused=${focused}`);
  if (focused) {
    // Refresh on every gain-of-focus — Tauri 2's emit_to/listen pairing
    // was unreliably dropping our 'clipboard:show' event, leaving the
    // panel stuck on whatever state the previous render produced.
    // Focus-gain is a backstop the OS guarantees fires whenever the
    // window becomes interactive, so it works even when the named event
    // doesn't.
    filterInput.value = '';
    activeIdx = 0;
    refresh().then(() => filterInput.focus());
  } else {
    // Auto-dismiss on click-away — same UX as the right-click menu and
    // spotlight. Without this the panel sticks around blocking clicks.
    invoke('hide_clipboard').catch(() => {});
  }
});

// Keep the named event listener too — when it does arrive it's the
// fastest signal (no need to wait for the focus event).
listen('clipboard:show', () => {
  logj('clipboard:show event received');
  filterInput.value = '';
  activeIdx = 0;
  refresh().then(() => filterInput.focus());
});

// First paint — covers the case where Tauri's show() races our listener
// registration on the very first invocation of a session.
refresh();
