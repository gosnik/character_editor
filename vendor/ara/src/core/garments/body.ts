/**
 * Body coordinate system for procedural garments. Every body vertex gets a
 * region (torso, upper arm...), a side, a position along its region's axis
 * (t) and an angle around that axis (theta, 0 = front). Garments are defined
 * against these, so they follow any morph.
 *
 * Units here are MakeHuman decimetres (1 cm = 0.1).
 */
import type { Vec3 } from '../math';

export const REGION = {
  head: 0, neck: 1, torso: 2, hips: 3, upperarm: 4, lowerarm: 5, hand: 6, upperleg: 7, lowerleg: 8, foot: 9,
} as const;
export type RegionName = keyof typeof REGION;

export const cm = (x: number) => x * 0.1;

export interface VInfo {
  region: number;
  /** +1 = character's left (+X), -1 = right. */
  side: number;
  /**
   * Position along the region axis. Torso/hips/neck share one vertical axis:
   * 0 = hip joints, 1 = neck base (can go below 0 / above 1). Limbs: 0 at the
   * proximal joint, 1 at the distal joint. Head: 0 = chin, 1 = crown.
   */
  t: number;
  /** Angle around the axis, radians; 0 = front, +PI/2 = toward the character's left. */
  theta: number;
  /** Trunk coordinates for every vertex (same axis as torso t/theta), for necklines and armholes. */
  tt: number;
  tth: number;
  /** Lateral distance from the body midline, decimetres. */
  ax: number;
}

export interface Body {
  coords: Float32Array; // morphed base-mesh coordinates (all vertices)
  normals: Float32Array; // body vertex normals
  regions: Uint8Array;
  info: VInfo[];
  joint(name: string): Vec3;
  /** Key landmarks (decimetres). */
  L: {
    /** Shoulder joint lateral distance (decimetres). */
    shoulderX: number;
    hipY: number;
    neckY: number;
    torsoZ: number;
    chinY: number;
    crownY: number;
    headZ: number;
  };
  /** Body vertex indices of a region (optionally one side). */
  verts(region: number, side?: number): number[];
  /** Limb axis endpoints for a region and side. */
  axis(region: number, side: number): [Vec3, Vec3];
  /** Vertex position as Vec3. */
  p(v: number): Vec3;
}

const sub = (a: Vec3, b: Vec3): Vec3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const dot = (a: Vec3, b: Vec3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a: Vec3, b: Vec3): Vec3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const norm = (a: Vec3): Vec3 => {
  const l = Math.hypot(a[0], a[1], a[2]) || 1;
  return [a[0] / l, a[1] / l, a[2] / l];
};

const LIMB: Record<number, [string, string]> = {
  [REGION.upperarm]: ['upperarm01', 'lowerarm01'],
  [REGION.lowerarm]: ['lowerarm01', 'wrist'],
  [REGION.hand]: ['wrist', 'finger3-3'],
  [REGION.upperleg]: ['upperleg01', 'lowerleg01'],
  [REGION.lowerleg]: ['lowerleg01', 'foot'],
  [REGION.foot]: ['foot', 'toe3-1'],
};

export function makeBody(
  coords: Float32Array,
  normals: Float32Array,
  regions: Uint8Array,
  joints: Record<string, number[]>,
): Body {
  const p = (v: number): Vec3 => [coords[v * 3], coords[v * 3 + 1], coords[v * 3 + 2]];
  const jcache = new Map<string, Vec3>();
  const joint = (name: string): Vec3 => {
    let j = jcache.get(name);
    if (!j) {
      const idx = joints[name] ?? joints[`${name}____head`];
      if (!idx) throw new Error(`unknown joint ${name}`);
      const s: Vec3 = [0, 0, 0];
      for (const v of idx) for (let d = 0; d < 3; d++) s[d] += coords[v * 3 + d];
      j = [s[0] / idx.length, s[1] / idx.length, s[2] / idx.length];
      jcache.set(name, j);
    }
    return j;
  };
  const head = (b: string, side: number) => joint(`${b}${side > 0 ? '.L' : '.R'}____head`);
  const axis = (region: number, side: number): [Vec3, Vec3] => {
    const [a, b] = LIMB[region];
    return [head(a, side), head(b, side)];
  };

  const n = regions.length;
  const byRegion = new Map<number, number[]>();
  let headMin = Infinity, headMax = -Infinity, torsoZ = 0, torsoN = 0, headZ = 0, headN = 0;
  for (let v = 0; v < n; v++) {
    const r = regions[v];
    if (r === 255) continue;
    let list = byRegion.get(r);
    if (!list) byRegion.set(r, (list = []));
    list.push(v);
    const y = coords[v * 3 + 1];
    if (r === REGION.head) {
      headMin = Math.min(headMin, y);
      headMax = Math.max(headMax, y);
      headZ += coords[v * 3 + 2];
      headN++;
    }
    if (r === REGION.torso) {
      torsoZ += coords[v * 3 + 2];
      torsoN++;
    }
  }
  const hipY = (head('upperleg01', 1)[1] + head('upperleg01', -1)[1]) / 2;
  const neckY = joint('neck01____head')[1];
  const shoulderX = (Math.abs(head('upperarm01', 1)[0]) + Math.abs(head('upperarm01', -1)[0])) / 2;
  const L = { shoulderX, hipY, neckY, torsoZ: torsoZ / torsoN, chinY: headMin, crownY: headMax, headZ: headZ / headN };

  const info: VInfo[] = new Array(n);
  const limbFrames = new Map<string, { a: Vec3; d: Vec3; len: number; side: Vec3; front: Vec3 }>();
  const frame = (r: number, s: number) => {
    const key = `${r}${s}`;
    let f = limbFrames.get(key);
    if (!f) {
      const [a, b] = axis(r, s);
      const ab = sub(b, a);
      const len = Math.hypot(...ab);
      const d = norm(ab);
      // Front reference: +Z projected off the axis (for feet use +Y).
      const ref: Vec3 = r === REGION.foot ? [0, 1, 0] : [0, 0, 1];
      const front = norm(sub(ref, [d[0] * dot(ref, d), d[1] * dot(ref, d), d[2] * dot(ref, d)]));
      f = { a, d, len, side: cross(front, d), front };
      limbFrames.set(key, f);
    }
    return f;
  };

  for (let v = 0; v < n; v++) {
    const r = regions[v];
    const P = p(v);
    const side = P[0] >= 0 ? 1 : -1;
    const tt = (P[1] - hipY) / (neckY - hipY), tth = Math.atan2(P[0], P[2] - L.torsoZ), ax = Math.abs(P[0]);
    if (r === 255) {
      info[v] = { region: r, side, t: 0, theta: 0, tt, tth, ax };
    } else if (r === REGION.torso || r === REGION.hips || r === REGION.neck) {
      info[v] = { region: r, side, t: tt, theta: tth, tt, tth, ax };
    } else if (r === REGION.head) {
      info[v] = { region: r, side, t: (P[1] - L.chinY) / (L.crownY - L.chinY), theta: Math.atan2(P[0], P[2] - L.headZ), tt, tth, ax };
    } else {
      const f = frame(r, side);
      const rel = sub(P, f.a);
      const t = dot(rel, f.d) / f.len;
      // Angle measured so +theta points toward the character's left on both sides.
      const theta = Math.atan2(dot(rel, f.side) * -1, dot(rel, f.front));
      info[v] = { region: r, side, t, theta, tt, tth, ax };
    }
  }

  return {
    coords, normals, regions, info, joint, L, axis, p,
    verts: (region, side) => (byRegion.get(region) ?? []).filter((v) => side === undefined || info[v].side === side),
  };
}

/** Tileable-ish 3D hash noise in [0,1), for ragged hems and variation. */
export function hash3(x: number, y: number, z: number): number {
  const s = Math.sin(x * 127.1 + y * 311.7 + z * 74.7) * 43758.5453;
  return s - Math.floor(s);
}

/** Smooth value noise in [0,1). */
export function noise3(x: number, y: number, z: number): number {
  const xi = Math.floor(x), yi = Math.floor(y), zi = Math.floor(z);
  const xf = x - xi, yf = y - yi, zf = z - zi;
  const s = (t: number) => t * t * (3 - 2 * t);
  const u = s(xf), v = s(yf), w = s(zf);
  const l = (a: number, b: number, t: number) => a + (b - a) * t;
  const h = (dx: number, dy: number, dz: number) => hash3(xi + dx, yi + dy, zi + dz);
  return l(
    l(l(h(0, 0, 0), h(1, 0, 0), u), l(h(0, 1, 0), h(1, 1, 0), u), v),
    l(l(h(0, 0, 1), h(1, 0, 1), u), l(h(0, 1, 1), h(1, 1, 1), u), v),
    w,
  );
}
