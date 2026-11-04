/** Minimal quaternion helpers, [x, y, z, w]. */
import type { Vec3 } from '../math';

export type Q = [number, number, number, number];

export const QI: Q = [0, 0, 0, 1];

export function qmul(a: Q, b: Q): Q {
  return [
    a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
    a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
    a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
    a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
  ];
}

export const qconj = (q: Q): Q => [-q[0], -q[1], -q[2], q[3]];

export function qaxis(axis: Vec3, deg: number): Q {
  const l = Math.hypot(...axis) || 1;
  const h = (deg * Math.PI) / 360, s = Math.sin(h) / l;
  return [axis[0] * s, axis[1] * s, axis[2] * s, Math.cos(h)];
}

export function qrot(q: Q, v: Vec3): Vec3 {
  const p = qmul(qmul(q, [v[0], v[1], v[2], 0]), qconj(q));
  return [p[0], p[1], p[2]];
}

/** Shortest rotation taking direction a onto direction b. */
export function qfromTo(a: Vec3, b: Vec3): Q {
  const na = Math.hypot(...a), nb = Math.hypot(...b);
  const u: Vec3 = [a[0] / na, a[1] / na, a[2] / na], w: Vec3 = [b[0] / nb, b[1] / nb, b[2] / nb];
  const d = u[0] * w[0] + u[1] * w[1] + u[2] * w[2];
  if (d < -0.99999) return qaxis(Math.abs(u[0]) < 0.9 ? [0, -u[2], u[1]] : [-u[1], u[0], 0], 180);
  const c: Vec3 = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
  const q: Q = [c[0], c[1], c[2], 1 + d];
  const l = Math.hypot(...q);
  return [q[0] / l, q[1] / l, q[2] / l, q[3] / l];
}

/** Euler XYZ (degrees, R = Rz * Ry * Rx) from a quaternion, FBX's default order. */
export function qToEulerXYZ(q: Q): Vec3 {
  const [x, y, z, w] = q;
  const r00 = 1 - 2 * (y * y + z * z), r10 = 2 * (x * y + w * z), r20 = 2 * (x * z - w * y);
  const r21 = 2 * (y * z + w * x), r22 = 1 - 2 * (x * x + y * y);
  const r11 = 1 - 2 * (x * x + z * z), r12 = 2 * (y * z - w * x);
  const deg = 180 / Math.PI;
  if (Math.abs(r20) < 0.99999) return [Math.atan2(r21, r22) * deg, Math.asin(-r20) * deg, Math.atan2(r10, r00) * deg];
  return [Math.atan2(-r12, r11) * deg, r20 < 0 ? 90 : -90, 0];
}

/** Spherical interpolation between unit quaternions (shortest way), renormalised. */
export function qslerp(a: Q, b: Q, t: number): Q {
  let d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
  const s = d < 0 ? -1 : 1;
  d *= s;
  if (d > 0.9995) {
    const r: Q = [a[0] + (s * b[0] - a[0]) * t, a[1] + (s * b[1] - a[1]) * t, a[2] + (s * b[2] - a[2]) * t, a[3] + (s * b[3] - a[3]) * t];
    const l = Math.hypot(...r);
    return [r[0] / l, r[1] / l, r[2] / l, r[3] / l];
  }
  const th = Math.acos(d), sn = Math.sin(th);
  const ka = Math.sin((1 - t) * th) / sn, kb = (Math.sin(t * th) / sn) * s;
  const r: Q = [a[0] * ka + b[0] * kb, a[1] * ka + b[1] * kb, a[2] * ka + b[2] * kb, a[3] * ka + b[3] * kb];
  const l = Math.hypot(...r);
  return [r[0] / l, r[1] / l, r[2] / l, r[3] / l];
}
