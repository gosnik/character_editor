/**
 * Lying down and sleeping on the back, built on Get up (from back): Lie down is that clip played
 * in reverse, Sleep holds its first (lying) pose with slow breathing. They chain:
 * Lie down -> Sleep -> Get up (from back).
 */
import { type Ch, type Clip } from './types';
import { getUpFaceUp } from './reactions';

const LIE_LEN = 2.6; // a little slower than getting up: lowering is controlled

export const lieDown: Clip = {
  id: 'lie_down', name: 'Lie down', category: 'Sitting & lying', duration: LIE_LEN, loop: false,
  sample: (t) => getUpFaceUp.sample((1 - Math.min(1, Math.max(0, t / LIE_LEN))) * getUpFaceUp.duration),
};

const BREATH = 4.8; // about 12 breaths a minute

export const sleep: Clip = {
  id: 'sleep', name: 'Sleep', category: 'Sitting & lying', duration: BREATH, loop: true,
  sample: (t) => {
    const s = getUpFaceUp.sample(0);
    const b = Math.sin((t / BREATH) * Math.PI * 2);
    const add = (name: string, d: Ch) => {
      const c = s.bones[name] ?? [0, 0, 0];
      s.bones[name] = [c[0] + d[0], c[1] + d[1], c[2] + d[2]];
    };
    // The chest rises and falls; the head rocks with it.
    add('Spine1', [-0.8 * b, 0, 0]);
    add('Spine2', [-1.2 * b, 0, 0]);
    add('Neck', [0.6 * b, 0, 0]);
    add('Head', [0.8 * b, 0, 0]);
    return s;
  },
};
