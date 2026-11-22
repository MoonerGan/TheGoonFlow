// Runs the viewer's zoom arithmetic for real. There is no JS test runner in this
// repo, so this is a plain node script: `node tools/view-zoom-check.js`.
//
// It exists because of the 2026-11-15 review: the Actual button produced
// `NaN%` and the picture did not move. Node 23+ strips the types from the .ts
// import, so this checks the exact file the component uses.
//
// The model changed on 2026-11-22 (see the header of view-zoom.ts): scale is now
// screen pixels per picture pixel, applied as the media box's own size, so fit is
// only the state "Fit" picks and 1:1 is scale 1 whatever the picture is.
import {
  MAX_DRAWN_PX,
  SCALE_MAX,
  SCALE_MIN,
  clampScale,
  drawnSize,
  fitScale,
  fitsTheStage,
  isOneToOne,
  oneToOneScale,
  scaleCeiling,
  scalePercent,
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

// The picture from the 2026-11-22 live probe: 2400x1600 in a stage with 1348x723.
const big = { w: 2400, h: 1600 };
const stage = { w: 1348, h: 723 };

near(fitScale(big, stage), 0.452, "fit of a 2400x1600 frame in a 1348x723 box");
eq(scalePercent(fitScale(big, stage)), 45, "what the bar says at fit");
eq(scalePercent(oneToOneScale()), 100, "what the bar says at 1:1");
eq(isOneToOne(1), true, "scale 1 is 1:1 for any picture");
eq(isOneToOne(fitScale(big, stage)), false, "fit is not 1:1 for a picture larger than the box");
eq(drawnSize(null, 1), null, "drawnSize lays out nothing without a picture");
eq(JSON.stringify(drawnSize(big, 1)), JSON.stringify(big), "1:1 lays the picture out at its own pixels");
eq(JSON.stringify(drawnSize(big, fitScale(big, stage))), JSON.stringify({ w: 1085, h: 723 }),
  "fit lays it out at 1085x723, the number the live stage drew");

// A picture smaller than the box: fitting does not upscale it, so fit is 1 and
// Fit and 1:1 are the same state - and the UI has to say so.
const small = { w: 600, h: 400 };
eq(fitScale(small, stage), 1, "a small picture is not upscaled to fit");
eq(fitsTheStage(small, stage), true, "so it is reported as fitting the stage");
eq(fitsTheStage(big, stage), false, "and the big one is not");
eq(JSON.stringify(drawnSize(small, fitScale(small, stage))), JSON.stringify(small),
  "and Fit draws it at its own size");

// Zoom past 1:1 stops at the compositor limit, but 1:1 itself is never clamped
// away: a frame wider than that limit is still drawn at exactly its own pixels,
// and + simply stops there.
const huge = { w: 40000, h: 30000 };
eq(scaleCeiling({ w: 1000, h: 800 }), SCALE_MAX, "an ordinary small picture keeps the ordinary ceiling");
near(scaleCeiling(big), MAX_DRAWN_PX / 2400, "a 2400 px frame is capped at the drawn-px limit, not 8x");
eq(scaleCeiling(huge), oneToOneScale(), "a frame past the limit still reaches exactly 1:1, and stops there");
eq(clampScale(4, scaleCeiling(huge)) === 1, true, "and + cannot push it past its own pixels");
eq(JSON.stringify(drawnSize(huge, scaleCeiling(huge))), JSON.stringify(huge), "1:1 of it is its own pixel count");
eq(scaleCeiling(null), SCALE_MAX, "no picture: ordinary ceiling");

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
  eq(Number.isFinite(scaleCeiling(n)), true, `${label}: ceiling`);
  eq(drawnSize(n, 1) === null || Number.isFinite(drawnSize(n, 1).w), true, `${label}: drawn size`);
  const p = scalePercent(fitScale(n, b));
  eq(p === null || Number.isFinite(p), true, `${label}: percent`);
  eq(Number.isFinite(clampScale(fitScale(n, b) / 0, scaleCeiling(n))), true, `${label}: clamped scale`);
}

// The 2026-11-22 shape of the bug: `transferFromImageBitmap` detaches the bitmap,
// and a detached ImageBitmap reports 0x0, so reading the size *after* the transfer
// handed the viewer a 0x0 "picture". Nothing may be printed or laid out from that:
// fit is 0, there is no drawn size, and the UI has to say "not known" instead of
// `0x0 px, shown at ?%`.
eq(fitScale({ w: 0, h: 0 }, stage), 0, "a detached bitmap's 0x0 is not a fit");
eq(drawnSize({ w: 0, h: 0 }, 1), null, "and it lays out nothing");
eq(scalePercent(fitScale({ w: 0, h: 0 }, stage)), 0, "fit 0 prints 0, and the bar checks sized first");
eq(fitsTheStage({ w: 0, h: 0 }, stage), false, "and a 0x0 frame is not 'fits the stage'");

// The old shape of the bug: a divisor that is undefined instead of a number.
eq(Number.isNaN(clampScale(big.w / undefined, SCALE_MAX)), false, "NaN scale falls back");
eq(clampScale(big.w / undefined, SCALE_MAX), 1, "NaN scale falls back to 1:1 (value)");
eq(clampScale(NaN, SCALE_MAX), 1, "explicit NaN falls back to 1:1");
eq(clampScale(1000, SCALE_MAX), SCALE_MAX, "an absurd scale is capped");
eq(clampScale(0.0001, SCALE_MAX), SCALE_MIN, "an absurd shrink is floored");

console.log(fails === 0 ? "\nview-zoom: ALL PASS" : `\nview-zoom: ${fails} FAILED`);
process.exit(fails === 0 ? 0 : 1);
