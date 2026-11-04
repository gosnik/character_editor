/** Procedural textures for generated hair: hair cards, braids and gelled spikes (greyscale, tinted by the material colour). */
import { render, type TexOut } from '../garments/textures';

const frac = (x: number) => x - Math.floor(x);
const hash = (i: number) => frac(Math.sin(i * 127.1 + 311.7) * 43758.5453);

/** Hair card (alpha mask): ~7 tapered strands of varied length, root at v = 0 (bottom). */
export function hairCardTexture(): TexOut {
  const N = 7;
  return render(64, (u, v) => {
    const y = 1 - v; // image v runs top -> bottom; strands grow from the root at the bottom
    let a = 0, shade = 0;
    for (let k = 0; k < N; k++) {
      const len = 0.65 + 0.35 * hash(k);
      if (y > len) continue;
      const cx = (k + 0.5) / N + (hash(k + 9) - 0.5) * 0.06 + Math.sin(y * 5 + k) * 0.015;
      const w = (0.55 / N) * (1 - (y / len) ** 2 * 0.75);
      const d = Math.abs(u - cx);
      if (d < w) {
        a = 1;
        shade = Math.max(shade, (0.55 + 0.45 * hash(k + 3)) * (0.75 + 0.25 * Math.min(1, y * 4)) - 0.15 * (d / w));
      }
    }
    const g = 255 * shade;
    return { c: [g, g, g], a };
  }, true);
}

/** Braid relief: overlapping lobes slanting left and right along v; u wraps around the braid. */
export function braidTexture(): TexOut {
  return render(128, (u, v) => {
    // Two strands cross per repeat: chevrons whose arms follow |u - 0.5|.
    const c = Math.abs(frac(u * 2) - 0.5); // 0 at the braid's crest lines
    const lobe = frac(v * 2 + c * 1.4);
    const h = Math.sin(lobe * Math.PI) * (1 - 0.4 * c);
    const fibre = 0.85 + 0.15 * Math.sin((u * 30 + lobe * 6) * Math.PI);
    const g = (90 + 165 * h) * fibre;
    return { c: [g, g, g], h };
  }, false, 3);
}

/** Gelled spike: fine lengthwise strands. */
export function spikeTexture(): TexOut {
  return render(64, (u, v) => {
    const s = 0.75 + 0.25 * Math.sin(u * Math.PI * 2 * 12 + Math.sin(v * 9) * 1.5) * (0.6 + 0.4 * hash(Math.floor(u * 24)));
    const g = 255 * s;
    return { c: [g, g, g], h: s };
  }, false, 1.5);
}
