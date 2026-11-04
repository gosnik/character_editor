/**
 * Skin overlay: tattoos, stubble and shaved-scalp hair are painted into one
 * RGBA texture in the base mesh's UV space and shown on a thin copy of the
 * body faces that carry any paint. Designs are evaluated on the *unmorphed*
 * base mesh, so they follow every body shape through the UVs and the texture
 * can be cached across slider changes.
 */
import type { AssetLibrary } from '../assets';
import { vertexNormals } from '../geometry';
import { encodePNGAsync } from '../garments/textures';
import { makeBody, type Body } from '../garments/body';
import type { Vec3 } from '../math';

/** RGB 0..255 + alpha 0..1 (null / alpha 0 = no paint). */
export type Paint = [number, number, number, number] | null;

export interface Layer {
  /** Identifies the layer's look (design + colours): its paint is cached under this key. */
  key: string;
  /** Body regions this layer can paint (limits the faces rasterised). */
  regions: number[];
  /** Optional box on the base body (min xyz, max xyz) outside which the layer never paints: faces outside are skipped. */
  bounds?: [number, number, number, number, number, number];
  /** Paint at a point on the base (unmorphed) body, in decimetres. */
  paint(p: Vec3, region: number, side: number): Paint;
}

const baseBodies = new WeakMap<AssetLibrary, Body>();
/** The unmorphed base body with its region coordinates (designs are defined on it). */
export function baseBody(lib: AssetLibrary): Body {
  let b = baseBodies.get(lib);
  if (!b) {
    const fv = lib.base.arrays.faceVerts as Uint32Array;
    const positions = lib.base.arrays.positions as Float32Array;
    const normals = vertexNormals({
      positions, faceVerts: fv, faceSizes: new Uint8Array(fv.length / 4).fill(4),
      faceUVs: lib.base.arrays.faceUVs as Uint32Array, uvs: lib.base.arrays.uvs as Float32Array,
    });
    b = makeBody(positions, normals, lib.base.arrays.regions as Uint8Array, lib.base.meta.joints);
    baseBodies.set(lib, b);
  }
  return b;
}

export interface Painted {
  /** PNG (RGBA). */
  png: Uint8Array;
  /** Base-mesh faces of the overlay: those that carry paint plus two rings of (transparent) neighbours. */
  faces: Uint8Array;
}

/** Grow a face selection by `rings` rings of vertex-adjacent faces. */
function grow(fv: Uint32Array, sel: Uint8Array, nVerts: number, rings: number): Uint8Array {
  const nf = fv.length / 4;
  let out = sel.slice();
  for (let ring = 0; ring < rings; ring++) {
    const vmark = new Uint8Array(nVerts);
    for (let f = 0; f < nf; f++) if (out[f]) for (let k = 0; k < 4; k++) vmark[fv[f * 4 + k]] = 1;
    const next = out.slice();
    for (let f = 0; f < nf; f++) if (!next[f]) for (let k = 0; k < 4; k++) if (vmark[fv[f * 4 + k]]) next[f] = 1;
    out = next;
  }
  return out;
}

/** One layer's paint, kept sparse: the texels it covers and their colour (RGBA, straight alpha). */
interface LayerRaster {
  idx: Uint32Array;
  col: Uint8Array;
  /** Base faces carrying paint. */
  faces: Uint32Array;
}

/** Scratch buffers per texture size (reused between paints; only touched texels are cleared). */
interface Scratch {
  acc: Float32Array; // premultiplied RGBA
  px: Uint8Array;
  used: Uint8Array;
  mark: Uint8Array;
}
const scratch = new Map<number, Scratch>();
const getScratch = (size: number): Scratch => {
  let s = scratch.get(size);
  if (!s) {
    const n = size * size;
    s = { acc: new Float32Array(n * 4), px: new Uint8Array(n * 4), used: new Uint8Array(n), mark: new Uint8Array(n) };
    scratch.set(size, s);
  }
  return s;
};

/** Visit the texels of base face `f` (both triangles), with the interpolated base-body position when `withPos`. */
function rasterFace(lib: AssetLibrary, f: number, size: number, withPos: boolean, visit: (t: number, p: Vec3) => void) {
  const P = baseBody(lib).coords;
  const fv = lib.base.arrays.faceVerts as Uint32Array;
  const fu = lib.base.arrays.faceUVs as Uint32Array;
  const uv = lib.base.arrays.uvs as Float32Array;
  const p: Vec3 = [0, 0, 0];
  for (const [a, b, c] of [[0, 1, 2], [0, 2, 3]]) {
    const i0 = f * 4 + a, i1 = f * 4 + b, i2 = f * 4 + c;
    const tx0 = uv[fu[i0] * 2] * size - 0.5, tx1 = uv[fu[i1] * 2] * size - 0.5, tx2 = uv[fu[i2] * 2] * size - 0.5;
    const ty0 = (1 - uv[fu[i0] * 2 + 1]) * size - 0.5, ty1 = (1 - uv[fu[i1] * 2 + 1]) * size - 0.5, ty2 = (1 - uv[fu[i2] * 2 + 1]) * size - 0.5;
    const v0 = fv[i0] * 3, v1 = fv[i1] * 3, v2 = fv[i2] * 3;
    const area = (tx1 - tx0) * (ty2 - ty0) - (tx2 - tx0) * (ty1 - ty0);
    if (Math.abs(area) < 1e-9) continue;
    const x0 = Math.max(0, Math.floor(Math.min(tx0, tx1, tx2))), x1 = Math.min(size - 1, Math.ceil(Math.max(tx0, tx1, tx2)));
    const y0 = Math.max(0, Math.floor(Math.min(ty0, ty1, ty2))), y1 = Math.min(size - 1, Math.ceil(Math.max(ty0, ty1, ty2)));
    for (let y = y0; y <= y1; y++)
      for (let x = x0; x <= x1; x++) {
        // Barycentrics, with a small margin so neighbouring triangles overlap (no cracks).
        const w1 = ((x - tx0) * (ty2 - ty0) - (tx2 - tx0) * (y - ty0)) / area;
        const w2 = ((tx1 - tx0) * (y - ty0) - (x - tx0) * (ty1 - ty0)) / area;
        const w0 = 1 - w1 - w2;
        if (w0 < -0.02 || w1 < -0.02 || w2 < -0.02) continue;
        if (withPos) for (let d = 0; d < 3; d++) p[d] = P[v0 + d] * w0 + P[v1 + d] * w1 + P[v2 + d] * w2;
        visit(y * size + x, p);
      }
  }
}

/** Evaluate one layer over its regions (the expensive part; cached by the layer's key). */
function paintLayer(lib: AssetLibrary, layer: Layer, size: number): LayerRaster {
  const body = baseBody(lib);
  const fv = lib.base.arrays.faceVerts as Uint32Array;
  const { mark } = getScratch(size);
  const idx: number[] = [], col: number[] = [], faces: number[] = [];
  for (let f = 0; f < fv.length / 4; f++) {
    const region = body.regions[fv[f * 4]];
    if (!layer.regions.includes(region)) continue;
    if (layer.bounds) {
      const b = layer.bounds;
      let out = false;
      for (let d = 0; d < 3 && !out; d++) {
        let lo = Infinity, hi = -Infinity;
        for (let k = 0; k < 4; k++) {
          const x = body.coords[fv[f * 4 + k] * 3 + d];
          lo = Math.min(lo, x);
          hi = Math.max(hi, x);
        }
        out = hi < b[d] || lo > b[d + 3];
      }
      if (out) continue;
    }
    const side = body.coords[fv[f * 4] * 3] >= 0 ? 1 : -1;
    let any = false;
    rasterFace(lib, f, size, true, (t, p) => {
      if (mark[t]) return; // shared triangle edges: paint each texel once
      const c = layer.paint(p, region, side);
      if (!c || c[3] <= 0.002) return;
      mark[t] = 1;
      idx.push(t);
      col.push(c[0], c[1], c[2], Math.round(Math.min(1, c[3]) * 255));
      any = true;
    });
    if (any) faces.push(f);
  }
  for (const t of idx) mark[t] = 0;
  return { idx: Uint32Array.from(idx), col: Uint8Array.from(col), faces: Uint32Array.from(faces) };
}

const rasterCache = new WeakMap<AssetLibrary, Map<string, LayerRaster>>();

/**
 * Composite layers (in order, "over") into a size x size RGBA texture. Each
 * layer's paint is cached by its key, so changing one tattoo only repaints
 * that tattoo.
 */
export async function paintOverlay(lib: AssetLibrary, layers: Layer[], size = 2048): Promise<Painted> {
  let cache = rasterCache.get(lib);
  if (!cache) rasterCache.set(lib, (cache = new Map()));
  const rasters = layers.map((l) => {
    const key = `${size}|${l.key}`;
    let r = cache!.get(key);
    if (!r) {
      r = paintLayer(lib, l, size);
      cache!.set(key, r);
      if (cache!.size > 32) cache!.delete(cache!.keys().next().value!); // drop the oldest
    } else {
      cache!.delete(key); // refresh LRU position
      cache!.set(key, r);
    }
    return r;
  });

  const fv = lib.base.arrays.faceVerts as Uint32Array;
  const nf = fv.length / 4;
  const { acc, px, used, mark } = getScratch(size);
  const touched: number[] = [];
  for (const r of rasters)
    for (let i = 0; i < r.idx.length; i++) {
      const t = r.idx[i], o = t * 4;
      if (!mark[t]) {
        mark[t] = 1;
        touched.push(t);
      }
      const A = r.col[i * 4 + 3] / 255, keep = 1 - A;
      acc[o] = r.col[i * 4] * A + acc[o] * keep;
      acc[o + 1] = r.col[i * 4 + 1] * A + acc[o + 1] * keep;
      acc[o + 2] = r.col[i * 4 + 2] * A + acc[o + 2] * keep;
      acc[o + 3] = A + acc[o + 3] * keep;
    }

  // Overlay faces: painted ones plus two rings of transparent neighbours. Claim their texels,
  // so edge bleeding below only fills the gutters between UV islands.
  const painted = new Uint8Array(nf);
  for (const r of rasters) for (const f of r.faces) painted[f] = 1;
  const sel = grow(fv, painted, baseBody(lib).coords.length / 3, 2);
  for (let f = 0; f < nf; f++) if (sel[f]) rasterFace(lib, f, size, false, (t) => (used[t] = 1));

  // Bleed colour (and alpha) up to 4 texels past island edges so mipmaps don't show seams.
  // Only texels next to paint can change, so walk outward from the painted ones.
  let frontier = touched;
  const filled: number[] = [];
  const around = [1, -1, size, -size];
  for (let pass = 0; pass < 4 && frontier.length; pass++) {
    const cand: number[] = [];
    for (const t of frontier)
      for (const d of around) {
        const q = t + d;
        if (q < 0 || q >= size * size || used[q] || mark[q] === 2 || ((d === 1 || d === -1) && Math.floor(q / size) !== Math.floor(t / size))) continue;
        mark[q] = 2;
        cand.push(q);
      }
    const vals = new Float32Array(cand.length * 4);
    cand.forEach((q, i) => {
      let n = 0;
      const x = q % size;
      for (const d of around) {
        const r = q + d;
        if (r < 0 || r >= size * size || (d === 1 && x === size - 1) || (d === -1 && x === 0)) continue;
        if (!used[r] && mark[r] !== 3) continue;
        for (let k = 0; k < 4; k++) vals[i * 4 + k] += acc[r * 4 + k];
        n++;
      }
      if (n) for (let k = 0; k < 4; k++) vals[i * 4 + k] /= n;
    });
    cand.forEach((q, i) => {
      for (let k = 0; k < 4; k++) acc[q * 4 + k] = vals[i * 4 + k];
      mark[q] = 3; // filled: counts as a source from the next pass on
      filled.push(q);
    });
    frontier = cand;
  }

  // Un-premultiply into the output, encode, then reset the touched scratch texels.
  const all = [...touched, ...filled];
  for (const t of all) {
    const A = acc[t * 4 + 3];
    if (A <= 0) continue;
    px[t * 4] = Math.min(255, acc[t * 4] / A);
    px[t * 4 + 1] = Math.min(255, acc[t * 4 + 1] / A);
    px[t * 4 + 2] = Math.min(255, acc[t * 4 + 2] / A);
    px[t * 4 + 3] = Math.round(Math.min(1, A) * 255);
  }
  // Copy out before the (async) encode, so the shared scratch buffers can be reset right away.
  const image = px.slice();
  for (const t of all) {
    acc.fill(0, t * 4, t * 4 + 4);
    px.fill(0, t * 4, t * 4 + 4);
    mark[t] = 0;
  }
  used.fill(0);
  return { png: await encodePNGAsync(size, size, image, true), faces: sel };
}

/**
 * The overlay surface: painted faces plus two rings of neighbours (so that
 * subdividing gives the same surface as the body underneath), minus faces
 * hidden under clothes.
 */
export function overlaySurface(faces: Uint8Array, hidden: Uint8Array): Uint8Array {
  const sel = faces.slice();
  for (let f = 0; f < sel.length; f++) if (hidden[f]) sel[f] = 0;
  return sel;
}
