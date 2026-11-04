/** Walk and run engine (shared by the locomotion and shooter clips), strafes and turns. */
import { type Ch, type Clip, type Sample } from './types';
import { cyc, fingers, side, smoother } from './helpers';

export interface Gait {
  thigh: number[];
  knee: number[];
  foot: number[];
  arm: number[];
  elbow: number[];
  bob: number[]; // metres, 8 keys over the full cycle (two steps)
  pelvisYaw: number;
  spineLean: number;
  armOut: number;
  legIn: number;
}

export const WALK: Gait = {
  thigh: [25, 18, 6, -8, -18, -8, 12, 24],
  knee: [-5, -15, -8, -5, -22, -58, -40, -10],
  foot: [15, 0, 0, -6, -20, -10, 5, 10],
  arm: [-18, -10, 0, 10, 18, 10, 0, -10],
  elbow: [10, 11, 14, 20, 25, 20, 14, 11],
  bob: [0, -0.018, -0.004, 0.012, 0, -0.018, -0.004, 0.012],
  pelvisYaw: 5,
  spineLean: 3,
  armOut: 2,
  legIn: 5,
};

export const RUN: Gait = {
  thigh: [30, 12, -10, -25, -5, 35, 55, 42],
  knee: [-20, -42, -25, -18, -85, -110, -72, -35],
  foot: [10, 0, -12, -30, -20, 0, 15, 15],
  arm: [-32, -18, 4, 22, 28, 18, -4, -26],
  elbow: [72, 74, 80, 86, 88, 84, 78, 74],
  bob: [0, -0.04, -0.015, 0.035, 0, -0.04, -0.015, 0.035],
  pelvisYaw: 9,
  spineLean: 11,
  armOut: 6,
  legIn: 4,
};

export function gait(g: Gait, p: number, reverse = false): Sample {
  const ph = reverse ? 1 - p : p;
  const c = (keys: number[]) => cyc(keys);
  const thigh = c(g.thigh), knee = c(g.knee), foot = c(g.foot), arm = c(g.arm), elbow = c(g.elbow), bob = c(g.bob);
  const out: Record<string, Ch> = {};
  const L = ph, R = ph + 0.5;
  // Thighs slightly adducted so the feet track under the body (the bind pose stands wide).
  side(out, 'UpLeg', [thigh(L), 0, -g.legIn], [thigh(R), 0, -g.legIn]);
  side(out, 'Leg', [knee(L), 0, 0], [knee(R), 0, 0]);
  side(out, 'Foot', [foot(L), 0, g.legIn * 0.8], [foot(R), 0, g.legIn * 0.8]);
  side(out, 'Arm', [arm(L), 0, g.armOut], [arm(R), 0, g.armOut]);
  side(out, 'ForeArm', [elbow(L), 0, 0], [elbow(R), 0, 0]);
  const twist = Math.sin(ph * Math.PI * 2) * g.pelvisYaw;
  out.Hips = [0, -twist, Math.sin(ph * Math.PI * 2 + Math.PI / 2) * 2];
  out.Spine = [g.spineLean * 0.4 * (reverse ? -1 : 1), twist * 0.6, 0];
  out.Spine1 = [g.spineLean * 0.4 * (reverse ? -1 : 1), twist * 0.6, 0];
  out.Spine2 = [g.spineLean * 0.2, twist * 0.2, 0];
  out.Head = [-g.spineLean * 0.6, -twist * 0.3, 0];
  fingers(out, g === RUN ? 25 : 12);
  // Walking keeps the stance foot planted; running has a flight phase so it uses its own bob.
  return { bones: out, root: g === RUN ? { y: bob(ph) } : { ground: true } };
}

export function turn(id: string, name: string, angle: number): Clip {
  const steps = Math.max(1, Math.round(Math.abs(angle) / 45));
  const duration = 0.55 + Math.abs(angle) / 180 * 0.85;
  const leadSide = angle > 0 ? 'Left' : 'Right';
  const other = angle > 0 ? 'Right' : 'Left';
  return {
    id, name, category: 'Turns', duration, loop: false,
    sample: (t) => {
      const u = t / duration;
      const turnAmt = smoother((u - 0.05) / 0.85);
      const lead = Math.sin(Math.min(1, u / 0.6) * Math.PI); // upper body leads the turn
      const out: Record<string, Ch> = {
        Spine1: [2, angle * 0.06 * lead, 0],
        Spine2: [0, angle * 0.05 * lead, 0],
        Head: [0, Math.max(-25, Math.min(25, angle * 0.18 * lead)), 0],
      };
      // Alternating steps, starting with the leg on the turning side.
      let bob = 0;
      const lift: Record<string, number> = { Left: 0, Right: 0 };
      for (let i = 0; i < steps; i++) {
        const a = 0.05 + (i / steps) * 0.85, b = 0.05 + ((i + 1) / steps) * 0.85;
        if (u > a && u < b) {
          const s = Math.sin(((u - a) / (b - a)) * Math.PI);
          lift[i % 2 === 0 ? leadSide : other] = s;
          bob = -0.012 * s;
        }
      }
      side(out, 'UpLeg', [24 * lift.Left, 0, 0], [24 * lift.Right, 0, 0]);
      side(out, 'Leg', [-45 * lift.Left - 4, 0, 0], [-45 * lift.Right - 4, 0, 0]);
      side(out, 'Foot', [12 * lift.Left, 0, 0], [12 * lift.Right, 0, 0]);
      side(out, 'Arm', [6 * lift.Right, 0, 4], [6 * lift.Left, 0, 4]);
      side(out, 'ForeArm', [14, 0, 0], [14, 0, 0]);
      fingers(out, 14);
      return { bones: out, root: { yaw: angle * turnAmt, y: bob, ground: true } };
    },
  };
}

export function strafe(dirLeft: boolean): Clip {
  const lead = dirLeft ? 'Left' : 'Right', trail = dirLeft ? 'Right' : 'Left';
  return {
    id: dirLeft ? 'strafe_left' : 'strafe_right', name: dirLeft ? 'Strafe left' : 'Strafe right', category: 'Locomotion',
    duration: 0.9, loop: true, travel: [dirLeft ? 1.1 : -1.1, 0, 0],
    sample: (t) => {
      const p = t / 0.9;
      const out: Record<string, Ch> = {};
      const legs: Record<string, { out: number; thigh: number; knee: number }> = {
        [lead]: { out: cyc([0, 10, 16, 12, 8, 6, 3, 0])(p), thigh: cyc([0, 10, 8, 0, 0, 0, 0, 0])(p), knee: cyc([-4, -28, -20, -6, -4, -4, -4, -4])(p) },
        [trail]: { out: cyc([0, -2, -4, -5, -7, -2, 3, 2])(p), thigh: cyc([0, 0, 0, 0, 6, 12, 6, 0])(p), knee: cyc([-4, -4, -4, -4, -8, -28, -20, -6])(p) },
      };
      side(out, 'UpLeg', [legs.Left.thigh, 0, legs.Left.out], [legs.Right.thigh, 0, legs.Right.out]);
      side(out, 'Leg', [legs.Left.knee, 0, 0], [legs.Right.knee, 0, 0]);
      side(out, 'Foot', [-legs.Left.knee * 0.3, 0, -legs.Left.out * 0.5], [-legs.Right.knee * 0.3, 0, -legs.Right.out * 0.5]);
      side(out, 'Arm', [8, 0, 6], [8, 0, 6]);
      side(out, 'ForeArm', [25, 0, 0], [25, 0, 0]);
      out.Spine = [4, 0, (dirLeft ? -1 : 1) * 2 * Math.sin(p * Math.PI * 2)];
      out.Head = [-2, 0, 0];
      fingers(out, 14);
      return { bones: out, root: { ground: true } };
    },
  };
}
