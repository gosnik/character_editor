/**
 * Reference poses from Ara's evaluator for native-rig parity tests.
 *
 * This is an offline oracle only. Rust never evaluates TypeScript at runtime.
 * Run after changing the evaluator, skeleton, or motion baker:
 *   ./vendor/ara/node_modules/.bin/tsx tools/bake_motion_fixtures.ts
 */
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { nodeLibrary } from '../vendor/ara/scripts/node-assets';
import { buildHuman } from '../vendor/ara/src/core/human';
import { DEFAULT_PARAMS } from '../vendor/ara/src/core/params';
import { Animator } from '../vendor/ara/src/core/anim/evaluate';
import { clipById } from '../vendor/ara/src/core/anim/clips';

type Q = [number, number, number, number];
const multiply = (a: Q, b: Q): Q => [
  a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
  a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
  a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
  a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
];

const assets = join(import.meta.dirname, '../assets/native');
const lib = await nodeLibrary(assets);
const character = await buildHuman(lib, {
  ...DEFAULT_PARAMS, rig: 'game', faceBones: true, restPose: 'A', animations: true,
  weapon: 'none', carry: 'none', outfit: 'none', top: 'none', bottom: 'none', jacket: 'none', shoes: 'none', hat: 'none',
});
if (!character.rig) throw new Error('canonical character did not create a rig');
const animator = new Animator(character.rig, character.face);
const joints = ['Hips', 'Spine2', 'Head', 'LeftArm', 'LeftForeArm', 'LeftHand', 'RightArm', 'RightForeArm', 'RightHand', 'LeftUpLeg', 'LeftLeg', 'LeftFoot', 'RightUpLeg', 'RightLeg', 'RightFoot'];
const ids = ['walk', 'run', 'jump', 'turn_left_90', 'turn_right_90', 'rifle_aim', 'sit_idle'];
const phases = [0, 0.25, 0.5, 0.75];
const index = new Map(character.rig.bones.map((bone, i) => [bone.name, i]));
const cases = ids.flatMap((id) => {
  const clip = clipById.get(id);
  if (!clip) throw new Error(`missing clip ${id}`);
  return phases.map((phase) => {
    // Probe exact frames in the 30 Hz table consumed by NativeRig, avoiding
    // an oracle mismatch caused solely by source-vs-runtime interpolation.
    const time = Math.min(clip.duration, Math.round(clip.duration * phase * 30) / 30);
    const frame = animator.frame(clip, time, { rootMotion: true });
    const pos = animator.positions(frame);
    const world: Q[] = [];
    character.rig!.bones.forEach((bone, i) => {
      world[i] = bone.parent < 0 ? frame.rot[i] : multiply(world[bone.parent], frame.rot[i]);
    });
    return {
      clip: id, time, joints: joints.map((name) => {
        const i = index.get(name);
        if (i === undefined) throw new Error(`missing bone ${name}`);
        return { name, position: pos[i], rotation: world[i] };
      }),
    };
  });
});
const tCharacter = await buildHuman(lib, {
  ...DEFAULT_PARAMS, rig: 'game', faceBones: true, restPose: 'T', animations: true,
  weapon: 'none', carry: 'none', outfit: 'none', top: 'none', bottom: 'none', jacket: 'none', shoes: 'none', hat: 'none',
});
if (!tCharacter.rig) throw new Error('canonical T-pose character did not create a rig');
const tAnimator = new Animator(tCharacter.rig, tCharacter.face);
const tIndex = new Map(tCharacter.rig.bones.map((bone, i) => [bone.name, i]));
const tCases = ids.flatMap((id) => {
  const clip = clipById.get(id);
  if (!clip) throw new Error(`missing clip ${id}`);
  return phases.map((phase) => {
    const time = Math.min(clip.duration, Math.round(clip.duration * phase * 30) / 30);
    const frame = tAnimator.frame(clip, time, { rootMotion: true });
    const pos = tAnimator.positions(frame);
    const world: Q[] = [];
    tCharacter.rig!.bones.forEach((bone, i) => { world[i] = bone.parent < 0 ? frame.rot[i] : multiply(world[bone.parent], frame.rot[i]); });
    return { clip: id, time, joints: joints.map((name) => {
      const i = tIndex.get(name);
      if (i === undefined) throw new Error(`missing T bone ${name}`);
      return { name, position: pos[i], rotation: world[i] };
    }) };
  });
});
await writeFile(join(assets, 'motion_reference.json'), JSON.stringify({ version: 1, source_height: character.height, cases, t_cases: tCases }, null, 2) + '\n');
console.log(`wrote ${cases.length} A-pose and ${tCases.length} T-pose native motion reference poses`);
