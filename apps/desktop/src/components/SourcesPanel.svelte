<script lang="ts">
  // Sources: enable/disable per site, rate limit, login cookie. Self-contained —
  // saving here never touches the grid views.
  import { onMount } from "svelte";
  import {
    getProxy,
    getSiteSettings,
    knownSites,
    setProxy,
    setSiteSetting,
    siteStats,
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
  });

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
