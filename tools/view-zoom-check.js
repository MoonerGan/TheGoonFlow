// Runs the viewer's zoom arithmetic for real. There is no JS test runner in this
// repo, so this is a plain node script: `node tools/view-zoom-check.js`.
//
// It exists because of the 2026-11-15 review: the Actual button produced
// `NaN%` and the picture did not move. Node 23+ strips the types from the .ts
// import, so this checks the exact file the component uses.
import {
  ZOOM_MAX,
  clampZoom,
  fitScale,
  isFitted,
  isOneToOne,
  onScreenPercent,
  oneToOneZoom,
  zoomCeiling,
} from "../apps/desktop/src/lib/view-zoom.ts";

let fails = 0;
const eq = (got, want, what) => {
  const ok = Object.is(got, want);
  if (!ok) fails += 1;
  console.log(`${ok ? "ok  " : "FAIL"} ${what}: got ${got}, want ${want}`);
};
const near = (got, want, what, tol = 0.01) => {
  const ok = Number.isFinite(got) && Math.abs(got - want) <= tol;
  if (!ok) fails += 1;
  console.log(`${ok ? "ok  " : "FAIL"} ${what}: got ${got}, want ~${want}`);
};

// The picture in the review screenshot: 6000x4000 PNG, and the stage could show
// it at about 20% of its own pixels.
const big = { w: 6000, h: 4000 };
const stage = { w: 1240, h: 800 };

near(fitScale(big, stage), 0.2, "fit of a 6000x4000 frame in a 1240x800 stage");
near(oneToOneZoom(big, stage), 5, "zoom that reaches 1:1 for that frame");
eq(onScreenPercent(1, big, stage), 20, "what the bar must say at fit (was 100%)");
eq(onScreenPercent(5, big, stage), 100, "what the bar must say at 1:1");
eq(isOneToOne(5, big, stage), true, "5x of that fit is 1:1");
eq(isOneToOne(1, big, stage), false, "fit is not 1:1 for a picture larger than the stage");
eq(isFitted(1), true, "zoom 1 is the fitted state");

// A picture smaller than the stage: fitting does not upscale it, so fit is 1 and
// fit and 1:1 are the same state.
const small = { w: 800, h: 500 };
eq(fitScale(small, stage), 1, "a small picture is not upscaled to fit");
eq(oneToOneZoom(small, stage), 1, "for it, fit and 1:1 are the same zoom");
eq(onScreenPercent(1, small, stage), 100, "and the bar says 100% honestly");

// 1:1 must always be reachable, even when it needs more than the ordinary cap.
const huge = { w: 40000, h: 30000 };
const tiny = { w: 600, h: 400 };
const need = oneToOneZoom(huge, tiny);
eq(zoomCeiling(huge, tiny) >= need, true, "the ceiling is raised so 1:1 is reachable");
eq(clampZoom(need, zoomCeiling(huge, tiny)) === need, true, "and 1:1 is not clamped away");
eq(zoomCeiling(big, stage), ZOOM_MAX, "an ordinary picture keeps the ordinary ceiling");

// The bug class itself: nothing may produce NaN, whatever comes in.
const junk = [
  [null, null],
  [big, null],
  [null, stage],
  [{ w: 0, h: 0 }, stage],
  [big, { w: 0, h: 0 }],
  [{ w: NaN, h: 10 }, stage],
  [big, { w: Infinity, h: NaN }],
];
for (const [n, b] of junk) {
  const label = `degenerate input ${JSON.stringify(n)} / ${JSON.stringify(b)}`;
  eq(Number.isFinite(fitScale(n, b)), true, `${label}: fit`);
  eq(Number.isFinite(oneToOneZoom(n, b)), true, `${label}: 1:1`);
  eq(Number.isFinite(zoomCeiling(n, b)), true, `${label}: ceiling`);
  const p = onScreenPercent(1, n, b);
  eq(p === null || Number.isFinite(p), true, `${label}: percent`);
}
// The old shape of the bug: a divisor that is undefined instead of a number.
eq(Number.isNaN(clampZoom((big.w * 1) / undefined, ZOOM_MAX)), false, "NaN zoom falls back to fit");
eq(clampZoom((big.w * 1) / undefined, ZOOM_MAX), 1, "NaN zoom falls back to fit (value)");
eq(clampZoom(NaN, ZOOM_MAX), 1, "explicit NaN falls back to fit");
eq(clampZoom(1000, ZOOM_MAX), ZOOM_MAX, "an absurd zoom is capped");
eq(clampZoom(0.001, ZOOM_MAX), 0.2, "an absurd shrink is floored");

console.log(fails === 0 ? "\nview-zoom: ALL PASS" : `\nview-zoom: ${fails} FAILED`);
process.exit(fails === 0 ? 0 : 1);
