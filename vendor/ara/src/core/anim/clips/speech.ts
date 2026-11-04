/**
 * A fixed stretch of babble for the Talk clip: phrases of consonant + vowel syllables with pauses,
 * as viseme events. The mouth (faces.ts) blends the events; the head (emotes.ts) nods on stressed
 * syllables. Rest at both ends, so the loop closes.
 */
export const TALK_LEN = 6.4;

interface SpeechEvent {
  shape: string;
  start: number;
  end: number;
  weight: number;
  stress: boolean;
}

const CONSONANTS = ['viseme_PP', 'viseme_FF', 'viseme_TH', 'viseme_DD', 'viseme_kk', 'viseme_CH', 'viseme_SS', 'viseme_nn', 'viseme_RR'];
// Vowels by how often they occur in speech.
const VOWELS: [string, number][] = [['viseme_aa', 0.3], ['viseme_E', 0.25], ['viseme_I', 0.2], ['viseme_O', 0.15], ['viseme_U', 0.1]];

function schedule(): SpeechEvent[] {
  let seed = 7;
  const rnd = () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646;
  const events: SpeechEvent[] = [];
  const phrases = [5, 3, 6, 4];
  let t = 0.35;
  phrases.forEach((n, p) => {
    for (let k = 0; k < n; k++) {
      const stress = k === 0 || rnd() < 0.25;
      const c = CONSONANTS[Math.floor(rnd() * CONSONANTS.length)];
      let r = rnd(), vw = VOWELS[0][0];
      for (const [name, f] of VOWELS) if ((r -= f) < 0) { vw = name; break; }
      const cl = 0.07 + rnd() * 0.03, vl = 0.12 + rnd() * 0.06 + (stress ? 0.04 : 0);
      events.push({ shape: c, start: t, end: t + cl, weight: c === 'viseme_PP' ? 1 : 0.85, stress: false });
      t += cl;
      events.push({ shape: vw, start: t, end: t + vl, weight: stress ? 0.95 : 0.75, stress });
      t += vl;
    }
    if (p < phrases.length - 1) t += 0.4 + rnd() * 0.15;
  });
  // Fit the speech into the loop, leaving rest at the end.
  const k = (TALK_LEN - 0.55) / t;
  return events.map((e) => ({ ...e, start: e.start * k, end: e.end * k }));
}

const EVENTS = schedule();
const RAMP = 0.06;
const smooth = (x: number) => (x <= 0 ? 0 : x >= 1 ? 1 : x * x * (3 - 2 * x));
/** 0..1 envelope of an event: eases in before it starts and out after it ends (sounds overlap). */
const bump = (t: number, e: SpeechEvent) => Math.min(smooth((t - e.start + RAMP) / (2 * RAMP)), smooth((e.end + RAMP - t) / (2 * RAMP)));

/** Viseme weights at time t. */
export function speechFace(t: number): Partial<Record<string, number>> {
  const out: Partial<Record<string, number>> = {};
  for (const e of EVENTS) {
    if (t < e.start - RAMP || t > e.end + RAMP) continue;
    const w = e.weight * bump(t, e);
    out[e.shape] = Math.max(out[e.shape] ?? 0, w);
  }
  return out;
}

/** 0..1 emphasis at time t (stressed syllables), with a longer tail for head nods and brows. */
export function speechStress(t: number): number {
  let s = 0;
  for (const e of EVENTS) {
    if (!e.stress) continue;
    // Rises from just before the syllable, peaks early, settles over about half a second.
    const x = (t - e.start + 0.08) / 0.55;
    if (x > 0 && x < 1) s = Math.max(s, Math.sin(Math.sqrt(x) * Math.PI));
  }
  return s;
}
