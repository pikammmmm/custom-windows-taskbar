const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const q = document.getElementById('q');
const list = document.getElementById('results');

let results = [];
let selected = 0;

async function refresh() {
  try {
    results = await invoke('search_apps', { query: q.value });
  } catch {
    results = [];
  }
  selected = 0;
  render();
}

function render() {
  list.innerHTML = '';
  if (!results.length) {
    const empty = document.createElement('div');
    empty.className = 'spotlight-empty';
    empty.textContent = q.value.trim()
      ? 'No matches. Try a different query.'
      : 'Indexing apps…';
    list.appendChild(empty);
    return;
  }
  results.forEach((r, i) => {
    const li = document.createElement('li');
    li.className = 'spotlight-row' + (i === selected ? ' selected' : '');
    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = r.name;
    const path = document.createElement('span');
    path.className = 'path';
    path.textContent = r.path;
    li.appendChild(name);
    li.appendChild(path);
    li.addEventListener('click', () => launch(i));
    list.appendChild(li);
  });
  // Keep the selected row visible.
  const sel = list.querySelector('.selected');
  if (sel) sel.scrollIntoView({ block: 'nearest' });
}

function move(delta) {
  if (!results.length) return;
  selected = Math.max(0, Math.min(results.length - 1, selected + delta));
  render();
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

// Re-focus + reset on every show so the user can start typing immediately.
listen('spotlight:show', () => {
  q.value = '';
  q.focus();
  refresh();
});

// Auto-dismiss on focus loss (clicked outside).
const win = getCurrentWindow();
win.onFocusChanged(({ payload: focused }) => {
  if (!focused) hide();
});

// Initial paint so first show isn't blank.
refresh();
q.focus();
