/**
 * Target skeletons. Each bone lists the MakeHuman source bones whose skin
 * weights it absorbs, and which MakeHuman joint marks its head. Bone names
 * follow Mixamo, so Mixamo/Unity Humanoid/Unreal retargeting maps them
 * automatically.
 */

export type RigType = 'game' | 'game_lite' | 'full' | 'none';

export interface BoneDef {
  name: string;
  parent: string | null;
  /** MakeHuman joint name for the head (e.g. "upperarm01.L____head"). */
  head: string;
  /** Explicit tail joint (leaf bones); otherwise the first child's head. */
  tail?: string;
  /** MakeHuman bones whose weights map onto this bone. */
  mh: string[];
}

const H = (b: string) => `${b}____head`;
const T = (b: string) => `${b}____tail`;

const FINGERS: [string, number][] = [['Thumb', 1], ['Index', 2], ['Middle', 3], ['Ring', 4], ['Pinky', 5]];

/** MakeHuman bone names grouped by role (filled from the actual skeleton). */
export interface MhBoneSet {
  all: string[];
}

export function gameSkeleton(mh: MhBoneSet, o: { fingers: boolean; face: boolean }): BoneDef[] {
  const bones: BoneDef[] = [];
  const add = (b: BoneDef) => bones.push(b);
  const faceBones = mh.all.filter((n) => /^(jaw|tongue)/.test(n));
  const eyeBones = (s: string) => mh.all.filter((n) => n === `eye.${s}`);
  const headExtras = mh.all.filter(
    (n) => /^(head|special|oris|levator|risorius|orbicularis|oculi|temporalis|jaw|tongue|eye)/.test(n) && n !== 'head',
  );

  add({ name: 'Hips', parent: null, head: H('spine05'), mh: ['root', 'pelvis.L', 'pelvis.R', 'spine05'] });
  add({ name: 'Spine', parent: 'Hips', head: H('spine04'), mh: ['spine04'] });
  add({ name: 'Spine1', parent: 'Spine', head: H('spine03'), mh: ['spine03'] });
  add({ name: 'Spine2', parent: 'Spine1', head: H('spine02'), mh: ['spine02', 'spine01', 'breast.L', 'breast.R'] });
  add({ name: 'Neck', parent: 'Spine2', head: H('neck01'), mh: ['neck01', 'neck02', 'neck03'] });
  add({
    name: 'Head', parent: 'Neck', head: H('head'),
    mh: ['head', ...headExtras.filter((n) => !(o.face && (faceBones.includes(n) || /^eye\./.test(n))))],
  });
  add({ name: 'HeadTop_End', parent: 'Head', head: T('head'), mh: [] });
  if (o.face) {
    add({ name: 'Jaw', parent: 'Head', head: H('jaw'), tail: T('jaw'), mh: faceBones });
    for (const [s, side] of [['L', 'Left'], ['R', 'Right']] as const)
      add({ name: `${side}Eye`, parent: 'Head', head: H(`eye.${s}`), tail: T(`eye.${s}`), mh: eyeBones(s) });
  }

  for (const [s, side] of [['L', 'Left'], ['R', 'Right']] as const) {
    const fingerBones = mh.all.filter((n) => n.startsWith('finger') && n.endsWith(`.${s}`));
    add({ name: `${side}Shoulder`, parent: 'Spine2', head: H(`clavicle.${s}`), mh: [`clavicle.${s}`] });
    add({ name: `${side}Arm`, parent: `${side}Shoulder`, head: H(`upperarm01.${s}`), mh: [`shoulder01.${s}`, `upperarm01.${s}`, `upperarm02.${s}`] });
    add({ name: `${side}ForeArm`, parent: `${side}Arm`, head: H(`lowerarm01.${s}`), mh: [`lowerarm01.${s}`, `lowerarm02.${s}`] });
    add({
      name: `${side}Hand`, parent: `${side}ForeArm`, head: H(`wrist.${s}`), tail: o.fingers ? undefined : T(`finger3-1.${s}`),
      mh: [`wrist.${s}`, ...mh.all.filter((n) => n.startsWith('metacarpal') && n.endsWith(`.${s}`)), ...(o.fingers ? [] : fingerBones)],
    });
    if (o.fingers)
      for (const [f, i] of FINGERS) {
        let parent = `${side}Hand`;
        for (let k = 1; k <= 3; k++) {
          const mhb = `finger${i}-${k}.${s}`;
          const name = `${side}Hand${f}${k}`;
          add({ name, parent, head: H(mhb), mh: [mhb] });
          parent = name;
        }
        add({ name: `${side}Hand${f}4`, parent, head: T(`finger${i}-3.${s}`), mh: [] });
      }
    add({ name: `${side}UpLeg`, parent: 'Hips', head: H(`upperleg01.${s}`), mh: [`upperleg01.${s}`, `upperleg02.${s}`] });
    add({ name: `${side}Leg`, parent: `${side}UpLeg`, head: H(`lowerleg01.${s}`), mh: [`lowerleg01.${s}`, `lowerleg02.${s}`] });
    add({ name: `${side}Foot`, parent: `${side}Leg`, head: H(`foot.${s}`), mh: [`foot.${s}`] });
    add({ name: `${side}ToeBase`, parent: `${side}Foot`, head: T(`foot.${s}`), mh: mh.all.filter((n) => n.startsWith('toe') && n.endsWith(`.${s}`)) });
    add({ name: `${side}Toe_End`, parent: `${side}ToeBase`, head: T(`toe3-3.${s}`), mh: [] });
  }
  return bones;
}

/** The full MakeHuman skeleton (163 bones incl. face), weights 1:1. */
export function fullSkeleton(bones: { name: string; parent: string | null; head: string; tail: string }[]): BoneDef[] {
  // Parent-first order (the source file is alphabetical).
  const out: BoneDef[] = [];
  const done = new Set<string>();
  const visit = (b: (typeof bones)[number]) => {
    if (done.has(b.name)) return;
    if (b.parent) visit(bones.find((x) => x.name === b.parent)!);
    done.add(b.name);
    out.push({ name: b.name, parent: b.parent, head: b.head, tail: b.tail, mh: [b.name] });
  };
  bones.forEach(visit);
  return out;
}

export const RIG_LABELS: Record<RigType, string> = {
  game: 'Game rig — Mixamo-compatible (65 bones + weapon socket)',
  game_lite: 'Game rig lite (25 bones + weapon socket, no fingers)',
  full: 'Full MakeHuman rig (163 bones + weapon socket, face)',
  none: 'No rig (static mesh)',
};
