const fs = require("node:fs");
const OK = new Set([
  0x00a7, 0x00b7, 0x2013, 0x2014, 0x2026, 0x2192, 0x2194, // § · – — … → ↔
  0x00d7, 0x2212, 0x2260, 0x2264, 0x2265, 0x2248, // × − ≠ ≤ ≥ ≈
  0x2190, 0x21c4, 0x25b6, 0x26a0, 0x26f6, 0x2705, 0x2713, // ← ⇄ ▶ ⚠ ⛶ ✅ ✓
  0x2500, 0x2502, 0x2514, 0x251c, // box drawing in DESIGN's tree
]);
const CJK = (c) => c >= 0x3000 && c <= 0x9fff;
let bad = 0;
for (const f of process.argv.slice(2)) {
  const t = fs.readFileSync(f, "utf8");
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
  console.log(`${f}: ${hits.length} line(s) with unexpected chars  ${[...seen].map(([c, n]) => `U+${c.toString(16).toUpperCase().padStart(4, "0")}x${n}`).join(" ")}`);
  hits.slice(0, 15).forEach((n) => console.log(`  ${n}: ${JSON.stringify(lines[n - 1].trim())}`));
  bad += hits.length;
}
console.log(bad === 0 ? "CLEAN" : `${bad} lines to inspect`);
