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
//
// It changed again on 2026-11-23, after `tools/viewer-real-media-probe.js` measured
// the viewer on the user's own bookmarked material: `fit` was capped at 1, so "Fit"
// did nothing at all for anything smaller than the window (see fitScale).
import {
  MAX_DRAWN_PX,
  SCALE_MAX,
  SCALE_MIN,
  clampScale,
  drawnSize,
  fitEnlarges,
  fitIsOneToOne,
  fitScale,
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

// The cases the 2026-11-23 report is about, measured in the live viewer on the
// user's own downloads: a 1200x850 gelbooru jpeg in a 1888x1105 stage (Fit drew it
// 1200x850 - 64% x 77% of the box - with Fit and 1:1 both lit), and a 176x256 gif in
// the same stage (drew 176x256, 23% of the box). Fit now fills the box in whichever
// direction binds, and the bar prints the number it used.
const wideStage = { w: 1888, h: 1105 };
const jpeg = { w: 1200, h: 850 };
const gif = { w: 176, h: 256 };
near(fitScale(jpeg, wideStage), 1.3, "the 1200x850 jpeg in a 1888x1105 stage is enlarged by 1.3");
eq(JSON.stringify(drawnSize(jpeg, fitScale(jpeg, wideStage))), JSON.stringify({ w: 1560, h: 1105 }),
  "and Fit lays it out filling the height of that stage");
eq(scalePercent(fitScale(jpeg, wideStage)), 130, "which the bar prints as 130%, not 100%");
near(fitScale(gif, wideStage), 4.316, "the 176x256 gif in that stage is enlarged by 4.3");
eq(JSON.stringify(drawnSize(gif, fitScale(gif, wideStage))), JSON.stringify({ w: 760, h: 1105 }),
  "so it is laid out 760x1105 rather than a 176x256 stamp");
eq(fitEnlarges(gif, wideStage), true, "which the info line calls enlarging, not zooming");
eq(fitEnlarges(big, stage), false, "and a frame larger than the stage is not enlargement");
eq(fitEnlarges(jpeg, wideStage), true, "nor is the smaller-than-the-window jpeg");
eq(fitIsOneToOne(gif, wideStage), false, "so Fit is not 1:1 there either");
eq(fitIsOneToOne(big, stage), false, "nor for the big frame");
eq(fitIsOneToOne({ w: 1348, h: 723 }, stage), true, "Fit is 1:1 only when the stage is the picture's own size");

// Fit may never lay a picture out bigger than the space the stage has, whatever the
// numbers: that is what makes letting it past 100% safe.
for (const n of [big, jpeg, gif, { w: 8, h: 8 }, { w: 1, h: 4000 }, { w: 9999, h: 3 }, wideStage]) {
  const d = drawnSize(n, fitScale(n, wideStage));
  const ok = d && d.w <= wideStage.w + 1 && d.h <= wideStage.h + 1;
  if (!ok) fails += 1;
  console.log(`${ok ? "ok  " : "FAIL"} fit of ${n.w}x${n.h} in ${wideStage.w}x${wideStage.h} stays inside the box: drew ${d && d.w}x${d && d.h}`);
}

// Fit can end up well past the manual + ceiling for a tiny stamp in a big window, and
// pressing - after Fit has to be one step out, not a jump: the component's ceiling is
// max(scaleCeiling, fit).
const tiny = { w: 8, h: 8 };
near(fitScale(tiny, wideStage), 138.125, "a tiny stamp is enlarged to fill the box");
eq(scaleCeiling(tiny), SCALE_MAX, "while its manual ceiling stays the ordinary one");
near(Math.max(scaleCeiling(tiny), fitScale(tiny, wideStage)), fitScale(tiny, wideStage),
  "and the ceiling used in the viewer never drops below where Fit already put the picture");

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
  eq(fitEnlarges(n, b), false, `${label}: not reported as enlarging`);
  eq(fitIsOneToOne(n, b), false, `${label}: not reported as Fit being 1:1`);
}

// The 2026-11-22 shape of the bug: `transferFromImageBitmap` detaches the bitmap,
// and a detached ImageBitmap reports 0x0, so reading the size *after* the transfer
// handed the viewer a 0x0 "picture". Nothing may be printed or laid out from that:
// fit is 0, there is no drawn size, and the UI has to say "not known" instead of
// `0x0 px, shown at ?%`.
eq(fitScale({ w: 0, h: 0 }, stage), 0, "a detached bitmap's 0x0 is not a fit");
eq(drawnSize({ w: 0, h: 0 }, 1), null, "and it lays out nothing");
eq(scalePercent(fitScale({ w: 0, h: 0 }, stage)), 0, "fit 0 prints 0, and the bar checks sized first");
eq(fitIsOneToOne({ w: 0, h: 0 }, stage), false, "and a 0x0 frame is not 'Fit is 1:1'");

// The old shape of the bug: a divisor that is undefined instead of a number.
eq(Number.isNaN(clampScale(big.w / undefined, SCALE_MAX)), false, "NaN scale falls back");
eq(clampScale(big.w / undefined, SCALE_MAX), 1, "NaN scale falls back to 1:1 (value)");
eq(clampScale(NaN, SCALE_MAX), 1, "explicit NaN falls back to 1:1");
eq(clampScale(1000, SCALE_MAX), SCALE_MAX, "an absurd scale is capped");
eq(clampScale(0.0001, SCALE_MAX), SCALE_MIN, "an absurd shrink is floored");

console.log(fails === 0 ? "\nview-zoom: ALL PASS" : `\nview-zoom: ${fails} FAILED`);
process.exit(fails === 0 ? 0 : 1);
