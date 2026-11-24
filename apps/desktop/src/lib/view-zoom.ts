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
//   * `fit`     - the scale that puts the whole picture inside that box, in whichever
//                 direction that needs: it shrinks a big frame and enlarges a small
//                 one, so pressing Fit always fills the stage (2026-11-23, see below);
//   * `scale`   - what is on screen: screen pixels per picture pixel. 1 is 1:1.
//
// It used to be `fit` x `zoom`, with `zoom` applied as a CSS `transform: scale()`.
// Measuring it showed that was wrong twice over: the shrinking box above, and a
// centred transform putting part of a zoomed picture outside the stage's scrollable
// area, so the top-left of a zoomed frame could not be scrolled to at all (scroll
// width 1868 for a picture drawn 2367). So `scale` is now applied as the media box's
// own size in CSS px: 1:1 is exactly one pixel per pixel, and the whole picture
// stays inside the stage's scroll area.
//
// What was wrong on 2026-11-23, measured by `tools/viewer-real-media-probe.js` on
// the user's own bookmarked material: `fit` was capped at 1 ("never enlarge"), and
// that is the bug the report is about. A 1200x850 jpeg in a 1888x1105 stage was laid
// out at 1200x850 - 64% x 77% of the box - with Fit and 1:1 both lit and pressing Fit
// doing nothing at all; a 176x256 gif was laid out at 176x256, 23% of the box. So
// "Fit" was only a fit for pictures bigger than the window, which is most of an
// imageboard library (gifs, short videos, smaller gallery pages). Fit now enlarges as
// well as shrinks; the drawn box is still bounded by the stage, so the upscale factor
// can never lay out more pixels than the window has.

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
 * How much the picture has to be scaled to sit inside the box: shrunk when it is
 * bigger than the space the stage has, enlarged when it is smaller. 0 means "not
 * computable" (no picture on screen yet, or a box with no size), never "invisible".
 * The enlargement is bounded by the box, so it can never lay out more pixels than
 * the window can show - that is what makes "Fit" safe to let past 100%.
 */
export function fitScale(natural: Size | null, box: Size | null): number {
  if (!natural || !box) return 0;
  if (!positive(natural.w) || !positive(natural.h)) return 0;
  if (!positive(box.w) || !positive(box.h)) return 0;
  const s = Math.min(box.w / natural.w, box.h / natural.h);
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
 * (no frame on screen yet, or its own size is not known), and plain CSS containment
 * is left to do the work until it is.
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
 * Does "Fit" land exactly on 1:1 for this picture in this box? That can only happen
 * when the stage is already the picture's own size in whichever direction binds, so
 * it is rare, and the info line says it in words rather than leaving two lit buttons
 * to be read as a bug (2026-11-22 report).
 */
export function fitIsOneToOne(natural: Size | null, box: Size | null): boolean {
  const f = fitScale(natural, box);
  return Number.isFinite(f) && Math.abs(f - ONE_TO_ONE) < 0.005;
}

/**
 * Does "Fit" have to enlarge this picture to fill the stage? Then the bar prints a
 * number above 100%, and it has to say so: a 299% readout is the honest answer for a
 * gif smaller than the window, not a broken zoom.
 */
export function fitEnlarges(natural: Size | null, box: Size | null): boolean {
  return fitScale(natural, box) > ONE_TO_ONE + 0.005;
}
