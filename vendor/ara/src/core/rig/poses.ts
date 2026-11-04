/**
 * Bone-name mapping between the game rig (Mixamo names, which clips are
 * written against) and the other skeleton types.
 */
import type { RigType } from './skeletons';

/** Game-rig bone name -> bone in the chosen skeleton (the full MakeHuman rig uses its own names). */
export function boneFor(rig: RigType, name: string): string | undefined {
  if (rig !== 'full') return name;
  const side = name.startsWith('Left') ? '.L' : name.startsWith('Right') ? '.R' : '';
  const base = name.replace(/^(Left|Right)/, '');
  const MAP: Record<string, string> = {
    Hips: 'root', Spine: 'spine04', Spine1: 'spine03', Spine2: 'spine02', Neck: 'neck01', Head: 'head',
    Shoulder: 'clavicle', Arm: 'upperarm01', ForeArm: 'lowerarm01', Hand: 'wrist',
    UpLeg: 'upperleg01', Leg: 'lowerleg01', Foot: 'foot',
    Eye: 'eye', Jaw: 'jaw',
  };
  const f = /^Hand(Thumb|Index|Middle|Ring|Pinky)(\d)$/.exec(base);
  if (f) return `finger${['Thumb', 'Index', 'Middle', 'Ring', 'Pinky'].indexOf(f[1]) + 1}-${f[2]}${side}`;
  return MAP[base] ? MAP[base] + side : undefined;
}
