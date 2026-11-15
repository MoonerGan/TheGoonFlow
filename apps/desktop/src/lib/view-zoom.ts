// The viewer's zoom arithmetic, kept out of the component so it can be run and
// asserted: `node tools/view-zoom-check.js`.
//
// Why this file exists at all: the "Actual" button used to divide by
// `element.offsetWidth` read off a *function reference* (`mediaEl` instead of
// `mediaEl()`), which is `undefined`, so the button set the scale to `NaN%` and
// the picture never moved (2026-11-15 review). Anything that can produce a NaN
// scale belongs in a pure function with a test, not inline in markup.
//
// The model:
//   * `natural` — the picture's own pixels (from the decoded ImageBitmap).
//   * `box`     — the box the picture is laid out in, before any transform.
//   * `fit`     — the fraction of one picture pixel that lands on one screen
//                 pixel at zoom 1. Always <= 1: zoom 1 means "the whole picture
//                 fits", never "one pixel per pixel".
//   * zoom      — the transform on top of that fit.
//
// So the honest readout is `zoom * fit`: the picture in the screenshot was
// 6000x4000 in a stage that could show it at 20% of its own pixels, and the bar
// said "100%" — which is how "Actual" ended up looking like it did nothing.

export type Size = { w: number; h: number };

/** Zoom floor: the smallest useful shrink. */
export const ZOOM_MIN = 0.2;
/** Normal ceiling for + / -, so the buttons cannot lose the picture in a blur. */
export const ZOOM_MAX = 8;

const positive = (n: unknown): n is number =>
  typeof n === "number" && Number.isFinite(n) && n > 0;

/**
 * How much the picture was already shrunk to fit. 0 means "not computable"
 * (no picture on screen yet, or a box with no size), never "invisible".
 */
export function fitScale(natural: Size | null, box: Size | null): number {
  if (!natural || !box) return 0;
  if (!positive(natural.w) || !positive(natural.h)) return 0;
  if (!positive(box.w) || !positive(box.h)) return 0;
  const s = Math.min(box.w / natural.w, box.h / natural.h, 1);
  return Number.isFinite(s) && s > 0 ? s : 0;
}

/**
 * The zoom that puts one picture pixel on one screen pixel. 0 when it cannot be
 * known; the UI says so instead of pretending.
 */
export function oneToOneZoom(natural: Size | null, box: Size | null): number {
  const f = fitScale(natural, box);
  if (f <= 0) return 0;
  const z = 1 / f;
  return Number.isFinite(z) && z > 0 ? z : 0;
}

/**
 * The ceiling for this picture: the ordinary ZOOM_MAX, raised when 1:1 needs
 * more than that, because a button that silently refuses to reach 1:1 is worse
 * than one that allows a big number.
 */
export function zoomCeiling(natural: Size | null, box: Size | null): number {
  const need = oneToOneZoom(natural, box);
  return Math.max(ZOOM_MAX, Math.ceil(need) || ZOOM_MAX);
}

/** Never returns NaN: a bad number falls back to fit, which is a known state. */
export function clampZoom(z: number, ceiling: number): number {
  const top = positive(ceiling) ? ceiling : ZOOM_MAX;
  if (typeof z !== "number" || !Number.isFinite(z)) return 1;
  return Math.min(top, Math.max(ZOOM_MIN, z));
}

/**
 * What the user actually sees, as a percentage of the picture's own pixels.
 * null = not knowable yet, and the bar says so rather than showing a guess.
 */
export function onScreenPercent(zoom: number, natural: Size | null, box: Size | null): number | null {
  const f = fitScale(natural, box);
  if (f <= 0 || typeof zoom !== "number" || !Number.isFinite(zoom)) return null;
  const pct = Math.round(zoom * f * 100);
  return Number.isFinite(pct) ? pct : null;
}

/** "Fit" and "1:1" are only *reached* states if the number really is there. */
export function isFitted(zoom: number): boolean {
  return Math.abs(zoom - 1) < 0.005;
}

export function isOneToOne(zoom: number, natural: Size | null, box: Size | null): boolean {
  const target = oneToOneZoom(natural, box);
  if (target <= 0) return false;
  const pct = onScreenPercent(zoom, natural, box);
  return pct !== null && Math.abs(pct - 100) <= 1;
}
