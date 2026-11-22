// The viewer's zoom arithmetic, kept out of the component so it can be run and
// asserted: `node tools/view-zoom-check.js`.
//
// Why this file exists at all: the "Actual" button used to divide by
// `element.offsetWidth` read off a *function reference* (`mediaEl` instead of
// `mediaEl()`), which is `undefined`, so the button set the scale to `NaN%` and
// the picture never moved (2026-11-15 review). Anything that can produce a NaN
// scale belongs in a pure function with a test, not inline in markup.
//
// The model, rewritten on 2026-11-22 after `tools/viewer-layout-probe.js` measured
// the live viewer:
//   * `natural` - the picture's own pixels (from the decoded ImageBitmap);
//   * `box`     - the space the stage has for the picture. It is measured from the
//                 stage's border box, never from the media box: the media box used
//                 to be a flex item sized by its content, so a zoomed picture
//                 raised the stage's scrollbars, which shrank the box, which
//                 changed the fit factor the 1:1 button had just used (a 2400 px
//                 frame was drawn 2367 px wide - 98.6% while the bar said 99%);
//   * `fit`     - the scale that puts the whole picture inside that box. Always
//                 <= 1: "Fit" means the whole picture, never one pixel per pixel;
//   * `scale`   - what is on screen: screen pixels per picture pixel. 1 is 1:1.
//
// It used to be `fit` x `zoom`, with `zoom` applied as a CSS `transform: scale()`.
// Measuring it showed that was wrong twice over: the shrinking box above, and a
// centred transform putting part of a zoomed picture outside the stage's scrollable
// area, so the top-left of a zoomed frame could not be scrolled to at all (scroll
// width 1868 for a picture drawn 2367). So `scale` is now applied as the media box's
// own size in CSS px: 1:1 is exactly one pixel per pixel, and the whole picture
// stays inside the stage's scroll area.

export type Size = { w: number; h: number };

/** Manual zoom floor: the smallest useful shrink, in screen px per picture px. */
export const SCALE_MIN = 0.05;
/** Manual zoom ceiling for + / -: eight screen pixels per picture pixel. */
export const SCALE_MAX = 8;
/** 1:1 in this model, by definition. */
export const ONE_TO_ONE = 1;
/** Past this many CSS px the compositor stops being reliable, so + stops there.
 *  WebView2 caps a single texture around 16384 px, which is where this came from. */
export const MAX_DRAWN_PX = 16384;

const positive = (n: unknown): n is number =>
  typeof n === "number" && Number.isFinite(n) && n > 0;

/**
 * How much the picture has to be shrunk to sit inside the box. 0 means "not
 * computable" (no picture on screen yet, or a box with no size), never "invisible".
 * It never enlarges: a picture smaller than the box is left at its own size, which
 * is why Fit and 1:1 are the same state for such a picture.
 */
export function fitScale(natural: Size | null, box: Size | null): number {
  if (!natural || !box) return 0;
  if (!positive(natural.w) || !positive(natural.h)) return 0;
  if (!positive(box.w) || !positive(box.h)) return 0;
  const s = Math.min(box.w / natural.w, box.h / natural.h, 1);
  return Number.isFinite(s) && s > 0 ? s : 0;
}

/**
 * The scale that puts one picture pixel on one screen pixel: always 1, because
 * scale is measured against the picture, not against the fit.
 */
export function oneToOneScale(): number {
  return ONE_TO_ONE;
}

/**
 * The ceiling for this picture: the ordinary one, lowered when that would lay the
 * picture out past what the compositor can hold. 1:1 is never clamped away, because
 * a button that silently refuses to reach 1:1 is worse than one that stops earlier.
 */
export function scaleCeiling(natural: Size | null): number {
  if (!natural || !positive(natural.w) || !positive(natural.h)) return SCALE_MAX;
  const room = MAX_DRAWN_PX / Math.max(natural.w, natural.h);
  const top = Math.min(SCALE_MAX, room);
  // 1:1 is never clamped away: a frame that wide is laid out at its own size and
  // the stage scrolls to it, even when + cannot go any further.
  return Math.max(ONE_TO_ONE, top);
}

/** Never returns NaN: a bad number falls back to 1:1, which is a known state. */
export function clampScale(scale: number, ceiling: number): number {
  const top = positive(ceiling) ? ceiling : SCALE_MAX;
  if (typeof scale !== "number" || !Number.isFinite(scale)) return Math.min(top, ONE_TO_ONE);
  return Math.min(top, Math.max(SCALE_MIN, scale));
}

/**
 * The box the media is laid out at, in CSS px. null means "nothing to lay out"
 * (no frame, or its own size is not known), and plain CSS containment is left to
 * do the work - which is what a video gets, since its pixel size is not read here.
 */
export function drawnSize(natural: Size | null, scale: number): Size | null {
  const n = natural && positive(natural.w) && positive(natural.h) ? natural : null;
  if (!n) return null;
  const s = typeof scale === "number" && Number.isFinite(scale) && scale > 0 ? scale : ONE_TO_ONE;
  const w = Math.max(1, Math.round(n.w * s));
  const h = Math.max(1, Math.round(n.h * s));
  return { w, h };
}

/**
 * What the bar prints: the picture's own pixels, as a percentage. null = not
 * knowable yet, and the bar says so rather than showing a guess.
 */
export function scalePercent(scale: number): number | null {
  if (typeof scale !== "number" || !Number.isFinite(scale)) return null;
  const pct = Math.round(scale * 100);
  return Number.isFinite(pct) ? pct : null;
}

/** "1:1" is only a reached state if the number really is there. */
export function isOneToOne(scale: number): boolean {
  return typeof scale === "number" && Math.abs(scale - ONE_TO_ONE) < 0.005;
}

/**
 * Is this picture no larger than the space the stage has? Then there is nothing
 * for Fit to shrink and Fit *is* 1:1 - the UI has to say that in words rather than
 * leave two lit buttons to be read as a bug (2026-11-22 report).
 */
export function fitsTheStage(natural: Size | null, box: Size | null): boolean {
  const f = fitScale(natural, box);
  return f >= 0.999;
}
