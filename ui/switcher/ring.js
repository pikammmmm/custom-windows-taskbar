// Pure ring-layout math. Importable from the browser (<script type=module>)
// and from node (tests). No DOM, no Three.js dependency.

// Shortest signed angular offset (in steps) from `selected` to `i`, wrapped
// to [-total/2, total/2] so the ring takes the short way round.
function signedOffset(i, selected, total) {
  let d = i - selected;
  const half = total / 2;
  if (d > half) d -= total;
  if (d < -half) d += total;
  return d;
}

// Ring radius grows with the number of cards so a crowded ring doesn't overlap.
export function ringRadius(total) {
  return Math.max(4.5, 0.85 * total);
}

// Transform for card `i` given the currently `selected` index out of `total`.
// Returns { angle, x, z, rotY, scale, opacity, blur } in scene units.
// angle 0 == front-center (largest z, facing camera).
export function ringTransform(i, selected, total, opts = {}) {
  const radius = opts.radius ?? ringRadius(total);
  const step = (2 * Math.PI) / total;
  const off = signedOffset(i, selected, total);
  const angle = off * step;

  // Position on a circle in the XZ plane; front of ring is +z toward camera.
  const x = Math.sin(angle) * radius;
  const z = Math.cos(angle) * radius;

  // Cards face outward along the ring tangent; the front one faces the camera.
  const rotY = -angle;

  // Emphasis falls off with angular distance from the front.
  const a = Math.abs(angle);
  const selected_scale = 1.35;
  const min_scale = 0.7;
  const scale = i === selected ? selected_scale : Math.max(min_scale, 1.0 - a * 0.18);
  const opacity = i === selected ? 1.0 : Math.max(0.25, 1.0 - a * 0.35);
  const blur = i === selected ? 0 : Math.min(1, a * 0.4);

  return { angle, x, z, rotY, scale, opacity, blur };
}
