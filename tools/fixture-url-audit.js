// Fixture-URL audit: test data in this repo may not contain a site URL that looks real.
//
//   node tools\fixture-url-audit.js                 audit every line this branch adds
//   node tools\fixture-url-audit.js --range A..B    audit that range instead
//
// Why this is a check and not a code-review comment. A fixture written as a gallery link
// with a plausible id (`e-hentai.org/g/` plus six digits, no key) cannot be told apart
// from a link that came out of somebody's library, and a public repo keeps it forever:
// force-pushing removes a ref, not the fact. So the rule is that a fixture URL must be
// *self-evidently* fake - placeholder ids (`9999999`, `0123456789abcdef`, `1234/abcd`),
// documentation hosts (`example.org`), or a template (`{id}`, `<gid>`) - and this tool
// enforces it on the lines a change adds, which is the only set a change can be held
// responsible for. The offending URLs are deliberately not spelled out in this file:
// writing them here would re-add them to the history this tool exists to clean.
//
// Lines that legitimately name a real site - a production host constant, an API
// template with `{id}` - pass on their own shape. If one is flagged anyway, say why on
// the line itself with a trailing `fixture-url-ok:` note; an unexplained suppression is
// worse than none, so the note text is printed in the report.
const { execFileSync } = require("node:child_process");

// Hosts this app talks to, plus the CDN hosts those sites serve art from. Keep this in
// step with the site vocabulary in `goon-core` (DEVELOPMENT §4 invariant 10: site names
// are data, so a fixture that names a site is naming a real one).
const SITE_HOST =
  /\b(e-hentai\.org|exhentai\.org|hitomi\.la|nhentai\.net|gelbooru\.com|rule34\.xxx|yande\.re|danbooru\.[a-z.]+|safebooru\.org|konachan\.com|4cdn\.org|4chans\.ca|img\.4cdn\.org|i\.4cdn\.org|[a-z0-9.-]*rule34\.xxx|[a-z0-9.-]*gelbooru\.com)\b/i;

// Shapes that prove the author typed a placeholder rather than copied a link.
const PLACEHOLDER =
  /(9{5,}|0{8,}|[0f]{8,}|0123456789abcdef|example\.(org|com|net)|x\.to|averylongthing|<[^>\s]+>|\{[^}\s]+\}|\/\d{1,4}\/[a-z]{1,4}\/?$|\/[a-z]{2,4}\/\d{1,4}-\d+\b|\/tag\/|\/index\.php\s*$|\/public\/index\.php\s*$|\/api\/\w+\s*$)/;

function range() {
  const i = process.argv.indexOf("--range");
  if (i >= 0) return process.argv[i + 1];
  const head = git(["rev-parse", "--abbrev-ref", "HEAD"]);
  if (head === "main") fail("on main: pass --range, or run it on a branch");
  return "origin/main.." + head;
}

function git(args) {
  return execFileSync("git", args, { encoding: "utf8", maxBuffer: 64e6 }).trim();
}

function fail(msg) {
  console.log("FIXTURE-URL AUDIT FAILED: " + msg);
  process.exit(2);
}

const spec = range();
const commits = git(["rev-list", spec]).split("\n").filter(Boolean);
if (!commits.length) fail("range " + spec + " contains no commits");

let lines = 0;
let urls = 0;
const hits = [];
const notes = [];
for (const c of commits) {
  const diff = execFileSync("git", ["show", "--format=", "--unified=0", c], {
    encoding: "utf8",
    maxBuffer: 64e6,
  });
  let file = "";
  for (const l of diff.split("\n")) {
    if (l.startsWith("+++ b/")) {
      file = l.slice(6);
      continue;
    }
    if (!l.startsWith("+") || l.startsWith("+++")) continue;
    lines++;
    const text = l.slice(1);
    for (const m of text.matchAll(/https?:\/\/[^\s"'`)\\]*/g)) {
      const url = m[0];
      if (!SITE_HOST.test(url)) continue;
      // A bare host is not a link to a thing. The site vocabulary lives in production
      // constants (`"https://e-hentai.org"`), and naming a site is not naming someone's
      // library; only URLs that carry a path or a query can be a real item.
      const path = url.replace(/^https?:\/\/[^/]+/, "");
      if (path === "" || path === "/") continue;
      urls++;
      const note = /fixture-url-ok:\s*(.+)/.exec(text);
      if (note) {
        notes.push(url + "  (noted: " + note[1].trim().slice(0, 60) + ")");
        continue;
      }
      if (PLACEHOLDER.test(url)) continue;
      hits.push({ sha: c.slice(0, 7), file, url });
    }
  }
}

const unique = [...new Map(hits.map((h) => [h.url, h])).values()];
for (const h of unique) {
  console.log("POSSIBLE REAL LINK  " + h.url);
  console.log("    first added by " + h.sha + " in " + h.file);
}
for (const n of notes) console.log("suppressed: " + n);
console.log(
  commits.length +
    " commit(s), " +
    lines +
    " added line(s), " +
    urls +
    " site URL(s) checked, " +
    unique.length +
    " violation(s)"
);
if (unique.length) {
  console.log("FIXTURE-URL AUDIT FAILED: a fixture URL cannot be proved synthetic");
  process.exit(1);
}
console.log("CLEAN");
