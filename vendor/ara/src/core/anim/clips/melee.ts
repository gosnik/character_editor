/** Melee set: sword guard, slash and block. */
import { type Ch, type Clip, type HandTarget } from './types';
import { crossV, fist, FISTS, normV, side, track, v } from './helpers';

// The blade leaves the top of the fist (socket +Y). For the right hand that direction is
// dir x palm, so a hand target is built from the blade and edge directions we want.
/** Right-hand target holding a blade pointing `blade`, edge (knuckles) toward `edge`. */
function swordHand(pos: [number, number, number], blade: [number, number, number], edge: [number, number, number], pole: [number, number, number]): HandTarget {
  const B = normV(blade);
  const k = edge[0] * B[0] + edge[1] * B[1] + edge[2] * B[2];
  const F = normV(v(edge[0] - B[0] * k, edge[1] - B[1] * k, edge[2] - B[2] * k));
  return { pos, pole, dir: F, palm: crossV(B, F) };
}
function swordBody(out: Record<string, Ch>) {
  side(out, 'UpLeg', [14, 0, 2], [-10, 0, 2]);
  side(out, 'Leg', [-22, 0, 0], [-10, 0, 0]);
  side(out, 'Foot', [6, 0, 0], [0, 0, 0]);
  out.Spine = [6, 0, 0];
  side(out, 'Arm', [35, 0, 15], [0, 0, 0]);
  side(out, 'ForeArm', [75, 0, 0], [0, 0, 0]);
  fist(out, ['Left']);
  fist(out, ['Right']);
}
const swordIdle: Clip = {
  id: 'sword_idle', name: 'Sword idle (guard)', category: 'Actions', duration: 3, loop: true,
  sample: (t) => {
    const out: Record<string, Ch> = {};
    swordBody(out);
    const b = Math.sin((t / 3) * Math.PI * 2);
    out.Spine1 = [b, 0, 0];
    return {
      bones: out, root: { ground: true, lift: 0 }, thumbs: FISTS,
      ik: { Right: swordHand(v(-0.14, -0.31 + 0.01 * b, 0.32), v(0.05, 0.8, 0.6), v(0, -0.6, 0.8), v(-1, -0.8, -0.3)) },
    };
  },
};
const swordSlash: Clip = {
  id: 'sword_slash', name: 'Sword slash', category: 'Actions', duration: 1.0, loop: false,
  sample: (t) => {
    const T = [0, 0.3, 0.45, 0.6, 1.0];
    const k = (vals: number[]) => track(T, vals)(t);
    const out: Record<string, Ch> = {};
    swordBody(out);
    out.Hips = [0, k([0, -18, 5, 15, 0]), 0];
    out.Spine1 = [k([0, -4, 8, 10, 0]), k([0, -22, 8, 22, 0]), 0];
    out.Head = [0, k([0, 12, 0, -10, 0]), 0];
    const pos = v(k([-0.14, -0.32, -0.05, 0.16, -0.14]), k([-0.31, 0.12, -0.2, -0.5, -0.31]), k([0.32, 0.05, 0.45, 0.32, 0.32]));
    const blade = v(k([0.05, -0.3, 0.4, 0.6, 0.05]), k([0.8, 0.6, -0.1, -0.5, 0.8]), k([0.6, -0.7, 0.9, 0.6, 0.6]));
    const edge = v(k([0, 0.3, 0.6, 0.5, 0]), k([-0.6, 0.6, -0.5, -0.6, -0.6]), k([0.8, 0.5, 0.2, -0.2, 0.8]));
    return { bones: out, root: { ground: true }, thumbs: FISTS, ik: { Right: swordHand(pos, blade, edge, v(-1, -0.6, -0.3)) } };
  },
};
const swordBlock: Clip = {
  id: 'sword_block', name: 'Sword block', category: 'Actions', duration: 0.8, loop: false,
  sample: (t) => {
    const k = (vals: number[]) => track([0, 0.2, 0.8], vals)(t);
    const out: Record<string, Ch> = {};
    swordBody(out);
    out.Spine1 = [k([0, -6, -6]), 0, 0];
    out.Head = [k([0, -6, -6]), 0, 0];
    const pos = v(k([-0.14, -0.2, -0.2]), k([-0.31, 0.14, 0.14]), k([0.32, 0.28, 0.28]));
    const blade = v(k([0.05, 1, 1]), k([0.8, 0.15, 0.15]), k([0.6, 0.05, 0.05]));
    return { bones: out, root: { ground: true }, thumbs: FISTS, ik: { Right: swordHand(pos, blade, v(0, 1, 0.3), v(-1, -0.4, -0.3)) } };
  },
};

export const MELEE: Clip[] = [
  swordIdle,
  swordSlash,
  swordBlock,
];
