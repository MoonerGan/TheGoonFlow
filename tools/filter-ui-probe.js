// The query box, measured in a real session: real exe, real WebView2, real (throwaway)
// vault, real keystrokes into the real box.
//
// Why: stage S5 replaces four facet rails with one grammar box, and almost everything it
// promises is a rendering claim, not a SQL claim. Unit tests prove the parser and the
// planned predicate; they cannot show that a chip row appears **above** the grid rather
// than beside it (invariant 24), that a parse error survives as form state, that an
// uncounted keystroke paints "… matches" instead of a spinner or last query's number
// (invariant 27), that the shape each tag term got is actually printed to the user, or
// that the two sidebar items plan §5 promises to keep still work. This measures all of
// that in the live page.
//
// Safety: the exe runs as a copy in %TEMP%\goon-filter-probe with its own vault folder
// and a throwaway passphrase, and the probe checks **twice** that this is the only vault
// it can reach — once in the path it builds, once against `app_status.vaultRoot` after
// unlocking. It refuses to run otherwise, because a probe that can open the operator's
// real library is not a probe: the queries it types are arbitrary and it holds the vault
// open while it does. Like the other live tools it stops running instances first, since
// the single-instance plugin would otherwise hand the copy off instead of opening its own
// window.
//
// Build: this probes the **debug** build of the tree it lives in, so the frontend assets
// baked into the exe have to be the ones under test. If the exe is older than any file
// under apps/desktop/src, it says so and stops rather than measuring last week's markup.
//
// House note: the page-side scripts are template literals, so a backtick inside one of
// their comments is a syntax error, not a comment — `node --check` catches it, and it is
// worth running after any edit here.
//
//   cargo build -p goonflow-desktop            # or: powershell tools\run-release.ps1
//   node tools/filter-ui-probe.js              # all cases
//   node tools/filter-ui-probe.js --keep       # leave the scratch folder alone
const { spawn, execSync } = require('child_process');
const fs = require('fs');
const net = require('net');
const os = require('os');
const path = require('path');
const zlib = require('zlib');

const REPO = path.resolve(__dirname, '..');
const EXE = path.join(REPO, 'target/debug/GoonFlow.exe');
const SCRATCH = path.join(os.tmpdir(), 'goon-filter-probe');
const MEDIA = path.join(SCRATCH, 'media');
const VAULT = path.join(SCRATCH, 'vault');
const PASSPHRASE = 'filter-probe-throwaway-not-a-real-one';

// ---------- the vault guard, before anything else can happen ----------
/** The one scratch folder this probe may ever touch, checked at module load, so a
 *  mistyped edit to this file cannot quietly point the probe at a real library.
 *  The rule is exact equality with `<temp>/goon-filter-probe`, not a list of "looks
 *  like a profile" markers: the first version of this guard rejected its own correct
 *  path, because on Windows the temp dir *is* inside AppData. A guard that always
 *  fails is as useless as one that never fires, so the comparison is one line and
 *  the name is the whole policy. */
function guardScratch() {
  const want = path.resolve(path.join(os.tmpdir(), 'goon-filter-probe'));
  const norm = path.resolve(SCRATCH);
  if (norm.toLowerCase() !== want.toLowerCase()) {
    throw new Error(`refusing to run: scratch folder is ${norm}, expected exactly ${want}`);
  }
}

// ---------- a small solid-colour PNG encoder: exact sizes, no dependencies ----------
const crcTable = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
const crc32 = (buf) => {
  let c = -1;
  for (let i = 0; i < buf.length; i += 1) c = (c >>> 8) ^ crcTable[(c ^ buf[i]) & 0xff];
  return (c ^ -1) >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
};
function writePng(file, w, h) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 2;
  const rows = [];
  for (let y = 0; y < h; y += 1) {
    const row = Buffer.alloc(1 + w * 3);
    for (let x = 0; x < w; x += 1) {
      const i = 1 + x * 3;
      const band = ((x >> 5) + (y >> 5)) % 2;
      row[i] = band ? 0x24 : 0xc8;
      row[i + 1] = band ? 0x5f : 0x3a;
      row[i + 2] = band ? 0x9c : 0x46;
    }
    rows.push(row);
  }
  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', zlib.deflateSync(Buffer.concat(rows), { level: 0 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
  fs.writeFileSync(file, png);
  return png.length;
}

/** Three stored pictures in three folders — `import_image_files` makes **one item per
 *  folder**, so one folder each is what gives three grid rows. Tags are then written on
 *  two of them and one stays bare, which is what the "no tags" preset has to find. Titles
 *  are deliberate: the folder name becomes the item title. */
const FIXTURES = [
  { dir: 'summer-witch-big-breasts-anal', file: 'page-1.png', w: 900, h: 700 },
  { dir: 'summer-witch-big-breasts', file: 'page-1.png', w: 700, h: 500 },
  { dir: 'untagged-tile', file: 'page-1.png', w: 500, h: 400 },
];

// ---------- page side ----------
/** One snapshot of the query box and everything it is supposed to have rendered. The
 *  selectors are the view's own classes; if one of them is missing the probe says so
 *  rather than reporting a pass, because a renamed class is exactly how a UI "test"
 *  ends up asserting nothing. */
const SNAPSHOT = `const snap = () => {
  const text = (el) => (el && el.textContent ? el.textContent.replace(/\\s+/g, ' ').trim() : null);
  const pick = (sel, re) => {
    const hit = [...document.querySelectorAll(sel)].find((b) => re.test(text(b)));
    return hit ? text(hit) : null;
  };
  const box = document.querySelector('.query-box');
  const grid = document.querySelector('.grid');
  const chips = document.querySelector('.query-chips');
  const r = (el) => (el ? el.getBoundingClientRect() : null);
  const gridBox = r(grid);
  const chipBox = r(chips);
  const tiles = [...document.querySelectorAll('button.tile')].map(text);
  return {
    boxValue: box ? box.value : null,
    error: text(document.querySelector('.query-error')),
    conflict: text(document.querySelector('.query-conflict')),
    presets: [...document.querySelectorAll('.query-presets .preset')].map((b) => ({
      label: text(b),
      on: /\\bon\\b/.test(b.className),
    })),
    suggest: [...document.querySelectorAll('.query-suggest .suggest')].map((b) => text(b)),
    chipTerms: [...document.querySelectorAll('.query-chips .chip')].map((b) => text(b).replace(/×$/, '').trim()),
    count: text(document.querySelector('.query-count')),
    diagnostic: text(document.querySelector('.query-diagnostic summary')),
    diagnosticLines: [...document.querySelectorAll('.query-diagnostic li')].map(text),
    paging: [...document.querySelectorAll('.query-paging button')].map((b) => ({
      label: text(b),
      disabled: !!b.disabled,
    })),
    pagingHint: text(document.querySelector('.query-paging .hint')),
    empty: text(document.querySelector('.empty')),
    panelTitle: text(document.querySelector('.panel-title')),
    factsRow: !!document.querySelector('.facts-row'),
    tiles,
    // invariant 24 / plan §5: the chips row is one block **above** the grid, never a
    // flex sibling of the thing it describes. Measured, not asserted.
    layout: gridBox && chipBox ? {
      chipsAboveGrid: chipBox.bottom <= gridBox.top + 1,
      sameColumn: Math.abs(chipBox.left - gridBox.left) < gridBox.width,
      chipsParentIsNotGridParentFlexRow: (() => {
        if (!chips.parentElement) return null;
        const cs = getComputedStyle(chips.parentElement);
        return cs.display + '/' + cs.flexDirection;
      })(),
      chipsBottom: Math.round(chipBox.bottom),
      gridTop: Math.round(gridBox.top),
    } : null,
    sidebar: [...document.querySelectorAll('.sidebar .section')].map(text),
    dupesButton: pick('.sidebar .nav-item', /possible dupes/i),
    vaultLine: text(document.querySelector('.sidebar .vault-info')),
  };
};`;
const withSnap = (expr) => `(() => { ${SNAPSHOT}\n return (${expr}); })()`;

const PAGE = {
  /** Backend-side setup: throwaway vault, import the three pictures, tag two of them. */
  setup: (dir, phrase, grouped) => `(async () => {
    const inv = (c, a) => window.__TAURI_INTERNALS__.invoke(c, a || {}, {});
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    try { await inv('create_vault', { dir: ${JSON.stringify(dir)}, passphrase: ${JSON.stringify(phrase)} }); }
    catch (e) { /* a vault from an earlier run of this probe */ }
    try { await inv('attach_vault', { dir: ${JSON.stringify(dir)} }); } catch (e) { /* already attached */ }
    let unlocked = false;
    let lastErr = '';
    for (let i = 0; i < 20 && !unlocked; i += 1) {
      try { await inv('unlock', { passphrase: ${JSON.stringify(phrase)} }); } catch (e) { lastErr = String(e); await sleep(300); continue; }
      unlocked = (await inv('app_status', {})).unlocked;
      if (!unlocked) await sleep(300);
    }
    if (!unlocked) return { unlocked, lastErr };
    const status = await inv('app_status', {});
    // One import call per folder: 'import_image_files' makes one item per call, and the
    // probe needs three grid rows, not one item with three frames.
    const imports = [];
    for (const group of ${JSON.stringify(grouped)}) imports.push(await inv('import_image_files', { paths: group }));
    const items = await inv('search_items', { view: 'media', limit: 50, withCount: true });
    // Provenance and tags, so the grammar has something real to match. One item stays
    // untagged: that is what the "no tags" preset is supposed to find.
    const ids = items.items.map((i) => i.id);
    const titles = items.items.map((i) => i.title);
    for (const id of ids.slice(0, 2)) {
      await inv('add_item_tag', { id, name: 'big_breasts', kind: 'content' });
    }
    if (ids.length) await inv('add_item_tag', { id: ids[0], name: 'anal', kind: 'content', site: 'ehentai' });
    const counted = await inv('search_items', { view: 'media', query: 'big_breasts', limit: 50, withCount: true });
    return { unlocked, vaultRoot: status.vaultRoot, imports, titles, taggedNow: counted.total };
  })()`,

  /** The paging fixtures: 300 video paths. Videos are the cheap bulk, because the
   *  importer records the path and its `local` source and never stores or decodes the
   *  bytes, so a second page exists without a minute of thumbnailing. */
  bulk: (paths) => `(async () => {
    const inv = (c, a) => window.__TAURI_INTERNALS__.invoke(c, a || {}, {});
    const report = await inv('import_video_files', { paths: ${JSON.stringify(paths)} });
    return { ok: report.imported === ${JSON.stringify(paths.length)}, report };
  })()`,

  /** Media tab, and wait for the grid to be on screen. */
  open: `(async () => {
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    const text = (el) => (el && el.textContent ? el.textContent.trim() : '');
    let tab = null;
    for (let i = 0; i < 40 && !tab; i += 1) {
      tab = [...document.querySelectorAll('.tabs button')].find((b) => /media/i.test(text(b)));
      if (!tab) await sleep(400);
    }
    if (!tab) return { err: 'still no media tab; buttons=' + [...document.querySelectorAll('button')].map(text).slice(0, 10).join('|') };
    if (!/active/.test(tab.className)) { tab.click(); await sleep(600); }
    for (let i = 0; i < 40 && !document.querySelector('.query-box'); i += 1) await sleep(300);
    if (!document.querySelector('.query-box')) return { err: 'no query box on the media view' };
    for (let i = 0; i < 40 && !document.querySelector('button.tile'); i += 1) await sleep(300);
    return { tiles: document.querySelectorAll('button.tile').length };
  })()`,

  /** Type into the real box the way a user does: set the value, dispatch `input`. The
   *  view's own debounce runs; nothing here calls the backend directly. */
  type: (value) => `(async () => {
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    const box = document.querySelector('.query-box');
    if (!box) return { err: 'the query box is gone' };
    box.focus();
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(box, ${JSON.stringify(value)});
    box.dispatchEvent(new Event('input', { bubbles: true }));
    await sleep(900);
    return { ok: true };
  })()`,

  press: (key) => `(async () => {
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    const box = document.querySelector('.query-box');
    box.focus();
    box.dispatchEvent(new KeyboardEvent('keydown', { key: ${JSON.stringify(key)}, bubbles: true }));
    await sleep(900);
    return { ok: true };
  })()`,

  /** Click by exact text inside a container — a chip, a preset, a sidebar item. */
  clickWhere: (selector, label) => `(async () => {
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    const text = (el) => (el && el.textContent ? el.textContent.trim() : '');
    const all = [...document.querySelectorAll(${JSON.stringify(selector)})];
    const hit = all.find((b) => text(b).replace(/×$/, '').trim() === ${JSON.stringify(label)});
    if (!hit) return { err: 'no ' + ${JSON.stringify(selector)} + ' labelled ' + ${JSON.stringify(label)} + '; saw ' + all.map(text).slice(0, 12).join('|') };
    hit.click();
    await sleep(900);
    return { ok: true };
  })()`,
};

function freePort() {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.once('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const { port } = srv.address();
      srv.close(() => resolve(port));
    });
  });
}

/** The debug exe has to be newer than the frontend it is supposed to be showing: the
 *  assets are baked in, so a stale build quietly measures the previous stage's markup.
 *  Both trees are checked — `src` (what the assets are built from) and `dist` (what is
 *  actually embedded), because `cargo build` does not re-run `npm run build`. */
function assertFreshExe() {
  const exeTime = fs.statSync(EXE).mtimeMs;
  let newest = 0;
  let newestFile = '';
  const walk = (dir) => {
    if (!fs.existsSync(dir)) return;
    for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
      const f = path.join(dir, e.name);
      if (e.isDirectory()) walk(f);
      else {
        const t = fs.statSync(f).mtimeMs;
        if (t > newest) {
          newest = t;
          newestFile = f;
        }
      }
    }
  };
  walk(path.join(REPO, 'apps/desktop/src'));
  walk(path.join(REPO, 'apps/desktop/dist'));
  if (newest > exeTime) {
    throw new Error(
      `${EXE} is older than ${newestFile}; rebuild it (npm run build in apps\\desktop, then cargo build -p goonflow-desktop) before probing the UI`,
    );
  }
}

async function main() {
  guardScratch();
  if (!fs.existsSync(EXE)) {
    throw new Error(`no debug exe at ${EXE} (run: cargo build -p goonflow-desktop)`);
  }
  assertFreshExe();

  // fresh scratch every run: a stale vault or stale items would change the answer
  fs.rmSync(SCRATCH, { recursive: true, force: true });
  fs.mkdirSync(MEDIA, { recursive: true });
  fs.mkdirSync(VAULT, { recursive: true });
  const groups = [];
  for (const f of FIXTURES) {
    const dir = path.join(MEDIA, f.dir);
    fs.mkdirSync(dir, { recursive: true });
    const file = path.join(dir, f.file);
    const bytes = writePng(file, f.w, f.h);
    groups.push([file]);
    console.log(`fixture ${f.dir}/${f.file}: ${bytes} B, ${f.w}x${f.h}`);
  }
  // Paging needs a result that does not fit on one page, and the view's page is 300
  // rows, so three items are not enough: 300 more come from the video importer, which
  // records a path and a `local` source without reading the bytes (so the padding is
  // four bytes each, and the import is seconds rather than a thumbnailing run).
  const BULK = [];
  const bulkDir = path.join(SCRATCH, 'bulk');
  fs.mkdirSync(bulkDir, { recursive: true });
  for (let i = 0; i < 300; i += 1) {
    const file = path.join(bulkDir, `probe-video-${String(i).padStart(3, '0')}.mp4`);
    fs.writeFileSync(file, Buffer.from([0x00, 0x00, 0x00, 0x20]));
    BULK.push(file);
  }
  console.log(`paging fixtures: 300 stub video files in ${bulkDir}`);

  const port = await freePort();
  try {
    execSync('taskkill /IM GoonFlow.exe /F', { stdio: 'ignore' });
  } catch {
    /* nothing running */
  }
  await new Promise((r) => setTimeout(r, 700));

  const copy = path.join(SCRATCH, 'GoonFlow.exe');
  fs.copyFileSync(EXE, copy);
  for (const f of ['flow.cfg', 'chrome.cfg']) fs.rmSync(path.join(SCRATCH, f), { force: true });

  const proc = spawn(copy, [], {
    cwd: SCRATCH,
    env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
    detached: true,
    stdio: 'ignore',
  });
  proc.unref();

  const seen = [];
  let ws;
  try {
    let target = null;
    for (let i = 0; i < 60 && !target; i += 1) {
      try {
        const r = await fetch(`http://127.0.0.1:${port}/json/list`);
        const t = (await r.json()).filter((x) => x.type === 'page' && x.webSocketDebuggerUrl);
        if (t.length) target = t[0];
      } catch {
        /* not up yet */
      }
      if (!target) await new Promise((r) => setTimeout(r, 500));
    }
    if (!target) throw new Error(`devtools port ${port} never reported a page`);
    console.log(`\npage: ${target.url}  title: ${JSON.stringify(target.title)}  (devtools :${port}, pid ${proc.pid})`);

    ws = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((res, rej) => {
      ws.addEventListener('open', res);
      ws.addEventListener('error', () => rej(new Error('devtools websocket failed')));
    });
    let id = 0;
    const pending = new Map();
    ws.addEventListener('message', (evData) => {
      const m = JSON.parse(evData.data);
      if (m.id && pending.has(m.id)) {
        const p = pending.get(m.id);
        pending.delete(m.id);
        if (m.error) p.rej(new Error(JSON.stringify(m.error)));
        else p.res(m.result);
      }
    });
    const send = (method, params) =>
      new Promise((res, rej) => {
        id += 1;
        pending.set(id, { res, rej });
        ws.send(JSON.stringify({ id, method, params: params || {} }));
      });
    const ev = async (expression) => {
      const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
      if (r.exceptionDetails) throw new Error('page threw: ' + JSON.stringify(r.exceptionDetails).slice(0, 400));
      return r.result.value;
    };

    const setup = await ev(PAGE.setup(VAULT, PASSPHRASE, groups));
    if (!setup.unlocked) throw new Error(`the scratch vault would not open: ${setup.lastErr}`);
    // The second half of the vault guard: whatever the page's app_status says the root
    // is, it has to be the throwaway folder this probe built. If the single-instance
    // handoff ever sent the copy to an already-running app, this is where it gets caught
    // — before a single query is typed.
    const root = String(setup.vaultRoot || '');
    if (root.replace(/[\\/]+$/, '').toLowerCase() !== VAULT.replace(/[\\/]+$/, '').toLowerCase()) {
      throw new Error(`the running app's vault is ${JSON.stringify(root)}, not the throwaway ${VAULT}: refusing to type queries into it`);
    }
    console.log(
      `vault ${JSON.stringify(root)} unlocked; items=${JSON.stringify(setup.titles)} ` +
        `big_breasts now matches ${setup.taggedNow}`,
    );
    // the unlock came from outside the page, so the UI has not noticed it: reload
    try {
      await send('Page.reload', { ignoreCache: true });
    } catch {
      /* the context goes away mid-reload, which is the point */
    }
    await new Promise((r) => setTimeout(r, 3000));

    const opened = await ev(withSnap(PAGE.open));
    if (opened.err) throw new Error(opened.err);
    console.log(`media tab open, ${opened.tiles} tiles on screen`);

    /** Do something in the page (type, press, click), then read one snapshot. The
     *  action and the read are deliberately separate calls: an action's own return
     *  value is `{ok}` or `{err}`, not the state, and reading the state in the same
     *  expression is how a probe ends up asserting on `undefined`. */
    const shot = async (label, action) => {
      if (action) {
        const r = await ev(action);
        if (r && r.err) throw new Error(`${label}: ${r.err}`);
      }
      return ev(withSnap('snap()'));
    };
    const record = (name, s, notes) => {
      seen.push({ name, s, notes });
      console.log(`\n--- ${name} ---`);
      console.log(`  box ${JSON.stringify(s.boxValue)}`);
      console.log(`  count ${JSON.stringify(s.count)}  chips ${JSON.stringify(s.chipTerms)}`);
      if (s.error) console.log(`  inline error ${JSON.stringify(s.error)}`);
      if (s.conflict) console.log(`  conflict ${JSON.stringify(s.conflict)}`);
      if (s.suggest.length) console.log(`  suggest ${JSON.stringify(s.suggest)}`);
      if (s.diagnostic) console.log(`  diagnostic ${JSON.stringify(s.diagnostic)} ${JSON.stringify(s.diagnosticLines)}`);
      console.log(`  paging ${JSON.stringify(s.paging)} hint ${JSON.stringify(s.pagingHint)}`);
      console.log(`  tiles ${s.tiles.length} empty ${JSON.stringify(s.empty)}`);
      if (s.layout) console.log(`  layout chips ${s.layout.chipsBottom} grid ${s.layout.gridTop} above=${s.layout.chipsAboveGrid} column=${s.layout.sameColumn} parent=${s.layout.chipsParentIsNotGridParentFlexRow}`);
      if (notes.length) console.log(`  PROBLEMS: ${notes.join(' · ')}`);
      else console.log('  OK');
      if (notes.length) return 1;
      return 0;
    };

    let failures = 0;

    // 1. an uncounted keystroke must not invent a total (invariant 27)
    let s = await shot('type big_breasts', PAGE.type('big_breasts'));
    failures += record('typed a tag, no Enter yet', s, [
      ...(s.chipTerms.join(' ') !== 'big_breasts' ? [`chip row says ${JSON.stringify(s.chipTerms)}`] : []),
      ...(s.count !== null && !/^… matches/.test(s.count) ? [`the count line reads ${JSON.stringify(s.count)} while no total was asked for`] : []),
      ...(s.pagingHint && /of \d+/.test(s.pagingHint) ? [`the paging hint claims a total: ${JSON.stringify(s.pagingHint)}`] : []),
      ...(s.tiles.length !== 2 ? [`grid shows ${s.tiles.length} tiles, the tag matches 2`] : []),
    ]);

    // 2. Enter commits, and the committed query gets its total (plan §5's "N matches")
    s = await shot('Enter', PAGE.press('Enter'));
    failures += record('committed with Enter', s, [
      ...(!/\b2 matches\b/.test(s.count || '') ? [`the count line reads ${JSON.stringify(s.count)}, expected "2 matches · 3 in the library"`] : []),
      ...(!/3 in the library/.test(s.count || '') ? [`no library total in ${JSON.stringify(s.count)}`] : []),
      ...(s.diagnosticLines.length !== 1 ? [`the plan printed ${s.diagnosticLines.length} shape notes, expected 1`] : []),
      ...(!/big_breasts:.*->/.test(s.diagnosticLines.join('|')) ? [`shape note not printed: ${JSON.stringify(s.diagnosticLines)}`] : []),
    ]);

    // 3. a parse error is form state: inline, and still there a moment later
    s = await shot('type a bad value', PAGE.type('type:banana'));
    const beforeWait = s;
    await new Promise((r) => setTimeout(r, 1500));
    s = await shot('wait', null);
    failures += record('a wrong value for type:', s, [
      ...(!s.error ? ['no inline error at all'] : []),
      ...(s.error && !/banana/.test(s.error) ? [`the error does not name the token: ${JSON.stringify(s.error)}`] : []),
      ...(s.error !== beforeWait.error ? ['the error is not state: it changed on its own'] : []),
      ...(s.error && /possible dupes|toast/i.test(s.error) ? ['the error arrived as a toast'] : []),
      ...(s.tiles.length !== 0 ? [`the grid still shows ${s.tiles.length} tiles for a query that does not parse`] : []),
    ]);

    // 4. an unsatisfiable conjunction of a single-valued key is noticed, not rendered
    //    as an empty grid (plan §4's `ext:gif ext:mp4`)
    s = await shot('type ext:gif ext:mp4', PAGE.type('ext:gif ext:mp4'));
    failures += record('two extensions ANDed', s, [
      ...(!s.conflict ? ['nothing said the conjunction cannot be true'] : []),
      ...(s.conflict && !/exactly one/.test(s.conflict) ? [`the hint does not explain why: ${JSON.stringify(s.conflict)}`] : []),
      ...(s.factsRow ? ['a query that is impossible by construction also claimed "not measured yet"'] : []),
      ...(!/cannot match/.test(s.empty || '') ? [`the empty state says ${JSON.stringify(s.empty)} instead of naming the conflict`] : []),
    ]);

    // 5. autocomplete rows, with the count of the query already typed
    s = await shot('type has:', PAGE.type('has:'));
    failures += record('half-typed keyword', s, [
      ...(s.suggest.length < 1 ? ['no candidates for `has:`'] : []),
      ...(!s.suggest.some((x) => /has:tags/.test(x)) ? [`candidates ${JSON.stringify(s.suggest)} miss has:tags`] : []),
    ]);

    // 6. a chip click removes the term — the box stays the source of truth, and the
    //    remaining term is still grammar
    s = await shot('type two terms', PAGE.type('big_breasts -anal'));
    s = await shot('click one chip', PAGE.clickWhere('.query-chips .chip', 'big_breasts'));
    failures += record('chip click', s, [
      ...(s.boxValue !== '-anal' ? [`box is ${JSON.stringify(s.boxValue)} after removing the big_breasts chip`] : []),
      ...(s.chipTerms.join(' ') !== '-anal' ? [`chips are ${JSON.stringify(s.chipTerms)}`] : []),
    ]);

    // 7. and the query that follows is the grammar the box holds, counted
    s = await shot('Enter', PAGE.press('Enter'));
    failures += record('the surviving term, committed', s, [
      ...(!/\b2 matches\b/.test(s.count || '') ? [`count line ${JSON.stringify(s.count)}, expected 2 matches of 3`] : []),
      ...(s.tiles.length !== 2 ? [`grid shows ${s.tiles.length} tiles for -anal (2 of 3 items lack it)`] : []),
      ...(s.layout && !s.layout.chipsAboveGrid ? [`chips bottom ${s.layout.chipsBottom} is not above grid top ${s.layout.gridTop}`] : []),
      ...(s.layout && s.layout.chipsParentIsNotGridParentFlexRow === 'flex/row' ? ['the chip row is a flex row sibling of the grid'] : []),
    ]);

    // 8. the #40 queues survive as presets: one click writes the predicate, and the
    //    old "no tags" queue is what `-has:tags` answers
    s = await shot('clear first', PAGE.type(''));
    s = await shot('click the no-tags preset', PAGE.clickWhere('.query-presets .preset', 'no tags'));
    s = await shot('Enter', PAGE.press('Enter'));
    failures += record('the no-tags preset', s, [
      ...(s.boxValue !== '-has:tags' ? [`the box holds ${JSON.stringify(s.boxValue)}, the preset should have written -has:tags`] : []),
      ...(!s.presets.some((p) => p.label === 'no tags' && p.on) ? ['the preset does not show as active'] : []),
      ...(!/\b1 match\b/.test(s.count || '') ? [`count line ${JSON.stringify(s.count)}, expected 1 match`] : []),
      ...(s.tiles.length !== 1 ? [`grid shows ${s.tiles.length} tiles`] : []),
    ]);

    // 9. a fact predicate on a vault that has never measured that fact says so
    s = await shot('type length>30s', PAGE.type('length>30s'));
    s = await shot('Enter', PAGE.press('Enter'));
    failures += record('a fact nobody measured', s, [
      ...(!s.factsRow ? ['no "not measured yet" row; a bare zero would read as an answer'] : []),
      ...(!/not measured yet/.test(s.count || '') ? [`the count line reads ${JSON.stringify(s.count)}, which claims a number for an unmeasured fact`] : []),
      ...(s.empty && /Nothing matches/.test(s.empty) && !s.factsRow ? ['an unmeasured fact rendered as "nothing matches"'] : []),
    ]);

    // 10. the honest empty: a term that parses and matches nothing says so, and does
    //     not borrow the unmeasured-facts wording
    s = await shot('type a site nobody imported from', PAGE.type('source:ehentai'));
    s = await shot('Enter', PAGE.press('Enter'));
    failures += record('parses, matches nothing', s, [
      ...(!/Nothing matches/.test(s.empty || '') ? [`the empty state says ${JSON.stringify(s.empty)}`] : []),
      ...(s.factsRow ? ['a term that only needs a source row claimed "not measured yet"'] : []),
      ...(!/\b0 matches\b/.test(s.count || '') ? [`count line ${JSON.stringify(s.count)}`] : []),
    ]);

    // 11. paging, measured across a real page boundary. 300 video paths are imported
    //     (one item per file, no bytes read), so the media library is 303 rows and the
    //     view's 300-row page has a second page behind it. A cursor gives load-more +
    //     back-to-start; a page number over a grid that shifts would be a lie
    //     (invariant 31).
    const bulk = await ev(PAGE.bulk(BULK));
    if (!bulk.ok) throw new Error(`the paging fixtures would not import: ${JSON.stringify(bulk.report)}`);
    console.log(`paging fixtures: ${bulk.report.imported} video items tracked`);
    await new Promise((r) => setTimeout(r, 1500));
    s = await shot('clear and commit', PAGE.type(''));
    s = await shot('Enter', PAGE.press('Enter'));
    failures += record('page 1 of a result that does not fit', s, [
      ...(s.tiles.length !== 300 ? [`the first page rendered ${s.tiles.length} tiles, expected exactly 300`] : []),
      ...(!/showing 300 of 303/.test(s.pagingHint || '') ? [`the loaded count reads ${JSON.stringify(s.pagingHint)}, expected "showing 300 of 303"`] : []),
      ...(!s.paging.some((b) => b.label === 'Load more' && !b.disabled) ? [`no enabled load-more control: ${JSON.stringify(s.paging)}`] : []),
      ...(s.paging.some((b) => /page \d|next|prev/i.test(b.label)) ? ['a page-number control on a keyset-paged grid'] : []),
      ...(!/303 matches/.test(s.count || '') ? [`the committed total reads ${JSON.stringify(s.count)}`] : []),
      ...(!s.dupesButton ? ['the *Possible dupes* sidebar item is gone'] : []),
      ...(!s.sidebar.some((x) => /vault/i.test(x)) ? ['no Vault section in the sidebar'] : []),
    ]);
    s = await shot('load more', PAGE.clickWhere('.query-paging button', 'Load more'));
    failures += record('page 2, by cursor', s, [
      ...(s.tiles.length !== 303 ? [`the grid holds ${s.tiles.length} tiles after load-more, expected 303`] : []),
      ...(!/showing 303 of 303/.test(s.pagingHint || '') ? [`the loaded count reads ${JSON.stringify(s.pagingHint)}, expected "showing 303 of 303": an append may not drop a total that was already paid for`] : []),
      ...(!/303 matches/.test(s.count || '') ? [`the count line reads ${JSON.stringify(s.count)} after load-more, which lost the total`] : []),
      ...(!s.paging.some((b) => b.label === 'Back to start') ? [`no way back to the first page: ${JSON.stringify(s.paging)}`] : []),
      ...(s.paging.some((b) => b.label === 'Load more') ? ['a third page offered where the result is exhausted'] : []),
    ]);
    s = await shot('back to start', PAGE.clickWhere('.query-paging button', 'Back to start'));
    failures += record('back to the first page', s, [
      ...(s.tiles.length !== 300 ? [`the grid holds ${s.tiles.length} tiles after back-to-start, expected the 300 of page 1`] : []),
      ...(!/showing 300 of 303/.test(s.pagingHint || '') ? [`the loaded count reads ${JSON.stringify(s.pagingHint)}`] : []),
    ]);

    // 12. provenance, now that the vault holds two kinds of local item: the image
    //     importer stores bytes into `media` and writes **no** `sources` row, while the
    //     video importer records a path as a `local` source. The two importers answer
    //     `source:` differently, and the box reports that difference instead of hiding
    //     it behind a rail that counted something else.
    s = await shot('type the site the importers write', PAGE.type('source:local'));
    s = await shot('Enter', PAGE.press('Enter'));
    failures += record('a site term, positively', s, [
      ...(!/300 matches/.test(s.count || '') ? [`count line ${JSON.stringify(s.count)}, expected 300 (only the video importer writes a sources row)`] : []),
      ...(s.tiles.length !== 300 ? [`grid shows ${s.tiles.length} tiles`] : []),
      ...(s.paging.some((b) => b.label === 'Load more') ? ['*Load more* offered over a result the count says is exhausted (300 of 300)'] : []),
    ]);
    s = await shot('type its negation', PAGE.type('-source:local'));
    s = await shot('Enter', PAGE.press('Enter'));
    failures += record('the same term negated', s, [
      ...(!/\b3 matches\b/.test(s.count || '') ? [`count line ${JSON.stringify(s.count)}, expected 3 (the disk-imported pictures carry no source row at all)`] : []),
      ...(s.tiles.length !== 3 ? [`grid shows ${s.tiles.length} tiles`] : []),
    ]);

    // 13. *Possible dupes* still opens, and the Vault section still names this vault
    s = await shot('open possible dupes', PAGE.clickWhere('.sidebar .nav-item', 'Possible dupes'));
    failures += record('the dupes panel', s, [
      ...(!/possible duplicates/i.test(s.panelTitle || '') ? [`the panel title is ${JSON.stringify(s.panelTitle)}`] : []),
      ...(!/no duplicates found/i.test(s.empty || '') ? [`the pairs panel says ${JSON.stringify(s.empty)}`] : []),
      ...(!s.vaultLine || !s.vaultLine.includes(VAULT) ? [`the Vault section does not name the throwaway vault: ${JSON.stringify(s.vaultLine)}`] : []),
    ]);

    const verdict = failures ? `${failures} case(s) broken` : 'all cases OK';
    console.log(`\nVERDICT: ${verdict}`);
    process.exitCode = failures ? 1 : 0;
  } finally {
    // the console mangles non-ASCII (the codepage), so the exact strings the app
    // printed go to a UTF-8 log as well
    try {
      fs.writeFileSync(path.join(REPO, 'tools/filter-ui-probe.log'), JSON.stringify(seen, null, 2), 'utf8');
      console.log(`raw snapshots: tools\\filter-ui-probe.log`);
    } catch (e) {
      console.log(`could not write the snapshot log: ${e.message}`);
    }
    try {
      if (ws) ws.close();
    } catch {
      /* gone */
    }
    try {
      execSync('taskkill /IM GoonFlow.exe /F /T', { stdio: 'ignore' });
    } catch {
      /* gone */
    }
    if (!process.argv.includes('--keep')) {
      for (const f of ['flow.cfg', 'chrome.cfg']) {
        try {
          fs.rmSync(path.join(SCRATCH, f), { force: true });
        } catch {
          /* nothing there */
        }
      }
    }
  }
}

main().catch((e) => {
  console.log('probe error:', e.message);
  process.exitCode = 3;
});
