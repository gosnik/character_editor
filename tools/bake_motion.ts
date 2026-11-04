/**
 * Offline conversion of Ara's authored animation clips to the compact native
 * runtime table. This is intentionally never called by the Rust library.
 *
 * Run from character_editor:
 *   ./vendor/ara/node_modules/.bin/tsx tools/bake_motion.ts
 */
import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { gzipSync } from 'node:zlib';
import { nodeLibrary } from '../vendor/ara/scripts/node-assets';
import { buildHuman } from '../vendor/ara/src/core/human';
import { DEFAULT_PARAMS } from '../vendor/ara/src/core/params';
import { Animator } from '../vendor/ara/src/core/anim/evaluate';
import { CLIPS } from '../vendor/ara/src/core/anim/clips';
import { quatOf } from '../vendor/ara/src/core/rig/rig';

type Q = [number, number, number, number];
const conjugate = ([x, y, z, w]: Q): Q => [-x, -y, -z, w];
const multiply = (a: Q, b: Q): Q => [
  a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
  a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
  a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
  a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
];
const normalize = (q: Q): Q => { const n = Math.hypot(...q) || 1; return [q[0] / n, q[1] / n, q[2] / n, q[3] / n]; };

const assets = join(import.meta.dirname, '../assets/native');
const lib = await nodeLibrary(assets);
const params = {
  ...DEFAULT_PARAMS,
  rig: 'game',
  faceBones: true,
  animations: true,
  weapon: 'none',
  carry: 'none',
  outfit: 'none', top: 'none', bottom: 'none', jacket: 'none', shoes: 'none', hat: 'none',
};

async function bake(restPose: 'A' | 'T', rig: 'game' | 'full' = 'game'): Promise<Record<string, unknown>> {
  const character = await buildHuman(lib, { ...params, rig, restPose });
  if (!character.rig) throw new Error('canonical character did not create a rig');
  const animator = new Animator(character.rig, character.face);
  // Frames are local, but their authored rest is the animator's relaxed pose.
  // Normalize against the raw local bind frame so Rust can retarget the delta
  // to a character built in the same A or T rest pose.
  const bindWorld = character.rig.bones.map((bone) => quatOf(bone.rot) as Q);
  const bindLocal: Q[] = character.rig.bones.map((bone, index) =>
    bone.parent === -1 ? bindWorld[index] : multiply(conjugate(bindWorld[bone.parent]), bindWorld[index]),
  );
  const sourceScale = character.height / 1.8;
  const clips: Record<string, unknown> = {};
  for (const clip of CLIPS) {
    const baked = animator.bake(clip, 30, { rootMotion: true });
    clips[clip.id] = {
    looped: clip.loop,
    duration: clip.duration,
    bones: character.rig.bones.map((bone) => bone.name),
    times: baked.times,
    // A frame is a local rotation.  Store the change expressed in that
    // local bind frame so runtime can retarget it as `bindLocal * delta`.
    // Quaternion multiplication is not commutative: the old `frame *
    // inverse(bind)` form applied the delta in the parent frame and flipped
    // limbs whenever their bind frame was not axis-aligned.
    rotations: baked.frames.map((frame) => frame.rot.map((rotation, index) => normalize(multiply(conjugate(bindLocal[index]), rotation)))),
    roots: baked.frames.map((frame) => {
      const root = frame.rootPos.get(0) ?? character.rig!.bones[0].head;
      const bind = character.rig!.bones[0].head;
      // Normalize source movement by source body height; runtime restores it
      // using its own morphed body height.
      return [(root[0] - bind[0]) / sourceScale, (root[1] - bind[1]) / sourceScale, (root[2] - bind[2]) / sourceScale];
    }),
  };
}
  return clips;
}
const clips = await bake('A');
const tClips = await bake('T');
const fullClips = await bake('A', 'full');
const fullTClips = await bake('T', 'full');
await mkdir(assets, { recursive: true });
await writeFile(join(assets, 'motion.json.gz'), gzipSync(JSON.stringify({ version: 3, clips, t_clips: tClips, full_clips: fullClips, full_t_clips: fullTClips }), { level: 9 }));
console.log(`baked ${Object.keys(clips).length} clips for game/full A/T poses to assets/native/motion.json.gz`);
