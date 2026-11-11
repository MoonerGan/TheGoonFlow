// Did a rewrite lose a working control? Compare one file across two revisions:
// every `await someApi(...)` call and every `onclick={handler}` button.
//
//   node tools/lost-calls.js apps/desktop/src/components/ItemEditor.svelte 7bceeea HEAD
//
// A UI rework (#43, #45) is where functionality quietly disappears: the views
// still compile, svelte-check is clean, and one button is just gone. This is a
// 20-second check that catches exactly that. Renamed handlers show up as
// lost+new pairs, which is the honest answer - read them, do not suppress them.
const cp = require("node:child_process");
const GIT = "C:/Program Files/Git/bin/git.exe";
const file = process.argv[2];
const [revA, revB] = [process.argv[3], process.argv[4]];
if (!file || !revA || !revB) {
  console.error("usage: node tools/lost-calls.js <path> <revA> <revB>");
  process.exit(2);
}
const src = (rev) =>
  cp.execSync(`"${GIT}" show ${rev}:${file}`, { maxBuffer: 1e8 }).toString();
const calls = (s) =>
  new Set([...s.matchAll(/await ([a-zA-Z_]+)\(/g)].map((m) => m[1]));
const btns = (s) => {
  // every handler name invoked from a click/keydown handler, so
  // `onclick={() => move(-1)}` counts as `move` and a dropped control shows up
  const out = new Set();
  for (const m of s.matchAll(/on(?:click|keydown|change)=\{([^}]*)\}/g))
    for (const n of m[1].matchAll(/\b([a-zA-Z_][a-zA-Z0-9_]*)\(/g)) out.add(n[1]);
  return out;
};
const a = src(revA), b = src(revB);
const ca = calls(a), cb = calls(b);
const ba = btns(a), bb = btns(b);
const lost = (x, y) => [...x].filter((k) => !y.has(k));
const added = (x, y) => [...y].filter((k) => !x.has(k));
console.log(`${file}  ${revA} -> ${revB}`);
console.log(`  api calls  ${ca.size} -> ${cb.size}`);
console.log(`    lost: ${lost(ca, cb).join(", ") || "(none)"}`);
console.log(`    new:  ${added(ca, cb).join(", ") || "(none)"}`);
console.log(`  buttons    ${ba.size} -> ${bb.size}`);
console.log(`    lost: ${lost(ba, bb).join(", ") || "(none)"}`);
console.log(`    new:  ${added(ba, bb).join(", ") || "(none)"}`);
// A name that vanished from the call sites but still exists in the file was
// renamed or moved (a prop, a wrapper), which is fine and worth printing. The
// other heuristic: `lockVault` -> `lockVaultWithErase` is a wrapper, not a loss,
// so a lost name that is a substring of an added one counts as moved too.
const still = (k) => new RegExp(`\\b${k}\\b`).test(b);
const grown = (k, added) => added.some((x) => x !== k && x.includes(k));
const addCalls = added(ca, cb), addBtns = added(ba, bb);
const goneCalls = lost(ca, cb).filter((k) => !still(k) && !grown(k, addCalls));
const goneBtns = lost(ba, bb).filter((k) => !still(k) && !grown(k, addBtns));
const moved = [
  ...lost(ca, cb).filter((k) => still(k) || grown(k, addCalls)),
  ...lost(ba, bb).filter((k) => still(k) || grown(k, addBtns)),
];
if (moved.length) console.log(`    renamed/moved: ${moved.join(", ")}`);
if (goneCalls.length + goneBtns.length) {
  console.error(
    `  ${goneCalls.length + goneBtns.length} call(s)/button(s) gone for good - explain each one`
  );
  process.exit(1);
}
