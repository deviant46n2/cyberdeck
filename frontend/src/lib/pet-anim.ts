/**
 * Idle animation catalogue + scheduler for the sidebar pet.
 *
 * The pet cycles through weighted random "idle" animations so he never sits
 * stone-still. Each animation is a CSS class whose multi-step @keyframes move
 * the layered SVG subgroups (.cp-whole translate, .cp-body squash, .cp-legs
 * shuffle, .cp-eyes dart, .cp-mouth shape). Mood-reactive classes (hot, sleepy,
 * surprised) layer on top.
 */

/** One selectable idle behaviour. `dur` is the animation's own cycle time;
 * the scheduler lets it run 1..n cycles based on `weight` so common ones look
 * frequent and rare ones feel surprising. */
export interface PetAnim {
  id: string;
  /** Relative pick weight (higher = more often). */
  weight: number;
  /** Base cycle duration in ms. */
  ms: number;
  /** How many full cycles to run once picked (1 = play once). */
  cycles: number;
  label: string;
}

/** The idle-behaviour pool. Keep weights summed conceptually; common settling
 * motions out-weigh the show-stoppers. */
export const IDLE_ANIMS: PetAnim[] = [
  { id: "breath", weight: 30, ms: 3600, cycles: 4, label: "breathing" },
  { id: "sway", weight: 18, ms: 2400, cycles: 3, label: "swaying" },
  { id: "dart", weight: 10, ms: 5000, cycles: 1, label: "lookin' around" },
  { id: "wobble", weight: 8, ms: 900, cycles: 2, label: "jigglin'" },
  { id: "stretch", weight: 5, ms: 2200, cycles: 1, label: "stretchin'" },
  { id: "hop", weight: 7, ms: 1400, cycles: 1, label: "hoppin'" },
  { id: "bounce", weight: 6, ms: 2300, cycles: 1, label: "bouncin'" },
  { id: "walkL", weight: 9, ms: 3000, cycles: 2, label: "walkin'" },
  { id: "walkR", weight: 9, ms: 3000, cycles: 2, label: "walkin'" },
  { id: "flip", weight: 2, ms: 1100, cycles: 1, label: "flippin'" },
  { id: "spin", weight: 2, ms: 1500, cycles: 1, label: "spinnin'" },
  { id: "tilt", weight: 7, ms: 2000, cycles: 1, label: "leain'/curious" },
  { id: "sleepy", weight: 5, ms: 4000, cycles: 2, label: "drowsy" },
  { id: "dance", weight: 4, ms: 2900, cycles: 2, label: "dancin'" },
];

export function pickIdleAnim(rand: () => number = Math.random): PetAnim {
  const total = IDLE_ANIMS.reduce((s, a) => s + a.weight, 0);
  let roll = rand() * total;
  for (const a of IDLE_ANIMS) {
    roll -= a.weight;
    if (roll <= 0) return a;
  }
  return IDLE_ANIMS[0];
}

/** CSS class names for the body-layer animations (each drives .cp-whole). */
export function animClass(id: string): string {
  return `cp-anim-${id}`;
}
