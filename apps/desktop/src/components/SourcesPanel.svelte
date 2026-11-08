<script lang="ts">
  // Sources: enable/disable per site, rate limit, login cookie. Self-contained —
  // saving here never touches the grid views.
  import { onMount } from "svelte";
  import {
    getProxy,
    getPanicInfo,
    getStatus,
    getSiteSettings,
    knownSites,
    nukeDrill,
    getScrubSettings,
    scrubNow,
    setPanicHotkey,
    setProxy,
    setScrubOnLock,
    setSiteSetting,
    siteStats,
    type CompanionStatus,
    type PanicInfo,
    type SiteCount,
    type SiteInfo,
    type SiteSetting,
  } from "../lib/api";

  let siteInfos = $state<SiteInfo[]>([]);
  let siteSettings = $state<Record<string, SiteSetting>>({});
  let cookieDraft = $state<Record<string, string>>({});
  let counts = $state<SiteCount[]>([]);
  let note = $state<string | null>(null);

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
  });

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

<div class="field-label">
  Proxy <span class="hint">— route every fetch through one address (stored in the vault, not this machine)</span>
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

<div class="field-label" style="margin-top:1.2rem">
  Panic key <span class="hint">— global hotkey: locks the vault and hides the window</span>
</div>
<p class="hint">
  Right now: <strong>{panic?.hotkey ?? "CommandOrControl+Alt+L"}</strong> (works even when this window
  isn't focused). A short press locks the vault and hides the window — press again to bring the
  <em>locked</em> window back, your passphrase still guards it.
  <strong>Hold it down for 3+ seconds and the vault is crypto-erased</strong> — the same destruction as
  typing NUKE, with no confirmation dialog. Rehearse taps; never hold on the real vault.
</p>
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

{#if counts.length > 0}
  <div class="chips" title="what is already stored is never deleted by switching a site off">
    {#each counts as c (c.site)}
      <span class="chip">{c.site} · {c.count}</span>
    {/each}
  </div>
{/if}

{#each siteInfos as s (s.site)}
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

<div class="field-label" style="margin-top:1.2rem">
  Browser pairing <span class="hint">— for the GoonFlow Clip extension</span>
</div>
{#if companion}
  <p class="hint">Load <code>extensions/goonflow-clip</code> as an unpacked extension (chrome://extensions →
    developer mode → Load unpacked), then paste these into its options page. The token is fresh every app start —
    traffic never leaves this machine.</p>
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
