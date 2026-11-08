<script lang="ts">
  // Sources: enable/disable per site, rate limit, login cookie. Self-contained —
  // saving here never touches the grid views.
  import { onMount } from "svelte";
  import {
    getSiteSettings,
    knownSites,
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

  onMount(async () => {
    try {
      siteInfos = await knownSites();
      const rows = await getSiteSettings();
      siteSettings = Object.fromEntries(rows.map((r) => [r.site, r]));
      counts = await siteStats();
    } catch (e) {
      note = `sources settings unavailable: ${e}`;
    }
  });

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
