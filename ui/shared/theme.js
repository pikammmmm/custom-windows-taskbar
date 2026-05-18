// Shared theme applier — pulls the persisted theme from the backend on
// load, sets the CSS variables defined in shared/glass.css, and listens
// for `theme:changed` events so a tweak in the HUD's theme picker
// repaints every panel (dock, HUD, clipboard, spotlight, menu) live
// without a glassbar restart.
//
// Every panel's index.html includes this script BEFORE its own app.js so
// the variables are set before the first paint that needs them.

(function () {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  function hexToRgb(hex) {
    const m = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex || '');
    if (!m) return { r: 92, g: 182, b: 255 };
    return {
      r: parseInt(m[1], 16),
      g: parseInt(m[2], 16),
      b: parseInt(m[3], 16),
    };
  }

  function apply(theme) {
    if (!theme) return;
    const rgb = hexToRgb(theme.accent);
    const root = document.documentElement.style;
    root.setProperty('--accent', theme.accent);
    root.setProperty('--accent-rgb', `${rgb.r}, ${rgb.g}, ${rgb.b}`);
    root.setProperty('--glass-hue', `${theme.glass_hue || 0}deg`);
    root.setProperty('--glass-opacity', String(
      Number.isFinite(theme.glass_opacity) ? theme.glass_opacity : 1
    ));
  }

  // Apply persisted theme as early as possible — before any DOM that
  // depends on accent paints. invoke() resolves on a microtask, so this
  // still beats the first user interaction.
  invoke('get_theme').then(apply).catch(() => {});

  // Live sync: HUD's theme picker calls set_theme which emits this.
  listen('theme:changed', (e) => apply(e.payload));
})();
