// The viewer, measured in a real session: real exe, real WebView2, real vault (a
// throwaway one in %TEMP%), real items, real clicks on the real buttons.
//
// Why: the 2026-11-22 report says **Fit** does not fit the picture to the window,
// that Fit and 1:1 cannot be told apart, and that both read as selected for
// pictures plainly bigger than the screen. `lib/view-zoom.ts` is unit-tested and a
// bare HTML fixture says the CSS contains the picture - neither is what the user
// sees. What decides it is (a) the box the *live* stage really gets, (b) what the
// app's own readout and button states claim, and (c) how many CSS px the engine
// actually draws the picture at. This measures all three per picture: open it,
// click 1:1, click Fit, compare claim with drawing.
//
// Safety: the exe runs as a copy in %TEMP%\goon-viewer-probe with its own vault
// folder and a throwaway passphrase, so no real vault is ever attached and none of
// the user's data is read. Like the other live tools it stops running instances
// first: the single-instance plugin would otherwise hand the copy off instead of
// opening its own window.
//
//   node tools/viewer-layout-probe.js            # all cases
//   node tools/viewer-layout-probe.js --keep     # leave the scratch folder alone
const { spawn, execSync } = require('child_process');
const fs = require('fs');
const net = require('net');
const os = require('os');
const path = require('path');
const zlib = require('zlib');

const REPO = 'C:/Source/TheGoonFlow';
const EXE = path.join(REPO, 'target/release/GoonFlow.exe');
const SCRATCH = path.join(os.tmpdir(), 'goon-viewer-probe');
const MEDIA = path.join(SCRATCH, 'media');
const VAULT = path.join(SCRATCH, 'vault');
const PASSPHRASE = 'layout-probe-throwaway-not-a-real-one';

/** Pictures of exact pixel size; one wider than the stage, one taller, one small
 *  enough that Fit and 1:1 legitimately are the same state. */
const CASES = [
  { want: 'wide', w: 2400, h: 1600 },
  { want: 'tall', w: 1200, h: 3000 },
  { want: 'small', w: 600, h: 400 },
];

// ---------- a small solid-colour PNG encoder: exact sizes, no dependencies ----------
const crcTable = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
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
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // truecolour RGB
  const rows = [];
  for (let y = 0; y < h; y += 1) {
    const row = Buffer.alloc(1 + w * 3);
    for (let x = 0; x < w; x += 1) {
      const i = 1 + x * 3;
      const band = ((x >> 5) + (y >> 5)) % 2;
      row[i] = band ? 0x2a : 0xd8;
      row[i + 1] = band ? 0x6f : 0x35;
      row[i + 2] = band ? 0xa8 : 0x52;
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

// ---------- page side ----------
/** One snapshot of geometry + what the app itself claims. Prepended to each probe
 *  in its own function scope, because a top-level `const` from an earlier
 *  Runtime.evaluate would still be sitting in the page's global lexical scope. */
const MEASURE = `const measure = () => {
  const r = (el) => {
    if (!el) return null;
    const b = el.getBoundingClientRect();
    return { x: Math.round(b.x), y: Math.round(b.y), w: Math.round(b.width), h: Math.round(b.height) };
  };
  const stage = document.querySelector('.viewer-stage');
  const box = document.querySelector('.frame-box');
  const cv = document.querySelector('.viewer-frame');
  const poster = document.querySelector('.frame-poster');
  const media = cv && cv.classList.contains('shown') ? cv : poster;
  const readout = document.querySelector('.zoom-readout');
  const info = document.querySelectorAll('.viewer-info .hint');
  return {
    viewport: { w: window.innerWidth, h: window.innerHeight },
    stage: r(stage),
    stageVisible: stage ? { cw: stage.clientWidth, ch: stage.clientHeight, sw: stage.scrollWidth, sh: stage.scrollHeight } : null,
    bar: r(document.querySelector('.viewer-bar')),
    box: r(box),
    media: r(media),
    mediaTag: media ? media.tagName.toLowerCase() : null,
    canvasPixels: cv ? { w: cv.width, h: cv.height } : null,
    posterNatural: poster ? { w: poster.naturalWidth, h: poster.naturalHeight } : null,
    transform: media ? getComputedStyle(media).transform : null,
    readout: readout ? (readout.textContent || '').trim() : null,
    info: info.length ? (info[info.length - 1].textContent || '').trim().replace(/\\s+/g, ' ') : null,
    buttons: [...document.querySelectorAll('.viewer-zoom button')].map((b) => ({
      label: (b.textContent || '').trim(),
      pressed: b.getAttribute('aria-pressed'),
      disabled: !!b.disabled,
    })),
  };
};`;
const withMeasure = (expr) => `(() => { ${MEASURE}\n return (${expr}); })()`;

const PAGE = {
  /** Backend-side setup: throwaway vault, import the three pictures as media. */
  setup: (dir, phrase, paths) => `(async () => {
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
    const report = await inv('import_image_files', { paths: ${JSON.stringify(paths)} });
    const items = await inv('list_items', { limit: 20, kind: 'media' });
    return { unlocked, report, items: items.map((i) => ({ id: i.id, title: i.title })) };
  })()`,

  /** Media tab, open the first card, wait for the frame to be on screen. */
  open: `(async () => {
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    const text = (el) => (el && el.textContent ? el.textContent.trim() : '');
    let tab = null;
    for (let i = 0; i < 40 && !tab; i += 1) {
      tab = [...document.querySelectorAll('.tabs button')].find((b) => /media/i.test(text(b)));
      if (!tab) await sleep(400);
    }
    if (!tab) return { err: 'still no media tab; buttons=' + [...document.querySelectorAll('button')].map(text).slice(0, 10).join('|') };
    if (!/active/.test(tab.className)) { tab.click(); await sleep(500); }
    let tile = null;
    for (let i = 0; i < 40 && !tile; i += 1) {
      tile = document.querySelector('button.tile');
      if (!tile) await sleep(300);
    }
    if (!tile) return { err: 'no tiles; titles=' + [...document.querySelectorAll('.tile-title')].map(text).join('|') };
    tile.click();
    for (let i = 0; i < 50 && !document.querySelector('.viewer'); i += 1) await sleep(200);
    if (!document.querySelector('.viewer')) return { err: 'the viewer never opened' };
    for (let i = 0; i < 50; i += 1) {
      const cv = document.querySelector('.viewer-frame.shown');
      if (cv && cv.width > 0) break;
      await sleep(200);
    }
    await sleep(600);
    return measure();
  })()`,

  clickZoom: (label) => `(async () => {
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    const btn = [...document.querySelectorAll('.viewer-zoom button')].find((b) => (b.textContent || '').trim() === ${JSON.stringify(label)});
    if (!btn) return { err: 'no ' + ${JSON.stringify(label)} + ' button; have ' + [...document.querySelectorAll('.viewer-zoom button')].map((b) => (b.textContent || '').trim()).join('|') };
    if (btn.disabled) return { err: ${JSON.stringify(label)} + ' is disabled' };
    btn.click();
    await sleep(450);
    return measure();
  })()`,

  nextFrame: `(async () => {
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    const before = (() => { const c = document.querySelector('.viewer-frame'); return c ? c.width + 'x' + c.height : ''; })();
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    for (let i = 0; i < 25; i += 1) {
      await sleep(200);
      const c = document.querySelector('.viewer-frame.shown');
      if (c && c.width + 'x' + c.height !== before && c.width > 0) break;
    }
    await sleep(400);
    return measure();
  })()`,
};

function freePort() {
  for (let p = 9223; p <= 9243; p += 1) {
    try {
      const s = net.createServer();
      s.listen(p, '127.0.0.1');
      s.close();
      return p;
    } catch {
      /* taken */
    }
  }
  throw new Error('no free loopback port');
}

const isContained = (s) =>
  !!s && !!s.media && !!s.stage && !!s.stageVisible &&
  s.media.x >= s.stage.x - 2 &&
  s.media.y >= s.stage.y - 2 &&
  s.media.x + s.media.w <= s.stage.x + s.stageVisible.cw + 2 &&
  s.media.y + s.media.h <= s.stage.y + s.stageVisible.ch + 2;

const ownPixels = (s) =>
  s.canvasPixels && s.canvasPixels.w ? s.canvasPixels : s.posterNatural && s.posterNatural.w ? s.posterNatural : null;

function report(label, s) {
  if (s.err) {
    console.log(`  ${label.padEnd(9)} ${s.err}`);
    return;
  }
  const own = ownPixels(s);
  const drawn = s.media ? `${s.media.w}x${s.media.h}` : 'none';
  const scale = own && s.media ? (s.media.w / own.w).toFixed(3) : '?';
  const pressed = s.buttons.map((b) => `${b.label}${b.pressed === 'true' ? '[on]' : ''}${b.disabled ? '[disabled]' : ''}`).join(' ');
  console.log(`  ${label.padEnd(9)} stage ${s.stage.w}x${s.stage.h} visible ${s.stageVisible.cw}x${s.stageVisible.ch} scroll ${s.stageVisible.sw}x${s.stageVisible.sh}  box ${s.box.w}x${s.box.h}`);
  console.log(`             media ${drawn} (${s.mediaTag}) own ${own ? own.w + 'x' + own.h : '?'} poster ${s.posterNatural ? s.posterNatural.w + 'x' + s.posterNatural.h : '-'} drawn/own ${scale} transform ${s.transform} contained ${isContained(s) ? 'YES' : 'NO'}`);
  console.log(`             readout ${JSON.stringify(s.readout)}  buttons ${pressed}`);
  console.log(`             info ${JSON.stringify(s.info)}`);
}

async function main() {
  const dumps = [];
  if (!fs.existsSync(EXE)) throw new Error(`no release exe at ${EXE} (run tools\\run-release.ps1 first)`);
  // fresh scratch every run: a stale vault or stale items would change the answer
  fs.rmSync(SCRATCH, { recursive: true, force: true });
  fs.mkdirSync(MEDIA, { recursive: true });
  fs.mkdirSync(VAULT, { recursive: true });
  const paths = [];
  for (const c of CASES) {
    const f = path.join(MEDIA, `${c.want}-${c.w}x${c.h}.png`);
    const bytes = writePng(f, c.w, c.h);
    paths.push(f);
    console.log(`fixture ${path.basename(f)}: ${bytes} B`);
  }

  const port = freePort();
  try {
    execSync('taskkill /IM GoonFlow.exe /F', { stdio: 'ignore' });
  } catch {
    /* nothing running */
  }
  await new Promise((r) => setTimeout(r, 700));

  fs.mkdirSync(SCRATCH, { recursive: true });
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
    ws.addEventListener('message', (ev) => {
      const m = JSON.parse(ev.data);
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

    const setup = await ev(PAGE.setup(VAULT, PASSPHRASE, paths));
    if (!setup.unlocked) throw new Error(`the scratch vault would not open: ${setup.lastErr}`);
    console.log(`vault unlocked=${setup.unlocked} import=${JSON.stringify(setup.report)} items=${JSON.stringify(setup.items)}`);
    // the unlock came from outside the page, so the UI has not noticed it: reload
    try {
      await send('Page.reload', { ignoreCache: true });
    } catch {
      /* the context goes away mid-reload, which is the point */
    }
    await new Promise((r) => setTimeout(r, 3000));

    const opened = await ev(withMeasure(PAGE.open));
    if (opened.err) throw new Error(opened.err);

    const seen = new Map();
    const keyOf = (s) => {
      const own = ownPixels(s);
      return own ? `${own.w}x${own.h}` : null;
    };
    for (let guard = 0; guard < 12 && seen.size < CASES.length; guard += 1) {
      const s = guard === 0 ? opened : await ev(withMeasure(PAGE.nextFrame));
      const key = keyOf(s);
      if (!key || seen.has(key) || !CASES.some((c) => `${c.w}x${c.h}` === key)) {
        if (s.err) console.log(`  frame step: ${s.err}`);
        continue;
      }
      const one = await ev(withMeasure(PAGE.clickZoom('1:1')));
      const fit = await ev(withMeasure(PAGE.clickZoom('Fit')));
      seen.set(key, { opened: s, one, fit });
    }

    let failures = 0;
    for (const c of CASES) {
      const key = `${c.w}x${c.h}`;
      const s = seen.get(key);
      console.log(`\n--- ${c.want} ${c.w}x${c.h} in a ${opened.viewport.w}x${opened.viewport.h} window ---`);
      if (!s) {
        console.log('  never reached on screen');
        failures += 1;
        continue;
      }
      report('on open', s.opened);
      report('after 1:1', s.one);
      report('after Fit', s.fit);

      const box = s.opened.box;
      const biggerThanStage = box.w < c.w || box.h < c.h;
      const notes = [];
      if (!isContained(s.opened)) notes.push('the picture is NOT contained by the stage');
      const pressedOnOpen = s.opened.buttons.filter((b) => b.pressed === 'true').map((b) => b.label);
      if (biggerThanStage && pressedOnOpen.length !== 1) {
        notes.push(`Fit/1:1 states: ${pressedOnOpen.join(' + ') || 'none'} for a picture larger than the box`);
      }
      if (!s.one.err && biggerThanStage) {
        if (s.one.media.w === s.opened.media.w && s.one.media.h === s.opened.media.h) notes.push('1:1 did not change the drawn size');
        const own = ownPixels(s.opened);
        // a couple of px is the sub-pixel rounding of the transform; anything more
        // means the box moved under the zoom (the scrollbar feedback bug)
        if (own && Math.abs(s.one.media.w - own.w) > 3) notes.push(`1:1 drew ${s.one.media.w}px wide, own is ${own.w}px`);
        if (s.one.stageVisible && s.one.stageVisible.sw + 4 < s.one.media.w) {
          notes.push(`1:1 does not make the stage scrollable (scroll ${s.one.stageVisible.sw}, drawn ${s.one.media.w}): the picture cannot be panned`);
        }
      }
      if (s.one.err) notes.push(`1:1 unusable: ${s.one.err}`);
      if (s.fit.err) notes.push(`Fit unusable: ${s.fit.err}`);
      if (!s.opened.readout || /zoom \d+%|^\?$|NaN/.test(s.opened.readout)) {
        notes.push(`the scale readout says ${JSON.stringify(s.opened.readout)} instead of the picture's own scale`);
      }
      if (!s.opened.info || !/\d+×\d+ px, shown at \d+%/.test(s.opened.info)) {
        notes.push(`the info line has no honest size/scale: ${JSON.stringify(s.opened.info)}`);
      }
      if (!s.one.err && biggerThanStage) {
        const p = s.one.buttons.find((b) => b.label === '1:1');
        if (p && p.pressed !== 'true') notes.push('1:1 did not become the selected state');
      }
      if (!s.fit.err && (s.fit.media.w !== s.opened.media.w || s.fit.media.h !== s.opened.media.h)) {
        notes.push('Fit did not return to the opening size');
      }
      dumps.push({ label: `${c.want} ${c.w}x${c.h}`, opened: s.opened, one: s.one, fit: s.fit, notes });
      if (notes.length) {
        failures += 1;
        console.log(`  PROBLEMS: ${notes.join(' · ')}`);
      } else {
        console.log(`  OK: contained, 1:1 draws ${c.w}px, Fit returns to the fitted size`);
      }
    }
    console.log(`\nVERDICT: ${failures ? `${failures} case(s) broken` : 'all cases OK'}`);
    process.exitCode = failures ? 1 : 0;
  } finally {
    // the console mangles non-ASCII (Shift_JIS codepage), so the exact strings the
    // app printed go to a UTF-8 log as well
    try {
      fs.writeFileSync(path.join(REPO, 'tools/viewer-layout-probe.log'), JSON.stringify(dumps || [], null, 2), 'utf8');
      console.log(`raw snapshots: tools\\viewer-layout-probe.log`);
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
