// Character audit for a repo whose docs and UI contain deliberate non-ASCII.
//
//   node tools\char-audit.js                 audit EVERY git-tracked text file
//   node tools\char-audit.js file1 file2     audit just those (still exits non-zero)
//
// It used to read only `process.argv` file arguments, so running it bare printed
// CLEAN while examining nothing at all - a check that cannot fail is worse than no
// check, because it is quoted in commit messages as evidence. It now defaults to
// everything git tracks, and its exit code is part of the verdict.
const fs = require("node:fs");
const { execFileSync } = require("node:child_process");

// Characters the project uses on purpose: typography in the docs, box drawing in
// DESIGN's tree, and symbols the UI actually prints. Anything else is either
// mojibake from a PowerShell round-trip or text pasted from somewhere else, and
// this repo is public, so an unexpected character is also a leak vector.
const OK = new Set([
  0x00a7, 0x00b7, 0x2013, 0x2014, 0x2026, 0x2192, 0x2194, // § · – — … → ↔
  0x00d7, 0x2212, 0x2260, 0x2264, 0x2265, 0x2248, // × − ≠ ≤ ≥ ≈
  0x2190, 0x21c4, 0x25b6, 0x26a0, 0x26f6, 0x2705, 0x2713, // ← ⇄ ▶ ⚠ ⛶ ✅ ✓
  0x2500, 0x2502, 0x2514, 0x251c, // box drawing in DESIGN's tree
  0x00b1, 0x2019, 0x2261, // ± (zoom controls) ’ (prose) ≡ (predicate identity)
  0x201c, 0x201d, // “ ” curly quotes inside UI sentences
  0x21ba, 0x25c0, 0x25b8, // ↺ revert, ◀ ▶ step buttons the viewer really draws
  0x1f512, // 🔒 the lock screen's glyph
]);
const CJK = (c) => c >= 0x3000 && c <= 0x9fff;

// Binary-ish and vendored paths have no business in a text audit; media fixtures
// are bytes, and `tools/*.log` evidence may contain whatever a probe printed.
const SKIP = /(^|\/)(target|node_modules|dist|fixtures)\//;
const TEXT = /\.(rs|js|mjs|cjs|ts|svelte|json|toml|md|ps1|css|html|txt|example|lock|yml|yaml|gitignore|editorconfig|nimble|slim|conf|ini|bat|cmd)$/i;

function tracked() {
  const out = execFileSync("git", ["ls-files"], { encoding: "utf8", maxBuffer: 64e6 });
  return out.split("\n").filter(Boolean);
}

const args = process.argv.slice(2);
const files = (args.length ? args : tracked()).filter(
  (f) => !SKIP.test(f) && (TEXT.test(f) || f.startsWith(".") || !/\.[a-z0-9]+$/i.test(f))
);
if (!files.length) {
  console.log("nothing to audit (no arguments and git tracked no matching files)");
  process.exit(1);
}

let bad = 0;
let files_with_hits = 0;
for (const f of files) {
  let t;
  try {
    t = fs.readFileSync(f, "utf8");
  } catch {
    continue; // deleted between `git ls-files` and now
  }
  if (t.includes("\ufffd") || /^\ufeff/.test(t)) {
    // U+FFFD replacement chars and a BOM are the signature of a text round-trip
    // through PowerShell - reported even when the allow-list would forgive them.
    console.log(`${f}: mojibake signature (U+FFFD) or BOM present`);
    bad += 1;
    files_with_hits += 1;
    continue;
  }
  const lines = t.split(/\r?\n/);
  const hits = [];
  const seen = new Map();
  lines.forEach((l, i) => {
    for (const ch of l) {
      const c = ch.codePointAt(0);
      if (c <= 127 || OK.has(c) || CJK(c)) continue;
      seen.set(c, (seen.get(c) || 0) + 1);
      if (!hits.includes(i + 1)) hits.push(i + 1);
    }
  });
  if (!hits.length) continue;
  files_with_hits += 1;
  console.log(
    `${f}: ${hits.length} line(s) with unexpected chars  ` +
      [...seen]
        .map(([c, n]) => `U+${c.toString(16).toUpperCase().padStart(4, "0")}x${n}`)
        .join(" ")
  );
  hits.slice(0, 15).forEach((n) => console.log(`  ${n}: ${JSON.stringify(lines[n - 1].trim())}`));
  bad += hits.length;
}
console.log(`${files.length} file(s) audited`);
console.log(bad === 0 ? "CLEAN" : `${bad} line(s) to inspect in ${files_with_hits} file(s)`);
process.exitCode = bad === 0 ? 0 : 1;
