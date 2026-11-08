<script lang="ts">
  // Sources: enable/disable per site, rate limit, login cookie. Self-contained —
  // saving here never touches the grid views.
  import { onMount } from "svelte";
  import {
    getProxy,
    getPanicInfo,
    getChrome,
    getStatus,
    getSiteSettings,
    knownSites,
    nukeDrill,
    getScrubSettings,
    scrubNow,
    setPanicHotkey,
    setChrome,
    setProxy,
    setScrubOnLock,
    setSiteSetting,
    siteStats,
    type CompanionStatus,
    type PanicInfo,
    type ChromeInfo,
    type SiteCount,
    type SiteInfo,
    type SiteSetting,
  } from "../lib/api";

  let siteInfos = $state<SiteInfo[]>([]);
  let siteSettings = $state<Record<string, SiteSetting>>({});
  let cookieDraft = $state<Record<string, string>>({});
  let counts = $state<SiteCount[]>([]);
  let note = $state<string | null>(null);

  // #37: the list gets long — group it the way people actually think about it
  const gameSites = $derived(siteInfos.filter((s) => s.kind === "games"));
  const mirrorSites = $derived(siteInfos.filter((s) => s.kind.includes("repack mirror")));
  const mediaSites = $derived(
    siteInfos.filter((s) => s.kind === "posts" || s.kind === "galleries"),
  );
  const localSites = $derived(siteInfos.filter((s) => s.site === "local"));

  // proxy (#24)
  let proxyUrl = $state<string | null>(null);
  let proxyDraft = $state("");
  let proxyMsg = $state<string | null>(null);

  // browser pairing (#19)
  let companion = $state<CompanionStatus | null>(null);
  let copied = $state<string | null>(null);

  // panic hotkey + nuke drill (#23)
  let panic = $state<PanicInfo | null>(null);
  let panicDraft = $state("");
  let panicMsg = $state<string | null>(null);
  let drilling = $state(false);

  // scrubber (#23)
  let scrubOnLock = $state(false);
  let scrubResult = $state<string | null>(null);
  let scrubbing = $state(false);

  // decoy chrome / deniability (#27)
  let chrome = $state<ChromeInfo | null>(null);
  let chromeDraft = $state("");
  let chromeMsg = $state<string | null>(null);

  onMount(async () => {
    try {
      siteInfos = await knownSites();
      const rows = await getSiteSettings();
      siteSettings = Object.fromEntries(rows.map((r) => [r.site, r]));
      counts = await siteStats();
    } catch (e) {
      note = `sources settings unavailable: ${e}`;
    }
    try {
      proxyUrl = (await getProxy()).proxyUrl;
    } catch {
      // advisory; the save below will surface a real error
    }
    try {
      companion = (await getStatus()).companion;
    } catch {
      // advisory
    }
    try {
      panic = await getPanicInfo();
    } catch {
      // advisory (vault locked → the unlock screen has the panic defaults)
    }
    try {
      scrubOnLock = (await getScrubSettings()).scrubOnLock;
    } catch {
      // advisory
    }
    try {
      chrome = await getChrome();
    } catch {
      // advisory
    }
  });

  async function saveChrome() {
    chromeMsg = null;
    try {
      const t = chromeDraft.trim();
      chrome = await setChrome(t || null);
      chromeMsg = chrome.decoy
        ? `window title now "${chrome.decoy}" (in-app brand changes after the next lock)`
        : "decoy cleared — back to the neutral Flow title";
      chromeDraft = "";
    } catch (e) {
      chromeMsg = String(e);
    }
  }

  async function clearChrome() {
    chromeMsg = null;
    try {
      chrome = await setChrome(null);
      chromeMsg = "decoy cleared — back to the neutral Flow title";
    } catch (e) {
      chromeMsg = String(e);
    }
  }

  async function toggleScrubOnLock() {
    try {
      scrubOnLock = (await setScrubOnLock(!scrubOnLock)).scrubOnLock;
    } catch (e) {
      scrubResult = String(e);
    }
  }

  async function doScrub() {
    if (scrubbing) return;
    scrubbing = true;
    try {
      const r = await scrubNow();
      scrubResult =
        `webview profile ${r.webviewCleared ? "cleared" : "could not clear"}` +
        ` · thumbcache: ${r.thumbcacheRemoved} gone, ${r.thumbcacheLocked} still held by Explorer`;
    } catch (e) {
      scrubResult = String(e);
    } finally {
      scrubbing = false;
    }
  }

  async function savePanic() {
    panicMsg = null;
    try {
      panic = await setPanicHotkey(panicDraft.trim() || null);
      panicDraft = "";
      panicMsg = panic.stored
        ? `panic key now ${panic.hotkey} — the default Ctrl+Alt+L works too`
        : "back to the default panic key";
    } catch (e) {
      panicMsg = String(e);
    }
  }

  async function doDrill() {
    if (drilling) return;
    drilling = true;
    panicMsg = null;
    try {
      const r = await nukeDrill();
      panic = await getPanicInfo();
      panicMsg = `drill ok — ${r}`;
    } catch (e) {
      panicMsg = `drill failed: ${e}`;
    } finally {
      drilling = false;
    }
  }

  async function copy(label: string, value: string) {
    try {
      await navigator.clipboard.writeText(value);
      copied = `${label} copied`;
    } catch {
      copied = `copy failed — select and copy manually`;
    }
    setTimeout(() => (copied = null), 2000);
  }

  async function saveProxy() {
    proxyMsg = null;
    try {
      const r = await setProxy(proxyDraft.trim() || null);
      proxyUrl = r.proxyUrl;
      proxyDraft = "";
      proxyMsg = r.proxyUrl ? `all traffic now routes through ${r.proxyUrl}` : "proxy cleared — direct connections";
    } catch (e) {
      proxyMsg = String(e);
    }
  }

  async function clearProxy() {
    proxyDraft = "";
    await saveProxy();
  }

  async function saveSite(site: string, patch: { enabled?: boolean; rateMs?: number; cookie?: string }) {
    note = null;
    try {
      await setSiteSetting(site, patch.enabled, patch.cookie, patch.rateMs);
      const rows = await getSiteSettings();
      siteSettings = Object.fromEntries(rows.map((r) => [r.site, r]));
      cookieDraft[site] = "";
    } catch (e) {
      note = `could not save ${site}: ${e}`;
    }
  }
</script>

<h2 class="panel-title">Sources</h2>
<p class="hint">Turn a site off and its links are skipped on import/clip. Repack mirrors default to off (they are
  DLsite re-uploads, not new works). A cookie is only used for sites that need a login — paste the whole
  <code>Cookie</code> header value. Rate is the delay between requests to that site.</p>

{#if note}
  <p class="report-inline">{note}</p>
{/if}

<div class="group-title">
  Network <span class="hint">— one address for every fetch the app makes</span>
</div>
<div class="field-label">
  Proxy <span class="hint">— stored in the vault, not this machine</span>
</div>
<div class="row">
  <input
    type="text"
    class="proxy-input"
    placeholder={proxyUrl ?? "e.g. socks5://127.0.0.1:9150 (Tor Browser) or http://127.0.0.1:7890"}
    bind:value={proxyDraft}
    onkeydown={(e) => e.key === "Enter" && saveProxy()}
  />
  <button onclick={saveProxy}>{proxyUrl ? "Replace" : "Set proxy"}</button>
  {#if proxyUrl}
    <button class="ghost" onclick={clearProxy}>Clear</button>
  {/if}
</div>
{#if proxyUrl}
  <p class="hint">active: <code>{proxyUrl}</code> — clip, enrich, update checks, match search and video downloads all use it.</p>
{:else}
  <p class="hint">No proxy: connections go direct. With the Tor Browser running, <code>socks5://127.0.0.1:9150</code>
    puts all enrichment traffic through Tor; nothing is ever sent anywhere except the sites you track.</p>
{/if}
{#if proxyMsg}
  <p class="report-inline">{proxyMsg}</p>
{/if}

<div class="group-title" style="margin-top:1.6rem">
  Panic &amp; privacy <span class="hint">— what the app does when you need it gone</span>
</div>
<div class="field-label" style="margin-top:0.6rem">
  Panic key <span class="hint">— global hotkey: locks the vault and hides the window</span>
</div>
<p class="hint">
  Right now: <strong>{panic?.hotkey ?? "CommandOrControl+Alt+L"}</strong> (works even when this window
  isn't focused). A short press locks the vault and hides the window — press again to bring the
  <em>locked</em> window back, your passphrase still guards it. If you have hidden it and forgotten,
  just run the app again: it reveals the window instead of starting a second copy.
  <strong>Hold it down for 3+ seconds and the vault is crypto-erased</strong> — the same destruction as
  typing NUKE, with no confirmation dialog. Rehearse taps; never hold on the real vault.
</p>
{#if panic && !panic.armed}
  <p class="error">
    This key is <strong>not armed</strong>: {panic.fault ?? "registration failed"}. Pressing it does
    nothing. Close the other GoonFlow instance (or the other program holding that combination), or
    set a different key below.
  </p>
{/if}
<div class="row">
  <input
    type="text"
    class="proxy-input"
    placeholder={panic?.stored ? "replace stored key" : "custom key, e.g. CommandOrControl+Alt+K (empty = default)"}
    bind:value={panicDraft}
    onkeydown={(e) => e.key === "Enter" && savePanic()}
  />
  <button onclick={savePanic}>Set</button>
  {#if panic?.stored}
    <button class="ghost" onclick={() => { panicDraft = ""; void setPanicHotkey(null).then((p) => (panic = p)).catch((e) => (panicMsg = String(e))); }}>Default</button>
  {/if}
</div>
<div class="row">
  <button disabled={drilling} onclick={doDrill}>{drilling ? "Drilling…" : "Run crypto-erase drill"}</button>
  <span class="hint">
    {#if panic?.lastDrill}
      last drill {new Date(Number(panic.lastDrill) * 1000).toLocaleString()}
    {:else}
      never run — proves the nuke really destroys a throwaway vault
    {/if}
  </span>
</div>
{#if panicMsg}
  <p class="report-inline">{panicMsg}</p>
{/if}

<div class="field-label" style="margin-top:1.2rem">
  Scrubber <span class="hint">— cleans traces the vault never held (web session, Explorer thumbnails)</span>
</div>
<div class="row">
  <button disabled={scrubbing} onclick={doScrub}>{scrubbing ? "Scrubbing…" : "Scrub now"}</button>
  <label class="inline-check">
    <input type="checkbox" checked={scrubOnLock} onchange={toggleScrubOnLock} />
    scrub every time I lock or hit the panic key
  </label>
</div>
{#if scrubResult}
  <p class="report-inline">{scrubResult}</p>
{/if}

<div class="field-label" style="margin-top:1.2rem">
  Decoy title <span class="hint">— deniability: rename the exe in Explorer freely; this dresses the window too</span>
</div>
<p class="hint">
  Renaming the exe/folder already works (everything resolves next to the exe). The locked screens show no branding
  and the window title is a neutral <em>Flow</em> — a decoy replaces even that. Stored as one plain line in
  <code>chrome.cfg</code> beside the exe; it changes the taskbar title immediately and the in-app brand after the
  next lock. (Explorer's file <em>properties</em> still show the original internal name — that one needs a rebuild
  to change.)
</p>
<div class="row">
  <input
    type="text"
    class="proxy-input"
    placeholder={chrome?.decoy ?? "boring title, e.g. Flow Notes (empty = clear)"}
    bind:value={chromeDraft}
    onkeydown={(e) => e.key === "Enter" && saveChrome()}
  />
  <button onclick={saveChrome}>Set</button>
  {#if chrome?.decoy}
    <button class="ghost" onclick={() => { chromeDraft = ""; void clearChrome(); }}>Clear</button>
  {/if}
</div>
{#if chromeMsg}
  <p class="report-inline">{chromeMsg}</p>
{/if}

<div class="group-title">Game sites</div>
{#if counts.length > 0}
  <div class="chips" title="what is already stored is never deleted by switching a site off">
    {#each counts as c (c.site)}
      <span class="chip">{c.site} · {c.count}</span>
    {/each}
  </div>
{/if}

{#snippet siteRows(list: SiteInfo[])}
  {#each list as s (s.site)}
    {@const cur = siteSettings[s.site]}
    <div class="site-row">
      <div class="site-name">
        <strong>{s.site}</strong>
        <span class="hint">{s.kind}{s.mirror ? " · off by default" : ""}</span>
      </div>
      <label class="inline-check">
        <input
          type="checkbox"
          checked={cur ? cur.enabled : !s.mirror}
          onchange={(e) => saveSite(s.site, { enabled: e.currentTarget.checked })}
        />
        enabled
      </label>
      <div class="site-rate">
        <input
          type="text"
          inputmode="numeric"
          value={cur ? String(cur.rateMs) : String(s.defaultRateMs)}
          onchange={(e) => {
            const n = Number(e.currentTarget.value);
            if (Number.isFinite(n)) saveSite(s.site, { rateMs: Math.round(n) });
          }}
        />
        <span class="hint">ms between hits</span>
      </div>
      <div class="site-cookie">
        <input
          type="text"
          placeholder={cur?.cookieSet ? "cookie saved — paste to replace" : "cookie (optional)"}
          bind:value={cookieDraft[s.site]}
          onkeydown={(e) => e.key === "Enter" && cookieDraft[s.site] && saveSite(s.site, { cookie: cookieDraft[s.site] })}
        />
        <button onclick={() => cookieDraft[s.site] && saveSite(s.site, { cookie: cookieDraft[s.site] })}>Set</button>
      </div>
    </div>
  {/each}
{/snippet}

{@render siteRows(gameSites)}

<div class="group-title">Repack mirrors</div>
<p class="hint">DLsite re-uploads, not new works. Off by default (D8) — turn one on only if a work is missing
  from DLsite itself.</p>
{@render siteRows(mirrorSites)}

<div class="group-title">Imageboards and galleries</div>
<p class="hint">E-hentai and f95 want a login cookie before their pages give anything useful — see
  <em>Getting a cookie</em> at the bottom of this page.</p>
{@render siteRows(mediaSites)}

<div class="group-title">Local files</div>
<p class="hint">Nothing to enable: this is the folder/file side of the vault. Items with only a local source show
  up under <em>Sources → Local only</em> in the Games and Media sidebars, which is the queue for adding links
  and tags by hand.</p>
{@render siteRows(localSites)}

<div class="group-title" style="margin-top:1.6rem">
  Browser pairing <span class="hint">— for the GoonFlow Clip extension</span>
</div>
{#if companion}
  <p class="hint">Chrome/Edge: <code>extensions/goonflow-clip</code> through chrome://extensions → developer mode →
    <em>Load unpacked</em>. Firefox: use <code>extensions/goonflow-clip-firefox</code> (the Chrome build needs service
    workers, which hardened Firefox profiles disable) on about:add-ons → gear icon → <em>Load Temporary Add-on</em> →
    pick that folder's <code>manifest.json</code>. Then click the toolbar icon → <em>Pairing / options</em> and paste
    these in. The token is fresh every app start — traffic never leaves this machine.</p>
  <p class="hint">The icon itself is the quick path: it shows whether the app is reachable and whether the vault is
    locked, and clipping the page is one button. Right-click any link, image or selection to clip something else.</p>
  <div class="row">
    <span class="chip">port <strong>{companion.port}</strong></span>
    <button class="ghost" onclick={() => copy("port", String(companion?.port ?? ""))}>Copy port</button>
    <span class="chip token-chip">token <strong>{companion.token}</strong></span>
    <button class="ghost" onclick={() => copy("token", companion?.token ?? "")}>Copy token</button>
  </div>
  {#if copied}
    <p class="report-inline">{copied}</p>
  {/if}
{:else}
  <p class="hint">The companion API is disabled on this run (port bind failed) — browser clipping is unavailable.</p>
{/if}

<div class="group-title" style="margin-top:1.6rem">
  Getting a cookie <span class="hint">— only for sites that need a login (f95, E-hentai, Exhentai)</span>
</div>
<details class="details">
  <summary>How to copy the <code>Cookie</code> header value</summary>
  <ol class="hint steps">
    <li>Open the site in your browser and make sure you are logged in there. A
      cookie copied while you were logged out is worthless.</li>
    <li>Press F12 → the <em>Network</em> tab → reload the page → click the first request (the page itself).</li>
    <li>Under <em>Request Headers</em> find the line starting <code>Cookie:</code> and copy <strong>everything after
      the colon</strong> — one long single line, no quotes. Keep the <code>=</code> and <code>;</code>, and leave out
      the word <code>Cookie:</code> itself.</li>
    <li>Paste it into that site's cookie box above and hit <em>Set</em>. It is stored inside the encrypted vault,
      never in a config file next to the exe. The app sends exactly what you pasted as the <code>Cookie</code>
      header.</li>
  </ol>
  <p class="hint">
    <strong>Only f95, E-hentai and Exhentai need one.</strong> DLsite and every imageboard work without it, so
    leave those boxes empty.
  </p>
  <p class="hint">
    How to tell the paste is real: an <strong>f95</strong> cookie must contain <code>xf_session</code> and
    <code>xf_user</code> (usually also <code>xf_csrf</code> and Cloudflare's <code>cf_clearance</code>); an
    <strong>E-hentai / Exhentai</strong> cookie must contain <code>ipb_member_id</code> and
    <code>ipb_pass_hash</code>. No <code>xf_session</code> means step 1 didn't happen. On f95 the cookie is what
    makes the thread's screenshots visible at all — without it the page is a "register to view" wall, so a clipped
    f95 link gives you a title and nothing else.
  </p>
  <p class="hint">
    The cookie is just a saved copy of what your browser already sends — the app never reads your browser's own
    cookie store. If a fetch starts failing after you log out or the site rotates the session, re-copy it; f95 says
    so out loud (<em>"f95 wants a login…"</em>) instead of quietly importing nothing.
  </p>
</details>
