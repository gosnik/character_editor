/** Shared helpers for clip definitions: curves, left/right mirroring, finger poses, vectors. */
import { type Ch } from './types';

// --- Curve helpers -----------------------------------------------------------------

/** Periodic Catmull-Rom through evenly spaced keys over phase 0..1. */
export function cyc(keys: number[]): (p: number) => number {
  const n = keys.length;
  return (p) => {
    const x = (((p % 1) + 1) % 1) * n;
    const i = Math.floor(x), f = x - i;
    const k = (j: number) => keys[(((i + j) % n) + n) % n];
    const p0 = k(-1), p1 = k(0), p2 = k(1), p3 = k(2);
    return 0.5 * (2 * p1 + (-p0 + p2) * f + (2 * p0 - 5 * p1 + 4 * p2 - p3) * f * f + (-p0 + 3 * p1 - 3 * p2 + p3) * f * f * f);
  };
}

/** Smooth (eased) interpolation through timed keys, clamped at the ends. */
export function track(times: number[], values: number[]): (t: number) => number {
  return (t) => {
    if (t <= times[0]) return values[0];
    const n = times.length;
    if (t >= times[n - 1]) return values[n - 1];
    let i = 0;
    while (t > times[i + 1]) i++;
    const f = (t - times[i]) / (times[i + 1] - times[i]);
    const s = f * f * (3 - 2 * f);
    return values[i] + (values[i + 1] - values[i]) * s;
  };
}

export const smoother = (x: number) => {
  const t = Math.max(0, Math.min(1, x));
  return t * t * t * (t * (t * 6 - 15) + 10);
};

/** Left/right channels: right is the mirror (twist and side negated). */
export function side(out: Record<string, Ch>, bone: string, left: Ch, right: Ch) {
  out[`Left${bone}`] = left;
  out[`Right${bone}`] = [right[0], -right[1], -right[2]];
}

/** Relaxed, slightly curled fingers (game rig). */
export function fingers(out: Record<string, Ch>, curl = 12) {
  for (const f of ['Index', 'Middle', 'Ring', 'Pinky'])
    for (let k = 1; k <= 3; k++) side(out, `Hand${f}${k}`, [curl * (k === 1 ? 0.8 : 1.1), 0, 0], [curl * (k === 1 ? 0.8 : 1.1), 0, 0]);
  side(out, 'HandThumb2', [8, 0, 0], [8, 0, 0]);
}

/**
 * Closed fists: fingers curl into the palm; the thumb is wrapped across the
 * front of the index/middle fingers by the evaluator (sample.thumbs).
 */
export function fist(out: Record<string, Ch>, sides: ('Left' | 'Right')[] = ['Left', 'Right']) {
  for (const s of sides) {
    for (const f of ['Index', 'Middle', 'Ring', 'Pinky']) {
      out[`${s}Hand${f}1`] = [80, 0, 0];
      out[`${s}Hand${f}2`] = [100, 0, 0];
      out[`${s}Hand${f}3`] = [65, 0, 0];
    }
    out[`${s}HandThumb2`] = [15, 0, 0];
    out[`${s}HandThumb3`] = [25, 0, 0];
  }
}
export const FISTS = { Left: 1, Right: 1 };

/** Grip on a rifle: trigger finger along the guard (right), wrapped fingers. */
export function grip(out: Record<string, Ch>, side: 'Left' | 'Right', trigger = false, curl = 1) {
  for (const f of ['Index', 'Middle', 'Ring', 'Pinky']) {
    const t = trigger && f === 'Index';
    out[`${side}Hand${f}1`] = [(t ? 20 : 60) * curl, 0, 0];
    out[`${side}Hand${f}2`] = [(t ? 25 : 80) * curl, 0, 0];
    out[`${side}Hand${f}3`] = [(t ? 15 : 50) * curl, 0, 0];
  }
  out[`${side}HandThumb2`] = [30 * curl, 0, 0];
  out[`${side}HandThumb3`] = [20 * curl, 0, 0];
}

// --- Vectors -------------------------------------------------------------------------------

export const v = (x: number, y: number, z: number): [number, number, number] => [x, y, z];
export const lerp3 = (a: [number, number, number], b: [number, number, number], t: number) => v(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t);
export const crossV = (a: [number, number, number], b: [number, number, number]) =>
  v(a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]);
export const normV = (a: [number, number, number]) => {
  const l = Math.hypot(...a) || 1;
  return v(a[0] / l, a[1] / l, a[2] / l);
};
