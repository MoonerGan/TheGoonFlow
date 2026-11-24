// The viewer, measured on real material rather than generated fixtures: files from
// the kinds of host in the user's library, put through the app's own clip/import
// paths, in a real session, with real clicks on Fit and 1:1.
//
// Why: `tools/viewer-layout-probe.js` proved the geometry on generated PNGs of
// exact size. It did NOT cover what a library actually holds - a 1200x850 jpeg, a
// 176x256 animated gif, a stored mp4, an e-hentai image page - and the 2026-11-23
// report ("the item still has the same bug", "anything animated and some e-hentai
// images") was about those. Measured here first: 5 of 12 cases broken.
//
// Safety: the exe runs as a copy in %TEMP%\goon-viewer-probe-real with its own
// throwaway vault (no flow.cfg, so the real vault is never attached). Cookies are
// read from the files named in GOON_F95_COOKIE / GOON_EH_COOKIE and are never
// printed. The links live in `tools/viewer-real-media-cases.json`, which is
// gitignored: they come out of someone's actual library and this repository is
// public. `tools/viewer-real-media-cases.example.json` is the shape.
//
//   node tools/viewer-real-media-probe.js            # fetch + measure
//   node tools/viewer-real-media-probe.js --reuse    # re-measure a filled vault
//   node tools/viewer-real-media-probe.js --keep     # keep the scratch folder
const { spawn, execSync } = require('child_process');
const fs = require('fs');
const net = require('net');
const os = require('os');
const path = require('path');

const REPO = 'C:/Source/TheGoonFlow';
const EXE = path.join(REPO, 'target/release/GoonFlow.exe');
const SCRATCH = path.join(os.tmpdir(), 'goon-viewer-probe-real');
const MEDIA = path.join(SCRATCH, 'media');
const VAULT = path.join(SCRATCH, 'vault');
const PASSPHRASE = 'real-media-probe-throwaway-not-a-real-one';
const LOG = path.join(REPO, 'tools/viewer-real-media-probe.log');
const CASES_FILE = process.env.GON_PROBE_CASES || path.join(REPO, 'tools/viewer-real-media-cases.json');
const UA = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36';

/**
 * The material to measure, from a local file (see the header). `how` is the app's
 * own route in: `clip` is one `clip_url`, `video` is a downloaded file imported and
 * then stored in the vault, which is the only way the viewer will play it.
 */
function loadCases() {
  if (!fs.existsSync(CASES_FILE)) {
    console.log(
      `no cases file at ${CASES_FILE}\n` +
        'copy tools\\viewer-real-media-cases.example.json to that path and put in the links you\n' +
        'actually have: this probe exists to measure the material a real library holds, and the\n' +
        'links it names are not something a public repository should carry.',
    );
    return null;
  }
  const cases = JSON.parse(fs.readFileSync(CASES_FILE, 'utf8')).cases || [];
  if (!cases.length) throw new Error(`${CASES_FILE} holds no cases`);
  for (const c of cases) {
    if (!c.key || !c.url || !['clip', 'video'].includes(c.how)) {
      throw new Error(`${CASES_FILE}: a case needs key, url and how (clip|video): ${JSON.stringify(c)}`);
    }
  }
  return cases;
}

// ---------- page side: one snapshot, prepended in its own function scope ----------
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
  const vid = document.querySelector('.viewer-stage video');
  const img = document.querySelector('.viewer-stage img:not(.frame-poster)');
  const media = (cv && cv.classList.contains('shown') ? cv : null) || vid || img || poster;
  const own = (el) => {
    if (!el) return null;
    if (el.tagName === 'CANVAS') return el.width && el.height ? { w: el.width, h: el.height } : null;
    if (el.tagName === 'VIDEO') return el.videoWidth && el.videoHeight ? { w: el.videoWidth, h: el.videoHeight } : null;
    return el.naturalWidth && el.naturalHeight ? { w: el.naturalWidth, h: el.naturalHeight } : null;
  };
  return {
    viewport: { w: window.innerWidth, h: window.innerHeight },
    stage: r(stage),
    stageVisible: stage ? { cw: stage.clientWidth, ch: stage.clientHeight, sw: stage.scrollWidth, sh: stage.scrollHeight } : null,
    box: r(box),
    media: r(media),
    mediaTag: media ? media.tagName.toLowerCase() : null,
    own: own(media),
    ownPoster: own(poster),
    mediaStyle: media ? media.getAttribute('style') : null,
    videoMeta: vid
      ? { w: vid.videoWidth, h: vid.videoHeight, ready: vid.readyState, srcHead: (vid.currentSrc || '').slice(0, 24) }
      : null,
    readout: (() => { const n = document.querySelector('.zoom-readout'); return n ? (n.textContent || '').trim() : null; })(),
    info: (() => { const n = document.querySelectorAll('.viewer-info .hint'); return n.length ? (n[n.length - 1].textContent || '').trim().replace(/\\s+/g, ' ') : null; })(),
    buttons: [...document.querySelectorAll('.viewer-zoom button')].map((b) => ({
      label: (b.textContent || '').trim(),
      pressed: b.getAttribute('aria-pressed'),
      disabled: !!b.disabled,
    })),
  };
};`;
const withMeasure = (expr) => `(() => { ${MEASURE}\n return (${expr}); })()`;

const PAGE = {
  setup: (dir, phrase) => `(async () => {
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
    return { unlocked, lastErr };
  })()`,

  /** one clip / import, then wait for the job ledger to go quiet */
  ingest: (plan, cookieSite, cookieValue) => `(async () => {
    const inv = (c, a) => window.__TAURI_INTERNALS__.invoke(c, a || {}, {});
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    const out = [];
    const ids = async () => new Set((await inv('list_items', { limit: 200 })).map((i) => i.id));
    const before = await ids();
    if (${JSON.stringify(cookieValue || '')}) {
      try {
        await inv('set_site_setting', { site: ${JSON.stringify(cookieSite)}, cookie: ${JSON.stringify(cookieValue)} });
        const v = await inv('check_site_cookie', { site: ${JSON.stringify(cookieSite)} });
        out.push('cookie: stored, app verdict ok=' + JSON.stringify(v.ok) + ' ' + JSON.stringify(v.note || ''));
      } catch (e) { out.push('cookie: ' + String(e)); }
    }
    for (const c of ${JSON.stringify(plan)}) {
      try {
        if (c.how === 'clip') {
          const rep = await inv('clip_url', { url: c.url });
          out.push(c.key + ': clip ' + JSON.stringify({ site: rep.site, created: rep.created, title: rep.title, note: rep.note }));
        } else {
          const rep = await inv('import_video_files', { paths: c.paths });
          out.push(c.key + ': import ' + JSON.stringify(rep));
        }
      } catch (e) {
        out.push(c.key + ': FAILED ' + String(e));
      }
      await sleep(1500);
    }
    // anything the import made is stored in the vault: the viewer only plays a
    // video whose bytes are inside it, which is the case the report is about
    const now = await ids();
    for (const id of [...now].filter((x) => !before.has(x))) {
      try {
        const r = await inv('store_video', { itemId: id });
        out.push('store_video ' + id + ': ' + JSON.stringify(r));
      } catch (e) {
        out.push('store_video ' + id + ': ' + String(e));
      }
    }
    for (let i = 0; i < 90; i += 1) {
      const jobs = await inv('list_jobs', {});
      const busy = jobs.filter((j) => j.state === 'running' || j.state === 'queued');
      if (!busy.length) break;
      await sleep(1000);
    }
    const items = await inv('list_items', { limit: 200, kind: 'media' });
    return { out, items: items.map((i) => ({ id: i.id, title: i.title, kind: i.kind })) };
  })()`,

  /** just what the (already filled) scratch vault holds */
  listItems: `(async () => {
    const inv = (c, a) => window.__TAURI_INTERNALS__.invoke(c, a || {}, {});
    const items = await inv('list_items', { limit: 200, kind: 'media' });
    return { out: ['(reusing the vault from an earlier run)'], items: items.map((i) => ({ id: i.id, title: i.title, kind: i.kind })) };
  })()`,

  /** open the nth media card, wait for something to be on the stage */
  openTile: (n) => `(async () => {
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    const text = (el) => (el && el.textContent ? el.textContent.trim() : '');
    let tab = null;
    for (let i = 0; i < 40 && !tab; i += 1) {
      tab = [...document.querySelectorAll('.tabs button')].find((b) => /media/i.test(text(b)));
      if (!tab) await sleep(400);
    }
    if (!tab) return { err: 'still no media tab; buttons=' + [...document.querySelectorAll('button')].map(text).slice(0, 10).join('|') };
    if (!/active/.test(tab.className)) { tab.click(); await sleep(600); }
    let tiles = [];
    for (let i = 0; i < 40; i += 1) {
      tiles = [...document.querySelectorAll('button.tile')];
      if (tiles.length > ${String(n)}) break;
      await sleep(400);
    }
    if (tiles.length <= ${String(n)}) return { err: 'only ' + tiles.length + ' tiles for index ${String(n)}' };
    tiles[${String(n)}].click();
    for (let i = 0; i < 60 && !document.querySelector('.viewer'); i += 1) await sleep(200);
    if (!document.querySelector('.viewer')) return { err: 'the viewer never opened' };
    for (let i = 0; i < 60; i += 1) {
      const s = measure();
      if (s.media && s.own) break;
      await sleep(200);
    }
    await sleep(900);
    return measure();
  })()`,

  clickZoom: (label) => `(async () => {
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    const btn = [...document.querySelectorAll('.viewer-zoom button')].find((b) => (b.textContent || '').trim() === ${JSON.stringify(label)});
    if (!btn) return { err: 'no ' + ${JSON.stringify(label)} + ' button; have ' + [...document.querySelectorAll('.viewer-zoom button')].map((b) => (b.textContent || '').trim()).join('|') };
    if (btn.disabled) return { err: ${JSON.stringify(label)} + ' is disabled' };
    btn.click();
    await sleep(500);
    return measure();
  })()`,

  close: `(() => {
    const btn = [...document.querySelectorAll('.viewer-bar button')].find((b) => (b.textContent || '').trim() === 'Close');
    if (btn) btn.click();
    return true;
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

async function download(url, file) {
  const r = await fetch(url, { headers: { 'user-agent': UA, referer: new URL(url).origin + '/' } });
  if (!r.ok) throw new Error(`http ${r.status}`);
  const buf = Buffer.from(await r.arrayBuffer());
  fs.writeFileSync(file, buf);
  return { bytes: buf.length, type: r.headers.get('content-type') };
}

const cookieFile = (which) => {
  const p = process.env[which];
  if (!p || !fs.existsSync(p)) return '';
  return fs.readFileSync(p, 'utf8').replace(/\r?\n/g, ' ').trim();
};

function report(label, s) {
  if (s.err) {
    console.log(`  ${label.padEnd(9)} ${s.err}`);
    return;
  }
  const own = s.own ? `${s.own.w}x${s.own.h}` : 'unknown';
  if (!s.media) {
    // No media element at all: an item whose bytes the app never stored. The stage
    // should be holding a sentence that says so, not a picture and not a number.
    console.log(`  ${label.padEnd(9)} nothing on the stage  info ${JSON.stringify(s.info)}`);
    return;
  }
  const fill = s.own && s.box ? `${(s.media.w / s.box.w).toFixed(2)}x${(s.media.h / s.box.h).toFixed(2)} of box` : '?';
  const pressed = s.buttons.map((b) => `${b.label}${b.pressed === 'true' ? '[on]' : ''}${b.disabled ? '[off]' : ''}`).join(' ');
  console.log(`  ${label.padEnd(9)} box ${s.box.w}x${s.box.h}  ${s.mediaTag} drawn ${s.media.w}x${s.media.h}  own ${own}  ${fill}`);
  console.log(`             readout ${JSON.stringify(s.readout)}  buttons ${pressed}`);
  console.log(`             style ${JSON.stringify(s.mediaStyle)}  video ${JSON.stringify(s.videoMeta)}`);
  console.log(`             info ${JSON.stringify(s.info)}`);
}

async function main() {
  const dumps = [];
  const reuse = process.argv.includes('--reuse');
  if (!fs.existsSync(EXE)) throw new Error(`no release exe at ${EXE} (run tools\\run-release.ps1 first)`);
  const cases = loadCases();
  if (!cases) {
    process.exitCode = 2;
    return;
  }
  if (!reuse) fs.rmSync(SCRATCH, { recursive: true, force: true });
  fs.mkdirSync(MEDIA, { recursive: true });
  fs.mkdirSync(VAULT, { recursive: true });

  // --- fetch the material the library actually holds -------------------------
  const ehCookie = cookieFile('GOON_EH_COOKIE');
  const plan = [];
  if (!reuse) {
    for (const c of cases) {
      if (c.cookie === 'ehentai' && !ehCookie) {
        console.log(`skip ${c.key}: GOON_EH_COOKIE not set, and this host is a login wall`);
        continue;
      }
      if (c.how === 'video') {
        const file = path.join(MEDIA, 'probe-animated.mp4');
        try {
          const d = await download(c.url, file);
          console.log(`fetched ${c.key}: ${d.bytes} B ${d.type}`);
          c.paths = [file];
        } catch (e) {
          console.log(`skip ${c.key}: download failed: ${e.message}`);
          continue;
        }
      } else {
        console.log(`clip ${c.key}: ${c.url}`);
      }
      plan.push({ key: c.key, url: c.url, how: c.how, paths: c.paths });
    }
  }

  const port = freePort();
  try {
    execSync('taskkill /IM GoonFlow.exe /F', { stdio: 'ignore' });
  } catch {
    /* nothing running */
  }
  await new Promise((r) => setTimeout(r, 700));

  fs.copyFileSync(EXE, path.join(SCRATCH, 'GoonFlow.exe'));
  const proc = spawn(path.join(SCRATCH, 'GoonFlow.exe'), [], {
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

    const setup = await ev(PAGE.setup(VAULT, PASSPHRASE));
    if (!setup.unlocked) throw new Error(`the scratch vault would not open: ${setup.lastErr}`);

    const ing = reuse ? await ev(PAGE.listItems) : await ev(PAGE.ingest(plan, 'ehentai', ehCookie));
    for (const line of ing.out) console.log(line);
    console.log(`items: ${JSON.stringify(ing.items)}`);

    try {
      await send('Page.reload', { ignoreCache: true });
    } catch {
      /* the context goes away mid-reload, which is the point */
    }
    await new Promise((r) => setTimeout(r, 3000));

    const count = ing.items.length;
    // Two window sizes, because the report is about the user's window: a picture
    // whose fit is below 1 in one and exactly 1 in the other is where "Fit and 1:1
    // are both lit, and Fit does nothing" comes from.
    const sizes = [
      { label: 'default window', bounds: null },
      { label: '1920x1200 window', bounds: { width: 1920, height: 1200 } },
    ];
    let windowId = null;
    try {
      const w = await send('Browser.getWindowForTarget', {});
      windowId = w.windowId;
    } catch {
      console.log('(no window id from devtools: only the default window size is measured)');
    }

    for (const size of sizes) {
      if (windowId && size.bounds) {
        await send('Browser.setWindowBounds', { windowId, bounds: size.bounds });
        await new Promise((r) => setTimeout(r, 900));
      }
      if (windowId && !size.bounds) {
        await send('Browser.setWindowBounds', { windowId, bounds: { width: 1380, height: 860 } });
        await new Promise((r) => setTimeout(r, 900));
      }
      console.log(`\n=========== ${size.label} ===========`);

      for (let n = 0; n < count; n += 1) {
        const opened = await ev(withMeasure(PAGE.openTile(n)));
        const label = `${n}: ${ing.items[n]?.title || '?'} [${ing.items[n]?.kind || '?'}]`;
        if (opened.err) {
          console.log(`\n--- ${label}\n  ${opened.err}`);
          dumps.push({ size: size.label, label, opened, notes: [opened.err] });
          continue;
        }
        // what the user actually looks at: the stage a moment after the frame
        // lands, once the preview-to-full swap and any metadata have settled
        await new Promise((r) => setTimeout(r, 1500));
        const settled = await ev(withMeasure('measure()'));
        // the stage's own sentence, for the case where there is no picture at all
        const stageNote = await ev(
          `(() => { const n = document.querySelector('.viewer-note'); return n ? (n.textContent || '').trim() : null; })()`,
        );
        const one = await ev(withMeasure(PAGE.clickZoom('1:1')));
        const fit = await ev(withMeasure(PAGE.clickZoom('Fit')));
        await ev(PAGE.close);
        await new Promise((r) => setTimeout(r, 400));

        console.log(`\n--- ${label} in a ${opened.viewport.w}x${opened.viewport.h} window ---`);
        report('first', opened);
        report('on screen', settled);
        report('after 1:1', one);
        report('after Fit', fit);

        const notes = [];
        const skips = [];
        const base = settled;
        if (!base.media) {
          // The app never stored this item's bytes (a file too large for the vault,
          // or a video kept as a link). That is a real state the viewer has to
          // describe, so it is reported separately from a broken zoom.
          skips.push(`not on stage: ${JSON.stringify(stageNote || '(the stage said nothing at all)')}`);
          if (!stageNote) notes.push('an item with no bytes leaves an empty stage: no sentence saying why');
          if (base.info && /\d+×\d+ px/.test(base.info)) {
            notes.push(`the bar claims a pixel size for an item with no bytes: ${JSON.stringify(base.info)}`);
          }
          if (base.readout && base.readout !== '?') {
            notes.push(`the readout says ${JSON.stringify(base.readout)} for an item with no bytes on stage`);
          }
          dumps.push({ size: size.label, label, opened, settled, one, fit, notes, skips });
          if (notes.length) console.log(`  PROBLEMS: ${notes.join(' · ')}`);
          else console.log(`  ${skips[0]} (honest: the stage says so, nothing is claimed)`);
          continue;
        }
        if (!base.own) notes.push(`the stage has no pixel size for this frame (${base.mediaTag})`);
        if (!base.readout || !/^\d+%$/.test(base.readout)) {
          notes.push(`readout says ${JSON.stringify(base.readout)}, not a percentage`);
        }
        if (!base.info || !/\d+×\d+ px, shown at \d+%/.test(base.info)) {
          notes.push(`info line has no honest size/scale: ${JSON.stringify(base.info)}`);
        }
        // the info line must not describe a different picture than the stage
        if (base.own && base.info) {
          const claimed = base.info.match(/(\d+)×(\d+) px/);
          if (claimed && (Number(claimed[1]) !== base.own.w || Number(claimed[2]) !== base.own.h)) {
            notes.push(`the info line claims ${claimed[1]}×${claimed[2]} px but the frame on screen is ${base.own.w}×${base.own.h}`);
          }
        }
        if (base.own) {
          // Fit must fill the box in at least one direction, whether that means
          // shrinking a big frame or enlarging a small one
          const fillW = base.media.w / base.box.w;
          const fillH = base.media.h / base.box.h;
          if (Math.max(fillW, fillH) < 0.98) {
            notes.push(`Fit fills only ${(Math.max(fillW, fillH) * 100).toFixed(0)}% of the box`);
          }
          const oneBtn = base.buttons.find((b) => b.label === '1:1');
          if (oneBtn?.pressed === 'true' && (base.own.w !== base.media.w || base.own.h !== base.media.h)) {
            notes.push('1:1 is lit while the picture is not at its own pixels');
          }
          if (oneBtn?.disabled === true) notes.push('1:1 is disabled for a picture the app has on screen');
          // a frame the app claims to fit must not spill out of the stage
          if (Math.max(fillW, fillH) > 1.02 && base.media.h > base.stageVisible.ch + 4 && base.media.w > base.stageVisible.cw + 4) {
            notes.push(`Fit leaves the frame spilling out of the stage (${base.media.w}x${base.media.h} in ${base.stageVisible.cw}x${base.stageVisible.ch})`);
          }
        }
        if (!one.err) {
          const own = base.own;
          if (own && Math.abs(one.media.w - own.w) > 3) notes.push(`1:1 drew ${one.media.w}px wide, own is ${own.w}px`);
          if (own && one.buttons.find((b) => b.label === '1:1')?.pressed !== 'true') notes.push('1:1 did not become the selected state');
          if (own && one.readout !== `${Math.round((one.media.w / own.w) * 100)}%`) {
            notes.push(`at 1:1 the readout says ${JSON.stringify(one.readout)} for a picture drawn ${(one.media.w / own.w) * 100}% of its own pixels`);
          }
        } else {
          notes.push(`1:1 unusable: ${one.err}`);
        }
        if (fit.err) notes.push(`Fit unusable: ${fit.err}`);
        dumps.push({ size: size.label, label, opened, settled, one, fit, notes });
        if (notes.length) console.log(`  PROBLEMS: ${notes.join(' · ')}`);
        else console.log('  OK: honest readout, Fit fills the box, 1:1 draws the frame at its own pixels');
      }
    }

    const bad = dumps.filter((d) => d.notes.length).length;
    const quiet = dumps.filter((d) => d.skips && d.skips.length).length;
    console.log(`\nVERDICT: ${bad} of ${dumps.length} cases broken`);
    if (quiet) {
      console.log(`(${quiet} measurement(s) had no picture on the stage at all: the app never stored those bytes)`);
      for (const d of dumps.filter((x) => x.skips && x.skips.length)) {
        console.log(`  ${d.size} | ${d.label} | ${d.skips.join(' / ')}`);
      }
    }
    process.exitCode = bad ? 1 : 0;
  } finally {
    try {
      fs.writeFileSync(LOG, JSON.stringify(dumps, null, 2), 'utf8');
      console.log(`raw snapshots: tools\\viewer-real-media-probe.log`);
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
