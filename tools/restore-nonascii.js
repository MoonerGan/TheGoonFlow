// Repairs Shift_JIS-mis-decoded text (PowerShell 5 `Get-Content -Raw` on UTF-8
// files without BOM). Alignment uses a letters+digits-only skeleton, which is
// immune to the decode damage (it also ate the byte after the character, so
// punctuation and newlines moved). Any line whose skeleton matches a HEAD line
// but whose text differs is a mojibake artifact: HEAD's exact bytes win.
//   node tools/restore-nonascii.js <repo-relative-path>...
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");

const git = process.env.GIT_EXE || "C:\\Program Files\\Git\\bin\\git.exe";
const sk = (s) => s.replace(/[^A-Za-z0-9]/g, "");

for (const rel of process.argv.slice(2)) {
  const head = execFileSync(git, ["show", `HEAD:${rel}`], {
    encoding: "utf8",
    maxBuffer: 1 << 26,
  }).split(/\r?\n/);
  const cur = fs.readFileSync(rel, "utf8");
  const eol = cur.includes("\r\n") ? "\r\n" : "\n";
  const C = cur.split(/\r?\n/);

  // index HEAD by skeleton (only for lines that actually carry non-ASCII)
  const bySk = new Map();
  head.forEach((l, i) => {
    if (!/[^\x00-\x7F]/.test(l)) return;
    const k = sk(l);
    if (!bySk.has(k)) bySk.set(k, []);
    bySk.get(k).push(i);
  });

  const out = [];
  const report = [];
  let merged = 0;
  for (let j = 0; j < C.length; j++) {
    const line = C[j];
    if (!/[^\x00-\x7F]/.test(line)) {
      out.push(line);
      continue;
    }
    const hits = bySk.get(sk(line)) || [];
    const hit = hits.find((i) => head[i] !== line);
    if (hit !== undefined) {
      out.push(head[hit]);
      report.push(`${j + 1}: ${JSON.stringify(line.trim())}\n   = ${JSON.stringify(head[hit].trim())}`);
      continue;
    }
    // merged pair: damage swallowed a newline, gluing two HEAD lines together.
    // Both sides must be real, non-ASCII-bearing lines: a trivial neighbour
    // (a lone "}" or blank line) would otherwise match almost any line and
    // duplicate it into the file.
    const meaningful = (l) => l.length > 12 && /[^\x00-\x7F]/.test(l);
    let done = false;
    for (let i = 0; i + 1 < head.length && !done; i++) {
      if (!meaningful(head[i]) || !meaningful(head[i + 1])) continue;
      if (sk(head[i] + head[i + 1]) === sk(line)) {
        out.push(head[i], head[i + 1]);
        merged++;
        report.push(`${j + 1}: merged pair restored\n   = ${JSON.stringify(head[i].trim())}\n   = ${JSON.stringify(head[i + 1].trim())}`);
        done = true;
      }
    }
    if (!done) {
      if (!hits.length) report.push(`${j + 1}: UNRESOLVED ${JSON.stringify(line.trim())}`);
      out.push(line);
    }
  }

  fs.writeFileSync(rel, out.join(eol), "utf8");
  console.log(`${rel}: restored ${report.filter((r) => !r.includes("UNRESOLVED")).length} (merged ${merged}), unresolved ${report.filter((r) => r.includes("UNRESOLVED")).length}`);
  console.log(report.slice(0, 40).join("\n"));
}
