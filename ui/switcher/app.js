import * as THREE from './three.module.min.js';
import { ringTransform, ringRadius } from './ring.js';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const canvas = document.getElementById('ring');
const scrim = document.getElementById('scrim');
const labelEl = document.getElementById('label');
const labelTitle = document.getElementById('label-title');

// ── Scene ───────────────────────────────────────────────────────────────
const renderer = new THREE.WebGLRenderer({ canvas, alpha: true, antialias: true });
renderer.setClearColor(0x000000, 0); // transparent — the CSS scrim shows through
renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));

const scene = new THREE.Scene();
// Depth fog: distant cards recede + darken toward the back of the ring.
scene.fog = new THREE.Fog(0x05070c, 9, 22);

const CAMERA_DIST = 6.5; // fixed gap from camera to the front card
const CAMERA_Y = 1.2;    // slight overhead tilt so reflections read
const camera = new THREE.PerspectiveCamera(45, 1, 0.1, 200);
camera.position.set(0, CAMERA_Y, 11);
camera.lookAt(0, 0, 0);

function resize() {
  const w = window.innerWidth, h = window.innerHeight;
  renderer.setSize(w, h, false);
  camera.aspect = w / h;
  camera.updateProjectionMatrix();
}
window.addEventListener('resize', resize);
resize();

// ── Cards ───────────────────────────────────────────────────────────────
const CARD_W = 3.2, CARD_H = 2.0;
const loader = new THREE.TextureLoader();

let cards = [];   // [{ mesh, reflection, id, item }]
let selected = 0;
let radius = ringRadius(1);

// Dark rounded placeholder until an icon/thumb arrives.
const PLACEHOLDER =
  'data:image/svg+xml;utf8,' +
  encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="320" height="200"><rect width="100%" height="100%" rx="16" fill="%231c2230"/></svg>');

function makeCard(item) {
  const geo = new THREE.PlaneGeometry(CARD_W, CARD_H);
  const tex = loader.load(PLACEHOLDER);
  const mat = new THREE.MeshBasicMaterial({ map: tex, transparent: true, toneMapped: false });
  const mesh = new THREE.Mesh(geo, mat);
  scene.add(mesh);

  // Reflection: same texture, mirrored on Y, faded, just below the card.
  const refMat = new THREE.MeshBasicMaterial({
    map: tex, transparent: true, opacity: 0.0, toneMapped: false, depthWrite: false,
  });
  const reflection = new THREE.Mesh(geo, refMat);
  reflection.scale.y = -1;
  scene.add(reflection);

  // textureRank: 0=placeholder, 1=icon, 2=thumbnail. A lower rank must never
  // overwrite a higher one, so a late-resolving icon can't clobber a thumb.
  return { mesh, reflection, id: item.id, item, textureRank: 0 };
}

function clearCards() {
  for (const c of cards) {
    scene.remove(c.mesh);
    scene.remove(c.reflection);
    c.mesh.geometry.dispose();
    c.mesh.material.map?.dispose();
    c.mesh.material.dispose();
    c.reflection.material.dispose();
  }
  cards = [];
}

function placeReflection(c, s) {
  const gap = 0.06;
  c.reflection.position.set(c.mesh.position.x, -(CARD_H * s) - gap, c.mesh.position.z);
  c.reflection.rotation.y = c.mesh.rotation.y;
  c.reflection.scale.set(s, -s, s);
  c.reflection.renderOrder = c.mesh.renderOrder - 1;
}

// Snap every card straight to its target (used on open — no spin-in).
function snapLayout() {
  const total = cards.length || 1;
  cards.forEach((c, i) => {
    const t = ringTransform(i, selected, total, { radius });
    c.mesh.position.set(t.x, 0, t.z);
    c.mesh.rotation.y = t.rotY;
    c.mesh.scale.setScalar(t.scale);
    c.mesh.material.opacity = t.opacity;
    c.mesh.material.color.setScalar(1 - t.blur * 0.45);
    c.mesh.renderOrder = Math.round(t.z * 100);
    placeReflection(c, t.scale);
    c.reflection.material.color.setScalar(1 - t.blur * 0.45);
    c.reflection.material.opacity = t.opacity * 0.22;
  });
}

function updateLabel() {
  const c = cards[selected];
  if (c) {
    labelTitle.textContent = c.item.title || '';
    labelEl.classList.add('show');
  }
}

// Position camera + fog so the front card stays a constant apparent size
// regardless of how many windows are on the ring.
function frameRing() {
  camera.position.set(0, CAMERA_Y, radius + CAMERA_DIST);
  camera.lookAt(0, 0, 0);
  scene.fog.near = CAMERA_DIST + radius * 0.7;
  scene.fog.far = CAMERA_DIST + radius * 2.0;
}

window.__switcherApply = (payload) => {
  if (!payload || !Array.isArray(payload.windows)) return;
  clearCards();
  cards = payload.windows.map(makeCard);
  selected = Math.min(payload.selected ?? 0, Math.max(0, cards.length - 1));
  radius = ringRadius(cards.length || 1);
  frameRing();
  snapLayout();
  updateLabel();
  scrim.classList.add('show');
  startLoop();
  // Immediate texture = app icon (sharp window thumbs replace these later).
  cards.forEach((c) => applyIcon(c));
};

async function applyIcon(card) {
  try {
    const url = await invoke('get_icon', { exePath: card.item.exe_path, hwnd: card.id });
    if (url) setTexture(card, url, /*isIcon*/ true);
  } catch { /* keep placeholder */ }
}

function assignTexture(card, tex) {
  card.mesh.material.map?.dispose();
  card.mesh.material.map = tex;
  card.mesh.material.needsUpdate = true;
  card.reflection.material.map = tex;
  card.reflection.material.needsUpdate = true;
}

function roundRect(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

// Card texture canvas — matches the plane aspect (CARD_W:CARD_H = 1.6:1) so
// images map without distortion. Rounded corners come from clipping the canvas
// to a rounded rect (the corners stay transparent and the scrim shows through).
const CARD_PX_W = 512, CARD_PX_H = 320, CARD_PX_R = 34;

function setTexture(card, url, isIcon) {
  const rank = isIcon ? 1 : 2;
  if (rank < card.textureRank) return; // don't start a downgrade
  const apply = (tex) => {
    // Re-check at completion: a thumbnail may have landed while we decoded.
    if (rank < card.textureRank) { if (tex.dispose) tex.dispose(); return; }
    tex.colorSpace = THREE.SRGBColorSpace;
    assignTexture(card, tex);
    card.textureRank = rank;
  };
  const img = new Image();
  img.onload = () => {
    const cv = document.createElement('canvas');
    cv.width = CARD_PX_W; cv.height = CARD_PX_H;
    const ctx = cv.getContext('2d');
    if (isIcon) {
      // App-icon fallback: icon centered on a dark rounded card.
      roundRect(ctx, 0, 0, CARD_PX_W, CARD_PX_H, CARD_PX_R);
      ctx.fillStyle = '#1c2230';
      ctx.fill();
      const s = 150;
      ctx.drawImage(img, (CARD_PX_W - s) / 2, (CARD_PX_H - s) / 2, s, s);
    } else {
      // Window snapshot: dark rounded card, whole window contain-fit (no crop),
      // clipped to the rounded rect so corners are clean.
      roundRect(ctx, 0, 0, CARD_PX_W, CARD_PX_H, CARD_PX_R);
      ctx.fillStyle = '#0e1118';
      ctx.fill();
      ctx.save();
      roundRect(ctx, 0, 0, CARD_PX_W, CARD_PX_H, CARD_PX_R);
      ctx.clip();
      const ir = img.width / img.height;
      const cr = CARD_PX_W / CARD_PX_H;
      let dw, dh;
      if (ir > cr) { dw = CARD_PX_W; dh = CARD_PX_W / ir; }
      else { dh = CARD_PX_H; dw = CARD_PX_H * ir; }
      ctx.drawImage(img, (CARD_PX_W - dw) / 2, (CARD_PX_H - dh) / 2, dw, dh);
      ctx.restore();
    }
    apply(new THREE.CanvasTexture(cv));
  };
  img.src = url;
}

// ── Render loop (eases every card toward its live target each frame) ──────
let rafRunning = false;
function startLoop() {
  if (rafRunning) return;
  rafRunning = true;
  const k = 0.22; // ease factor — snappy but smooth
  const tick = () => {
    if (!rafRunning) return;
    const total = cards.length || 1;
    cards.forEach((c, i) => {
      const t = ringTransform(i, selected, total, { radius });
      c.mesh.position.x += (t.x - c.mesh.position.x) * k;
      c.mesh.position.z += (t.z - c.mesh.position.z) * k;
      c.mesh.rotation.y += (t.rotY - c.mesh.rotation.y) * k;
      const s = c.mesh.scale.x + (t.scale - c.mesh.scale.x) * k;
      c.mesh.scale.setScalar(s);
      c.mesh.material.opacity += (t.opacity - c.mesh.material.opacity) * k;
      const shade = 1 - t.blur * 0.45;
      c.mesh.material.color.setScalar(shade);
      c.mesh.renderOrder = Math.round(c.mesh.position.z * 100);
      placeReflection(c, s);
      c.reflection.material.color.setScalar(shade);
      c.reflection.material.opacity += (t.opacity * 0.22 - c.reflection.material.opacity) * k;
    });
    renderer.render(scene, camera);
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}
function stopLoop() { rafRunning = false; }

// Called by Rust (event + eval) when the overlay hides — SW_HIDE doesn't
// reliably fire visibilitychange, so stop the loop + clear visuals here.
window.__switcherClose = () => {
  stopLoop();
  scrim.classList.remove('show');
  labelEl.classList.remove('show');
};

// ── Mouse: hover-to-select, scroll-to-spin, click-to-commit ───────────────
const raycaster = new THREE.Raycaster();
const pointer = new THREE.Vector2();

function pickIndex(ev) {
  pointer.x = (ev.clientX / window.innerWidth) * 2 - 1;
  pointer.y = -(ev.clientY / window.innerHeight) * 2 + 1;
  raycaster.setFromCamera(pointer, camera);
  const hits = raycaster.intersectObjects(cards.map((c) => c.mesh), false);
  if (!hits.length) return -1;
  return cards.findIndex((c) => c.mesh === hits[0].object);
}

// Mouse only requests a selection change; Rust echoes switcher:select back and
// THAT updates `selected` (single source of truth — no local mutation here).
window.addEventListener('mousemove', (ev) => {
  if (!cards.length) return;
  const i = pickIndex(ev);
  if (i >= 0 && i !== selected) {
    invoke('switcher_set_index', { index: i }).catch(() => {});
  }
});

window.addEventListener('wheel', (ev) => {
  if (!cards.length) return;
  const dir = ev.deltaY > 0 ? 1 : -1;
  const next = (selected + dir + cards.length) % cards.length;
  invoke('switcher_set_index', { index: next }).catch(() => {});
}, { passive: true });

window.addEventListener('click', (ev) => {
  const i = pickIndex(ev);
  if (i >= 0) invoke('switcher_commit', { index: i }).catch(() => {});
});

// ── Tauri wiring ──────────────────────────────────────────────────────────
async function init() {
  await listen('switcher:open', (e) => window.__switcherApply(e.payload));
  await listen('switcher:select', (e) => {
    selected = e.payload | 0;
    updateLabel();
  });
  await listen('switcher:thumb', (e) => {
    const { id, thumb } = e.payload || {};
    const card = cards.find((c) => c.id === id);
    if (card && thumb) setTexture(card, thumb, /*isIcon*/ false);
  });
  await listen('switcher:close', () => window.__switcherClose());
  // Backup path: stop the loop if the webview ever does report hidden.
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) window.__switcherClose();
  });
  // Apply any payload that arrived (via eval) before listeners registered.
  if (window.__switcherPending) window.__switcherApply(window.__switcherPending);
}
init();
