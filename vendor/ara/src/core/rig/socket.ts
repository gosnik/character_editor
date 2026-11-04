/**
 * Weapon socket: a bone ("WeaponR") parented to the right hand, sitting in the
 * centre of a closed grip. Weapons are modelled in this frame, so an engine can
 * attach any of our weapon props to it with zero offset.
 *
 * Socket frame (from the hand's own joints, so it fits every body):
 *   +Y  out of the top of the fist (index/thumb side): grip axis, blades point this way
 *   +Z  toward the knuckles: blade edge / barrel direction
 *   +X  = Y x Z
 */
import type { Vec3 } from '../math';
import type { Mat3, RigBone } from './rig';

const sub = (a: Vec3, b: Vec3): Vec3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const norm = (v: Vec3): Vec3 => {
  const l = Math.hypot(...v) || 1;
  return [v[0] / l, v[1] / l, v[2] / l];
};
const cross = (a: Vec3, b: Vec3): Vec3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const dot = (a: Vec3, b: Vec3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];

export interface HandJoints {
  wrist: Vec3;
  index: Vec3; // index finger base
  middle: Vec3;
  pinky: Vec3;
  thumbTip: Vec3;
}

export const SOCKET_NAME = 'WeaponR';

export function socketBone(h: HandJoints, parent: number, scale: number): RigBone {
  // Palm normal, oriented to the thumb side (the palm).
  let n = norm(cross(sub(h.index, h.wrist), sub(h.pinky, h.wrist)));
  if (dot(sub(h.thumbTip, h.wrist), n) < 0) n = [-n[0], -n[1], -n[2]];
  const Y = norm(sub(h.index, h.pinky));
  const f = norm(sub(h.middle, h.wrist));
  const Z = norm(sub(f, Y.map((y) => y * dot(f, Y)) as Vec3));
  const X = cross(Y, Z);
  // Centre of the closed fist: between the knuckles, in front of the palm, a little toward the wrist.
  const mid: Vec3 = [(h.index[0] + h.pinky[0]) / 2, (h.index[1] + h.pinky[1]) / 2, (h.index[2] + h.pinky[2]) / 2];
  const head: Vec3 = [0, 1, 2].map((d) => mid[d] + n[d] * 0.024 * scale - f[d] * 0.012 * scale) as Vec3;
  const tail: Vec3 = [0, 1, 2].map((d) => head[d] + Y[d] * 0.06 * scale) as Vec3;
  return { name: SOCKET_NAME, parent, head, tail, rot: [...X, ...Y, ...Z] as Mat3 };
}
