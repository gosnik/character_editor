/**
 * Tattoo designs, defined on the unmorphed base body (decimetres) and painted
 * into the skin overlay (see overlay.ts). Body art is adult-only (18+).
 */
import { noise3, REGION, type Body } from '../garments/body';
import { rgb } from '../garments/textures';
import type { Vec3 } from '../math';
import type { Layer, Paint } from './overlay';

export const TATTOOS = ['armband_l', 'sleeve_r', 'forearm_l', 'chest', 'back', 'face_marks'] as const;
export type TattooId = (typeof TATTOOS)[number];
export const TATTOO_LABELS: Record<TattooId, string> = {
  armband_l: 'Tribal armband (left arm)',
  sleeve_r: 'Full sleeve (right arm)',
  forearm_l: 'Forearm bands (left)',
  chest: 'Chest piece',
  back: 'Back piece',
  face_marks: 'Face marks (Yoruba-style cheek lines)',
};

const clamp01 = (x: number) => (x < 0 ? 0 : x > 1 ? 1 : x);
const frac = (x: number) => x - Math.floor(x);

// --- Strokes: tapered curves in a 2D design space ---------------------------------------

interface Stroke {
  pts: [number, number, number][]; // x, y, width
  box: [number, number, number, number];
}

/** Smooth tapered stroke through control points (Catmull-Rom), width w0 -> w1. */
function stroke(ctrl: [number, number][], w0: number, w1 = 0): Stroke {
  const P = [ctrl[0], ...ctrl, ctrl[ctrl.length - 1]];
  const pts: [number, number, number][] = [];
  const segs = ctrl.length - 1, per = 8;
  for (let s = 0; s < segs; s++)
    for (let k = 0; k < per + (s === segs - 1 ? 1 : 0); k++) {
      const t = k / per;
      const [p0, p1, p2, p3] = [P[s], P[s + 1], P[s + 2], P[s + 3]];
      const cr = (i: 0 | 1) =>
        0.5 * (2 * p1[i] + (-p0[i] + p2[i]) * t + (2 * p0[i] - 5 * p1[i] + 4 * p2[i] - p3[i]) * t * t + (-p0[i] + 3 * p1[i] - 3 * p2[i] + p3[i]) * t * t * t);
      const f = (s + t) / segs;
      // Tribal blade: swells a little then tapers to a point.
      const w = (w0 + (w1 - w0) * f) * Math.min(1, 0.55 + f * 3);
      pts.push([cr(0), cr(1), w]);
    }
  const pad = Math.max(w0, w1);
  const xs = pts.map((p) => p[0]), ys = pts.map((p) => p[1]);
  return { pts, box: [Math.min(...xs) - pad, Math.min(...ys) - pad, Math.max(...xs) + pad, Math.max(...ys) + pad] };
}

/** Ink coverage (0..1, anti-aliased over `aa`) of a set of strokes at (x, y). */
function strokes(list: Stroke[], x: number, y: number, aa = 0.01): number {
  let best = 0;
  for (const s of list) {
    if (x < s.box[0] || x > s.box[2] || y < s.box[1] || y > s.box[3]) continue;
    for (let i = 0; i < s.pts.length - 1; i++) {
      const [ax, ay, aw] = s.pts[i], [bx, by, bw] = s.pts[i + 1];
      const dx = bx - ax, dy = by - ay;
      const L = dx * dx + dy * dy || 1e-9;
      const t = clamp01(((x - ax) * dx + (y - ay) * dy) / L);
      const d = Math.hypot(x - ax - dx * t, y - ay - dy * t);
      const w = aw + (bw - aw) * t;
      best = Math.max(best, clamp01((w - d) / aa + 0.5));
      if (best >= 1) return 1;
    }
  }
  return best;
}

const diamond = (x: number, y: number, cx: number, cy: number, rx: number, ry: number, aa = 0.01) =>
  clamp01(((1 - Math.abs(x - cx) / rx - Math.abs(y - cy) / ry) * Math.min(rx, ry)) / aa + 0.5);

// --- Limb coordinates ------------------------------------------------------------------------

function limbCoords(body: Body, p: Vec3, region: number, side: number): { t: number; theta: number } {
  const [a, b] = body.axis(region, side);
  const d: Vec3 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
  const len = Math.hypot(...d);
  const n: Vec3 = [d[0] / len, d[1] / len, d[2] / len];
  const rel: Vec3 = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
  const along = rel[0] * n[0] + rel[1] * n[1] + rel[2] * n[2];
  // Front reference (+Z) made perpendicular to the axis; theta 0 = front.
  const fz = n[2];
  const front: Vec3 = [-n[0] * fz, -n[1] * fz, 1 - n[2] * fz];
  const fl = Math.hypot(...front) || 1;
  const sv: Vec3 = [(front[1] * n[2] - front[2] * n[1]) / fl, (front[2] * n[0] - front[0] * n[2]) / fl, (front[0] * n[1] - front[1] * n[0]) / fl];
  const x = (rel[0] * front[0] + rel[1] * front[1] + rel[2] * front[2]) / fl;
  const y = -(rel[0] * sv[0] + rel[1] * sv[1] + rel[2] * sv[2]);
  return { t: along / len, theta: Math.atan2(y, x) };
}

// --- Designs -----------------------------------------------------------------------------------

const CHEST = [
  stroke([[0.12, 4.95], [0.45, 5.1], [0.85, 5.12], [1.2, 4.98], [1.38, 4.72]], 0.1),
  stroke([[0.22, 4.78], [0.55, 4.84], [0.9, 4.74], [1.12, 4.52]], 0.08),
  stroke([[0.5, 5.06], [0.58, 5.2], [0.56, 5.3]], 0.06),
  stroke([[0.12, 4.95], [0.14, 4.76], [0.26, 4.62]], 0.07),
];
const BACK = [
  stroke([[0.08, 5.42], [0.5, 5.58], [1.0, 5.48], [1.36, 5.18]], 0.11),
  stroke([[0.08, 5.02], [0.55, 5.16], [1.0, 4.98], [1.24, 4.6]], 0.1),
  stroke([[0.08, 4.62], [0.45, 4.66], [0.78, 4.42], [0.88, 4.1]], 0.09),
  stroke([[0, 5.7], [0, 4.7], [0, 3.8]], 0.05),
];
// Three short horizontal cheek lines (pele marks), slanting slightly down and out.
const FACE = [6.96, 6.9, 6.84].map((y) => stroke([[0.36, y], [0.44, y - 0.012], [0.52, y - 0.028]], 0.014, 0.01));

const grain = (p: Vec3) => 0.86 + 0.14 * noise3(p[0] * 40, p[1] * 40, p[2] * 40);

/** Overlay layers for the chosen tattoos. */
export function tattooLayers(body: Body, ids: TattooId[], ink: string): Layer[] {
  const [r, g, b] = rgb(ink);
  const paint = (cov: number, p: Vec3, opacity = 0.9): Paint => (cov > 0 ? [r, g, b, cov * opacity * grain(p)] : null);
  const tz = body.L.torsoZ;
  const layers: Layer[] = [];
  for (const id of ids)
    switch (id) {
      case 'armband_l':
        layers.push({
          key: `tattoo:${id}:${ink}`,
          bounds: [0, -99, -99, 99, 99, 99],
          regions: [REGION.upperarm],
          paint(p, region, side) {
            if (side < 0) return null;
            const { t, theta } = limbCoords(body, p, region, side);
            const lo = 0.42, hi = 0.58;
            if (t < lo - 0.02 || t > hi + 0.02) return null;
            const aa = 0.012;
            const border = Math.max(clamp01((0.022 - Math.abs(t - lo)) / aa + 0.5), clamp01((0.022 - Math.abs(t - hi)) / aa + 0.5));
            // Interlocking triangles between the borders.
            const f = frac((theta / (2 * Math.PI)) * 9 + 0.5);
            const h = 1 - Math.abs(2 * f - 1);
            const u = (t - lo) / (hi - lo);
            const tri = u > 0 && u < 1 ? clamp01(((h - u) * 0.6) / aa + 0.5) * (h > 0.08 ? 1 : 0) : 0;
            return paint(Math.max(border, tri), p);
          },
        });
        break;
      case 'forearm_l':
        layers.push({
          key: `tattoo:${id}:${ink}`,
          bounds: [0, -99, -99, 99, 99, 99],
          regions: [REGION.lowerarm],
          paint(p, region, side) {
            if (side < 0) return null;
            const { t } = limbCoords(body, p, region, side);
            const band = (c: number, w: number) => clamp01((w - Math.abs(t - c)) / 0.01 + 0.5);
            return paint(Math.max(band(0.5, 0.035), band(0.58, 0.012), band(0.42, 0.012)), p);
          },
        });
        break;
      case 'sleeve_r':
        layers.push({
          key: `tattoo:${id}:${ink}`,
          bounds: [-99, -99, -99, 0, 99, 99],
          regions: [REGION.upperarm, REGION.lowerarm],
          paint(p, region, side) {
            if (side > 0) return null;
            const { t } = limbCoords(body, p, region, side);
            const upper = region === REGION.upperarm;
            if (upper ? t < 0.1 : t > 0.86) return null;
            const edge = upper ? t - 0.1 : 0.86 - t;
            if (edge < 0.035) return paint(clamp01((0.02 - Math.abs(edge - 0.015)) / 0.006 + 0.5), p);
            // Flowing blackwork: contour lines and fills of a smooth 3D noise field (seamless across UV seams).
            const f = noise3(p[0] * 1.3, p[1] * 1.3, p[2] * 1.3) * 0.65 + noise3(p[0] * 2.9 + 7, p[1] * 2.9, p[2] * 2.9) * 0.35;
            const line = clamp01((0.13 - Math.abs(frac(f * 11) - 0.5)) / 0.04 + 0.5);
            const fill = Math.max(clamp01((f - 0.6) / 0.01), clamp01((0.36 - f) / 0.01));
            return paint(Math.max(line, fill), p);
          },
        });
        break;
      case 'chest':
        layers.push({
          key: `tattoo:${id}:${ink}`,
          bounds: [-1.8, 4.2, 0.3, 1.8, 5.6, 9],
          regions: [REGION.torso],
          paint(p) {
            if (Math.abs(Math.atan2(p[0], p[2] - tz)) > 1.25 || p[1] < 4.3) return null;
            const x = Math.abs(p[0]), y = p[1];
            return paint(Math.max(strokes(CHEST, x, y), diamond(x, y, 0, 4.9, 0.11, 0.2)), p);
          },
        });
        break;
      case 'back':
        layers.push({
          key: `tattoo:${id}:${ink}`,
          bounds: [-1.8, 3.5, -9, 1.8, 5.9, 0.9],
          regions: [REGION.torso, REGION.neck],
          paint(p) {
            if (Math.abs(Math.atan2(p[0], p[2] - tz)) < 1.9 || p[1] < 3.6) return null;
            const x = Math.abs(p[0]), y = p[1];
            return paint(Math.max(strokes(BACK, x, y), diamond(x, y, 0, 5.25, 0.1, 0.17), diamond(x, y, 0, 4.85, 0.08, 0.13)), p);
          },
        });
        break;
      case 'face_marks':
        layers.push({
          key: `tattoo:${id}:${ink}`,
          bounds: [-0.7, 6.6, 0.8, 0.7, 7.2, 9],
          regions: [REGION.head],
          paint(p) {
            if (p[2] < 0.9) return null;
            return paint(strokes(FACE, Math.abs(p[0]), p[1], 0.008), p, 0.75);
          },
        });
        break;
    }
  return layers;
}
