import assert from 'node:assert';
import { ringTransform, ringRadius } from './ring.js';

// Selected card sits front-and-center, facing camera (angle 0).
{
  const t = ringTransform(2, 2, 6, { radius: 5 });
  assert.ok(Math.abs(t.angle) < 1e-9, `selected angle should be 0, got ${t.angle}`);
  assert.ok(Math.abs(t.z - 5) < 1e-9, `selected should be at +radius z, got ${t.z}`);
  assert.ok(t.scale > 1.0, 'selected scale should be emphasized');
  assert.ok(Math.abs(t.opacity - 1) < 1e-9, 'selected fully opaque');
}

// Neighbours are offset by one angular step each side.
{
  const total = 8;
  const step = (2 * Math.PI) / total;
  const left = ringTransform(1, 2, total, { radius: 5 });
  const right = ringTransform(3, 2, total, { radius: 5 });
  assert.ok(Math.abs(left.angle - (-step)) < 1e-9, `left neighbour angle ${left.angle}`);
  assert.ok(Math.abs(right.angle - step) < 1e-9, `right neighbour angle ${right.angle}`);
  assert.ok(left.scale < 1.0 && right.scale < 1.0, 'neighbours smaller than selected');
  assert.ok(left.opacity < 1 && right.opacity < 1, 'neighbours dimmer');
}

// Wrap: with total=6, index 5 relative to selected 0 is one step left (-step), not +5 steps.
{
  const total = 6;
  const step = (2 * Math.PI) / total;
  const t = ringTransform(5, 0, total, { radius: 5 });
  assert.ok(Math.abs(t.angle - (-step)) < 1e-9, `wrapped angle should be -step, got ${t.angle}`);
}

// Radius scales with count so big lists don't overlap.
assert.ok(ringRadius(12) > ringRadius(4), 'radius grows with window count');

console.log('ring.test.mjs: all assertions passed');
