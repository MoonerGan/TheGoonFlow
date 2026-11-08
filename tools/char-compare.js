const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const git = process.env.GIT_EXE || "C:\\Program Files\\Git\\bin\\git.exe";
const rel = process.argv[2];
const head = execFileSync(git, ["show", `HEAD:${rel}`], { encoding: "utf8", maxBuffer: 1 << 26 });
const cur = fs.readFileSync(rel, "utf8");
const count = (t) => {
  const m = new Map();
  for (const ch of t) {
    const c = ch.codePointAt(0);
    if (c <= 127) continue;
    m.set(c, (m.get(c) || 0) + 1);
  }
  return [...m.entries()].sort((a, b) => b[1] - a[1]);
};
const fmt = (m) => m.map(([c, n]) => `U+${c.toString(16).toUpperCase().padStart(4, "0")}x${n}`).join(" ");
console.log(rel);
console.log(`  HEAD non-ascii: ${fmt(count(head))}`);
console.log(`  WORK non-ascii: ${fmt(count(cur))}`);
