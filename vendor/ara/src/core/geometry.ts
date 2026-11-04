/**
 * Polygon-mesh helpers: compaction, Catmull-Clark subdivision, normals, and
 * conversion to per-vertex (UV-split) arrays.
 */

/** Up to 4 bone influences per vertex. */
export interface Skin {
  j: Uint16Array; // 4 per vertex
  w: Float32Array; // 4 per vertex, sum 1
}

/** A polygon mesh with face-varying UVs (as in OBJ). */
export interface PolyMesh {
  positions: Float32Array; // xyz per vertex
  faceSizes: Uint8Array;
  faceVerts: Uint32Array; // vertex index per face corner
  faceUVs: Uint32Array; // uv index per face corner
  uvs: Float32Array; // uv pairs
  /** Skin weights, once assigned. */
  skin?: Skin;
  /** Base-mesh vertex each vertex was derived from (-1 = none), for weight transfer. */
  ref?: Int32Array;
  /** Blend-shape offsets: one array (vertex count * 3) per face shape, in the same order everywhere. */
  morphs?: Float32Array[];
}

/** Blend weighted source influences into the top 4, normalised. */
export function blendInfluences(acc: Map<number, number>, j: Uint16Array, w: Float32Array, o: number): void {
  const top = [...acc.entries()].filter(([, x]) => x > 1e-6).sort((a, b) => b[1] - a[1]).slice(0, 4);
  const sum = top.reduce((s, [, x]) => s + x, 0) || 1;
  for (let k = 0; k < 4; k++) {
    j[o + k] = top[k]?.[0] ?? 0;
    w[o + k] = top[k] ? top[k][1] / sum : 0;
  }
}

/** Skin for new vertices that are weighted averages of source vertices. */
function mixSkin(src: Skin, sources: { v: number[]; f: number }[]): Skin {
  const j = new Uint16Array(sources.length * 4), w = new Float32Array(sources.length * 4);
  const acc = new Map<number, number>();
  sources.forEach((s, i) => {
    acc.clear();
    for (const v of s.v)
      for (let k = 0; k < 4; k++) {
        const b = src.j[v * 4 + k], x = src.w[v * 4 + k] * s.f;
        if (x) acc.set(b, (acc.get(b) ?? 0) + x);
      }
    blendInfluences(acc, j, w, i * 4);
  });
  return { j, w };
}

/** Keep only faces passing `keep` and vertices they use. */
export function compact(m: PolyMesh, keep?: (f: number, corners: Uint32Array) => boolean): PolyMesh {
  const remap = new Int32Array(m.positions.length / 3).fill(-1);
  const sizes: number[] = [], verts: number[] = [], uvIdx: number[] = [];
  const pos: number[] = [];
  const order: number[] = [];
  let c = 0;
  for (let f = 0; f < m.faceSizes.length; f++) {
    const n = m.faceSizes[f];
    const corners = m.faceVerts.subarray(c, c + n);
    if (!keep || keep(f, corners)) {
      sizes.push(n);
      for (let k = 0; k < n; k++) {
        const v = corners[k];
        if (remap[v] < 0) {
          remap[v] = pos.length / 3;
          order.push(v);
          pos.push(m.positions[v * 3], m.positions[v * 3 + 1], m.positions[v * 3 + 2]);
        }
        verts.push(remap[v]);
        uvIdx.push(m.faceUVs[c + k]);
      }
    }
    c += n;
  }
  return {
    positions: Float32Array.from(pos),
    faceSizes: Uint8Array.from(sizes),
    faceVerts: Uint32Array.from(verts),
    faceUVs: Uint32Array.from(uvIdx),
    uvs: m.uvs,
    skin: m.skin && {
      j: Uint16Array.from(order.flatMap((v) => [...m.skin!.j.subarray(v * 4, v * 4 + 4)])),
      w: Float32Array.from(order.flatMap((v) => [...m.skin!.w.subarray(v * 4, v * 4 + 4)])),
    },
    ref: m.ref && Int32Array.from(order.map((v) => m.ref![v])),
    morphs: m.morphs?.map((d) => {
      const out = new Float32Array(order.length * 3);
      order.forEach((v, i) => out.set(d.subarray(v * 3, v * 3 + 3), i * 3));
      return out;
    }),
  };
}

/** One level of Catmull-Clark subdivision. Boundaries use the crease rules; UVs are interpolated linearly. */
export function subdivide(m: PolyMesh): PolyMesh {
  const nv = m.positions.length / 3;
  const nf = m.faceSizes.length;
  const faceStart = new Uint32Array(nf);
  for (let f = 0, c = 0; f < nf; f++) {
    faceStart[f] = c;
    c += m.faceSizes[f];
  }

  // Edges
  const edgeId = new Map<number, number>();
  const edgeA: number[] = [], edgeB: number[] = [], edgeF0: number[] = [], edgeF1: number[] = [];
  const cornerEdge = new Uint32Array(m.faceVerts.length); // edge from corner k to k+1
  for (let f = 0; f < nf; f++) {
    const s = faceStart[f], n = m.faceSizes[f];
    for (let k = 0; k < n; k++) {
      const a = m.faceVerts[s + k], b = m.faceVerts[s + ((k + 1) % n)];
      const key = a < b ? a * nv + b : b * nv + a;
      let e = edgeId.get(key);
      if (e === undefined) {
        e = edgeA.length;
        edgeId.set(key, e);
        edgeA.push(a);
        edgeB.push(b);
        edgeF0.push(f);
        edgeF1.push(-1);
      } else edgeF1[e] = f;
      cornerEdge[s + k] = e;
    }
  }
  const ne = edgeA.length;

  // New vertex positions: a linear function of the old ones, so it applies to blend-shape offsets too.
  const points = (P: Float32Array): Float32Array => {
    // Face points
    const FP = new Float32Array(nf * 3);
    for (let f = 0; f < nf; f++) {
      const s = faceStart[f], n = m.faceSizes[f];
      for (let k = 0; k < n; k++) for (let d = 0; d < 3; d++) FP[f * 3 + d] += P[m.faceVerts[s + k] * 3 + d] / n;
    }

    // Edge points
    const EP = new Float32Array(ne * 3);
    for (let e = 0; e < ne; e++) {
      const a = edgeA[e], b = edgeB[e], f0 = edgeF0[e], f1 = edgeF1[e];
      for (let d = 0; d < 3; d++) {
        const mid = (P[a * 3 + d] + P[b * 3 + d]) / 2;
        EP[e * 3 + d] = f1 < 0 ? mid : (P[a * 3 + d] + P[b * 3 + d] + FP[f0 * 3 + d] + FP[f1 * 3 + d]) / 4;
      }
    }

    // Vertex points
    const fSum = new Float32Array(nv * 3), fCnt = new Uint32Array(nv);
    const eSum = new Float32Array(nv * 3), eCnt = new Uint32Array(nv);
    const bSum = new Float32Array(nv * 3), bCnt = new Uint32Array(nv);
    for (let f = 0; f < nf; f++) {
      const s = faceStart[f], n = m.faceSizes[f];
      for (let k = 0; k < n; k++) {
        const v = m.faceVerts[s + k];
        fCnt[v]++;
        for (let d = 0; d < 3; d++) fSum[v * 3 + d] += FP[f * 3 + d];
      }
    }
    for (let e = 0; e < ne; e++) {
      const a = edgeA[e], b = edgeB[e];
      for (const [v, o] of [[a, b], [b, a]]) {
        eCnt[v]++;
        for (let d = 0; d < 3; d++) eSum[v * 3 + d] += (P[v * 3 + d] + P[o * 3 + d]) / 2;
        if (edgeF1[e] < 0) {
          bCnt[v]++;
          for (let d = 0; d < 3; d++) bSum[v * 3 + d] += P[o * 3 + d];
        }
      }
    }
    const VP = new Float32Array(nv * 3);
    for (let v = 0; v < nv; v++) {
      for (let d = 0; d < 3; d++) {
        const p = P[v * 3 + d];
        if (bCnt[v] === 2) VP[v * 3 + d] = (6 * p + bSum[v * 3 + d]) / 8;
        else if (bCnt[v] > 0 || eCnt[v] < 3) VP[v * 3 + d] = p;
        else {
          const n = eCnt[v];
          const F = fSum[v * 3 + d] / fCnt[v], R = eSum[v * 3 + d] / n;
          VP[v * 3 + d] = (F + 2 * R + (n - 3) * p) / n;
        }
      }
    }

    const out = new Float32Array((nv + ne + nf) * 3);
    out.set(VP, 0);
    out.set(EP, nv * 3);
    out.set(FP, (nv + ne) * 3);
    return out;
  };
  const positions = points(m.positions);

  // Faces and face-varying UVs (deduplicated by value).
  const uvOut: number[] = [];
  const uvKey = new Map<string, number>();
  const uvIndex = (u: number, v: number) => {
    const key = `${Math.round(u * 1e6)},${Math.round(v * 1e6)}`;
    let i = uvKey.get(key);
    if (i === undefined) {
      i = uvOut.length / 2;
      uvOut.push(u, v);
      uvKey.set(key, i);
    }
    return i;
  };
  const U = m.uvs;
  const outVerts: number[] = [], outUVs: number[] = [];
  for (let f = 0; f < nf; f++) {
    const s = faceStart[f], n = m.faceSizes[f];
    let fu = 0, fv = 0;
    for (let k = 0; k < n; k++) {
      fu += U[m.faceUVs[s + k] * 2] / n;
      fv += U[m.faceUVs[s + k] * 2 + 1] / n;
    }
    const fuv = uvIndex(fu, fv);
    const fpt = nv + ne + f;
    for (let k = 0; k < n; k++) {
      const prev = (k + n - 1) % n, next = (k + 1) % n;
      const c = m.faceUVs[s + k], cp = m.faceUVs[s + prev], cn = m.faceUVs[s + next];
      const cu = U[c * 2], cv = U[c * 2 + 1];
      outVerts.push(m.faceVerts[s + k], nv + cornerEdge[s + k], fpt, nv + cornerEdge[s + prev]);
      outUVs.push(
        uvIndex(cu, cv),
        uvIndex((cu + U[cn * 2]) / 2, (cv + U[cn * 2 + 1]) / 2),
        fuv,
        uvIndex((cu + U[cp * 2]) / 2, (cv + U[cp * 2 + 1]) / 2),
      );
    }
  }
  let skin: Skin | undefined;
  if (m.skin) {
    const sources: { v: number[]; f: number }[] = [];
    for (let v = 0; v < nv; v++) sources.push({ v: [v], f: 1 });
    for (let e = 0; e < ne; e++) sources.push({ v: [edgeA[e], edgeB[e]], f: 0.5 });
    for (let f = 0; f < nf; f++) {
      const n = m.faceSizes[f];
      sources.push({ v: [...m.faceVerts.subarray(faceStart[f], faceStart[f] + n)], f: 1 / n });
    }
    skin = mixSkin(m.skin, sources);
  }
  return {
    skin,
    positions,
    morphs: m.morphs?.map(points),
    faceSizes: new Uint8Array(outVerts.length / 4).fill(4),
    faceVerts: Uint32Array.from(outVerts),
    faceUVs: Uint32Array.from(outUVs),
    uvs: Float32Array.from(uvOut),
  };
}

/** Smooth, angle-weighted vertex normals. */
export function vertexNormals(m: PolyMesh): Float32Array {
  const P = m.positions;
  const N = new Float32Array(P.length);
  for (let f = 0, c = 0; f < m.faceSizes.length; f++) {
    const n = m.faceSizes[f];
    // Newell normal of the polygon
    let nx = 0, ny = 0, nz = 0;
    for (let k = 0; k < n; k++) {
      const a = m.faceVerts[c + k] * 3, b = m.faceVerts[c + ((k + 1) % n)] * 3;
      nx += (P[a + 1] - P[b + 1]) * (P[a + 2] + P[b + 2]);
      ny += (P[a + 2] - P[b + 2]) * (P[a] + P[b]);
      nz += (P[a] - P[b]) * (P[a + 1] + P[b + 1]);
    }
    const l = Math.hypot(nx, ny, nz) || 1;
    nx /= l; ny /= l; nz /= l;
    for (let k = 0; k < n; k++) {
      const v = m.faceVerts[c + k] * 3;
      const p = m.faceVerts[c + ((k + n - 1) % n)] * 3, q = m.faceVerts[c + ((k + 1) % n)] * 3;
      const ax = P[p] - P[v], ay = P[p + 1] - P[v + 1], az = P[p + 2] - P[v + 2];
      const bx = P[q] - P[v], by = P[q + 1] - P[v + 1], bz = P[q + 2] - P[v + 2];
      const cos = (ax * bx + ay * by + az * bz) / (Math.hypot(ax, ay, az) * Math.hypot(bx, by, bz) || 1);
      const w = Math.acos(Math.max(-1, Math.min(1, cos)));
      N[v] += nx * w; N[v + 1] += ny * w; N[v + 2] += nz * w;
    }
    c += n;
  }
  for (let i = 0; i < N.length; i += 3) {
    const l = Math.hypot(N[i], N[i + 1], N[i + 2]);
    if (l > 1e-12) { N[i] /= l; N[i + 1] /= l; N[i + 2] /= l; } else N[i + 1] = 1;
  }
  return N;
}
